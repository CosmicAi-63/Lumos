//! Sharing end to end against a fake Google: record with the fake backend, sign in through the
//! OAuth loopback (PKCE checked, a wrong `state` rejected), upload with a forced resume and a
//! server error mid-upload, refresh an expired access token, and keep the link in the manifest.
//!
//! One test per file: the engine reads Google's endpoints from process-wide env vars.
//! Needs `ffmpeg` on PATH (or LUMOS_FFMPEG), like record_flow.rs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use lumos_capture::fake::FakeBackend;
use lumos_core::{Engine, EngineOptions};
use lumos_model::ErrorCode;
use lumos_model::manifest::Manifest;
use lumos_rpc::paths::Paths;
use lumos_rpc::{Client, Dispatch};
use serde_json::{Value, json};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
struct Fake {
    /// The PKCE challenge from the sign-in URL; the token endpoint checks the verifier against it.
    challenge: String,
    access: String,
    refreshes: u32,
    /// Answer the next Drive metadata call with 401 once (the access token "expired").
    expire_next: bool,
    folders: u32,
    /// Upload session state.
    received: Vec<u8>,
    total: u64,
    puts: u32,
    status_queries: u32,
    file: Option<(String, Vec<u8>)>,
    public: bool,
    log: Vec<String>,
}

struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_req(sock: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: HashMap<String, String> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Req { method, path, headers, body })
}

async fn reply(sock: &mut tokio::net::TcpStream, status: u16, extra: &[(&str, String)], body: &str) {
    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n", body.len());
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.shutdown().await;
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

/// A tiny stand-in for accounts.google.com, oauth2.googleapis.com and the Drive API.
async fn fake_google(state: Arc<Mutex<Fake>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let session_url = format!("{base}/session/1");
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let state = state.clone();
            let session_url = session_url.clone();
            tokio::spawn(async move {
                let Some(req) = read_req(&mut sock).await else { return };
                let (status, extra, body) = handle(&state, &req, &session_url);
                reply(&mut sock, status, &extra, &body).await;
            });
        }
    });
    base
}

fn handle(state: &Mutex<Fake>, req: &Req, session_url: &str) -> (u16, Vec<(&'static str, String)>, String) {
    let mut f = state.lock().unwrap();
    f.log.push(format!("{} {}", req.method, req.path.split('?').next().unwrap_or("")));
    let path = req.path.as_str();
    let authed = || req.headers.get("authorization").map(String::as_str) == Some(&format!("Bearer {}", f.access));

    if path == "/token" {
        let p = form(&req.body);
        match p.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                let verifier = p.get("code_verifier").cloned().unwrap_or_default();
                let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
                if p.get("code").map(String::as_str) != Some("good-code") || challenge != f.challenge || p.get("client_secret").map(String::as_str) != Some("test-secret") {
                    return (400, vec![], json!({ "error": "invalid_grant" }).to_string());
                }
                f.access = "access-1".into();
                return (200, vec![], json!({ "access_token": "access-1", "expires_in": 3600, "refresh_token": "refresh-1" }).to_string());
            }
            Some("refresh_token") if p.get("refresh_token").map(String::as_str) == Some("refresh-1") => {
                f.refreshes += 1;
                f.access = format!("access-{}", f.refreshes + 1);
                return (200, vec![], json!({ "access_token": f.access, "expires_in": 3600 }).to_string());
            }
            _ => return (400, vec![], json!({ "error": "invalid_grant" }).to_string()),
        }
    }
    if path == "/revoke" {
        return (200, vec![], "{}".into());
    }
    if path.starts_with("/session/") {
        // Resumable upload chunks and status queries.
        let range = req.headers.get("content-range").cloned().unwrap_or_default();
        if let Some(total) = range.strip_prefix("bytes */") {
            f.status_queries += 1;
            assert_eq!(total.parse::<u64>().unwrap(), f.total);
            let n = f.received.len();
            return (308, if n > 0 { vec![("Range", format!("bytes=0-{}", n - 1))] } else { vec![] }, String::new());
        }
        f.puts += 1;
        let spec = range.strip_prefix("bytes ").unwrap();
        let (span, total) = spec.split_once('/').unwrap();
        let start: usize = span.split_once('-').unwrap().0.parse().unwrap();
        assert_eq!(start, f.received.len(), "the client must resume where the server left off");
        assert_eq!(total.parse::<u64>().unwrap(), f.total);
        if f.puts == 2 {
            // A server hiccup: drop the chunk, so the client must ask where to resume.
            return (503, vec![], "{}".into());
        }
        // The first chunk: keep only half, so the client must send the rest again.
        let keep = if f.puts == 1 { req.body.len() / 2 } else { req.body.len() };
        let body = req.body[..keep].to_vec();
        f.received.extend_from_slice(&body);
        if (f.received.len() as u64) < f.total {
            let n = f.received.len();
            return (308, vec![("Range", format!("bytes=0-{}", n - 1))], String::new());
        }
        let data = std::mem::take(&mut f.received);
        f.file = Some(("file-1".into(), data));
        return (200, vec![], json!({ "id": "file-1", "webViewLink": "https://drive.google.com/file/d/file-1/view?usp=drivesdk" }).to_string());
    }

    // Everything below needs a valid access token.
    if f.expire_next && path.starts_with("/drive/") {
        f.expire_next = false;
        return (401, vec![], json!({ "error": { "code": 401, "message": "Invalid Credentials" } }).to_string());
    }
    if !authed() {
        return (401, vec![], json!({ "error": { "code": 401, "message": "Invalid Credentials" } }).to_string());
    }
    match (req.method.as_str(), path.split('?').next().unwrap_or("")) {
        ("GET", "/drive/v3/about") => (200, vec![], json!({ "user": { "emailAddress": "tester@example.com" } }).to_string()),
        ("POST", "/drive/v3/files") => {
            let meta: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(meta["mimeType"], "application/vnd.google-apps.folder");
            assert_eq!(meta["name"], "Lumos");
            f.folders += 1;
            (200, vec![], json!({ "id": format!("folder-{}", f.folders) }).to_string())
        }
        ("GET", p) if p.starts_with("/drive/v3/files/folder-") => (200, vec![], json!({ "id": "folder-1", "trashed": false }).to_string()),
        ("POST", "/upload/drive/v3/files") => {
            assert!(path.contains("uploadType=resumable"));
            let meta: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(meta["parents"], json!(["folder-1"]));
            assert_eq!(meta["mimeType"], "video/mp4");
            assert!(meta["name"].as_str().unwrap().ends_with(".mp4"));
            f.total = req.headers.get("x-upload-content-length").unwrap().parse().unwrap();
            f.received.clear();
            (200, vec![("Location", session_url.to_string())], String::new())
        }
        ("POST", "/drive/v3/files/file-1/permissions") => {
            let p: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(p, json!({ "type": "anyone", "role": "reader" }));
            f.public = true;
            (200, vec![], json!({ "id": "anyoneWithLink" }).to_string())
        }
        _ => (404, vec![], json!({ "error": { "code": 404, "message": "not found" } }).to_string()),
    }
}

async fn hello(socket: &std::path::Path, kind: &str) -> Client {
    let c = Client::connect(socket).await.unwrap();
    c.call("session.hello", json!({ "client": { "kind": kind, "name": "test", "version": "0" }, "protocol": 1 })).await.unwrap();
    c
}

/// Act as the browser: open the redirect URL Google would send the user back to.
async fn visit(url: &str) -> u16 {
    let u = url::Url::parse(url).unwrap();
    let mut sock = tokio::net::TcpStream::connect((u.host_str().unwrap(), u.port().unwrap())).await.unwrap();
    let target = format!("{}?{}", u.path(), u.query().unwrap_or(""));
    sock.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    sock.read_to_string(&mut out).await.unwrap();
    out.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn connect_record_and_share_a_link() {
    let fake = Arc::new(Mutex::new(Fake::default()));
    let base = fake_google(fake.clone()).await;
    // SAFETY: set before the engine (the only reader) starts; this file has one test.
    unsafe {
        std::env::set_var("LUMOS_GOOGLE_AUTH_URL", format!("{base}/auth"));
        std::env::set_var("LUMOS_GOOGLE_TOKEN_URL", format!("{base}/token"));
        std::env::set_var("LUMOS_GOOGLE_REVOKE_URL", format!("{base}/revoke"));
        std::env::set_var("LUMOS_GOOGLE_API_URL", &base);
        std::env::set_var("LUMOS_GOOGLE_UPLOAD_URL", &base);
        // The smallest chunk Google allows, so even a short test video takes several requests.
        std::env::set_var("LUMOS_UPLOAD_CHUNK_KB", "256");
    }

    let home = tempfile::Builder::new().prefix("lm").tempdir_in("/tmp").unwrap();
    let paths = Paths::under(home.path().to_path_buf());
    let save = home.path().join("save");
    let engine = Engine::new(EngineOptions { paths: paths.clone(), host: "test".into(), backend: Some(Arc::new(FakeBackend)) }).unwrap();
    let listener = lumos_rpc::bind(&paths.socket).unwrap();
    tokio::spawn(lumos_rpc::serve(listener, engine.clone() as Arc<dyn Dispatch>));
    let c = hello(&paths.socket, "cli").await;
    c.call("settings.set", json!({ "patch": { "storage.saveFolder": save.to_str().unwrap(), "recording.minFreeDiskMB": 1 } })).await.unwrap();

    // Not set up yet: no client JSON, so status says so and upload asks to connect.
    let st = c.call("share.status", json!({})).await.unwrap();
    assert_eq!(st["configured"], false);
    assert_eq!(st["connected"], false);

    // Record a short clip with the fake backend. Make it long enough to need several 256 KB chunks.
    let started = c.call("recording.start", json!({ "countdownSeconds": 0, "title": "Share Test" })).await.unwrap();
    let id = started["recordingId"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let stopped = c.call("recording.stop", json!({ "wait": true, "timeoutMs": 60000 })).await.unwrap();
    let mp4 = std::path::PathBuf::from(stopped["outputPath"].as_str().unwrap());
    let original = std::fs::read(&mp4).unwrap();
    assert!(original.len() > 3 * 256 * 1024 / 2, "test video is {} bytes; too small to exercise resume", original.len());

    let e = c.call("share.upload", json!({ "id": id })).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthRequired);

    // Agents can't connect accounts, and can't share without the opt-in.
    let agent = hello(&paths.socket, "mcp").await;
    assert_eq!(agent.call("share.connect", json!({})).await.unwrap_err().code, ErrorCode::PolicyDenied);
    assert_eq!(agent.call("share.upload", json!({ "id": id })).await.unwrap_err().code, ErrorCode::PolicyDenied);

    // Sign in, with the client JSON as Google Cloud downloads it.
    let client_json = home.path().join("client_secret_123.json");
    std::fs::write(&client_json, r#"{"installed":{"client_id":"test-client.apps.googleusercontent.com","client_secret":"test-secret","redirect_uris":["http://localhost"]}}"#).unwrap();
    let watcher = hello(&paths.socket, "cli").await;
    watcher.call("events.subscribe", json!({ "topics": ["share."] })).await.unwrap();
    let mut events = watcher.take_events().unwrap();
    let socket = paths.socket.clone();
    let cj = client_json.to_str().unwrap().to_string();
    let connecting = tokio::spawn(async move {
        let c = hello(&socket, "cli").await;
        c.call("share.connect", json!({ "clientJson": cj, "noBrowser": true, "timeoutMs": 30000 })).await
    });
    let sign_in = loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), events.recv()).await.unwrap().unwrap();
        if ev.kind == "share.signInUrl" {
            break url::Url::parse(ev.data["url"].as_str().unwrap()).unwrap();
        }
    };
    let q: HashMap<String, String> = sign_in.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "test-client.apps.googleusercontent.com");
    assert_eq!(q["scope"], "https://www.googleapis.com/auth/drive.file");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["access_type"], "offline");
    fake.lock().unwrap().challenge = q["code_challenge"].clone();
    let redirect = &q["redirect_uri"];
    assert!(redirect.starts_with("http://127.0.0.1:"));

    // A request with the wrong state is refused and doesn't end the sign-in.
    assert_eq!(visit(&format!("{redirect}/?code=evil&state=wrong")).await, 400);
    assert_eq!(visit(&format!("{redirect}/?code=good-code&state={}", q["state"])).await, 200);
    let st = connecting.await.unwrap().unwrap();
    assert_eq!(st["connected"], true);
    assert_eq!(st["account"], "tester@example.com");

    // Tokens stay out of settings and in a private file.
    let settings = std::fs::read_to_string(&paths.settings_file).unwrap_or_default();
    assert!(!settings.contains("refresh-1") && !settings.contains("test-secret"));
    let token_file = home.path().join("google-drive.json");
    assert!(std::fs::read_to_string(&token_file).unwrap().contains("refresh-1"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!c.call("share.status", json!({})).await.unwrap().to_string().contains("refresh-1"));

    // The access token "expires" before the folder call; the engine refreshes once and carries on.
    fake.lock().unwrap().expire_next = true;
    let up = c.call("share.upload", json!({ "id": id, "wait": true, "timeoutMs": 60000 })).await.unwrap();
    assert_eq!(up["url"], "https://drive.google.com/file/d/file-1/view?usp=drivesdk");
    assert_eq!(up["existing"], false);
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.refreshes, 1, "{:?}", f.log);
        assert!(f.status_queries >= 1, "the 503 should make the client ask where to resume: {:?}", f.log);
        assert!(f.public);
        let (_, uploaded) = f.file.as_ref().unwrap();
        assert!(uploaded == &original, "uploaded bytes differ from the recording ({} vs {})", uploaded.len(), original.len());
    }

    // The link is kept with the recording, shown in the library, and reused next time.
    let bundle = std::path::PathBuf::from(stopped["bundlePath"].as_str().unwrap());
    let m: Manifest = serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m.shares.len(), 1);
    assert_eq!(m.shares[0].file_id, "file-1");
    let list = c.call("library.list", json!({})).await.unwrap();
    assert_eq!(list["items"][0]["shareUrl"], up["url"]);
    let again = c.call("share.upload", json!({ "id": "last" })).await.unwrap();
    assert_eq!(again["existing"], true);
    assert_eq!(again["url"], up["url"]);
    assert_eq!(fake.lock().unwrap().folders, 1);

    // Signing out deletes the token; the next upload asks to connect again.
    let st = c.call("share.disconnect", json!({})).await.unwrap();
    assert_eq!(st["connected"], false);
    assert!(!token_file.exists());
    let e = c.call("share.upload", json!({ "id": id, "force": true })).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthRequired);
}
