//! Google Drive: browser sign-in (OAuth 2.0 loopback + PKCE) and resumable uploads.
//!
//! Only the `drive.file` scope is requested, so Lumos sees only the files it creates.
//! Tokens live in `<home>/google-drive.json` (mode 0600), never in settings.json,
//! because agents can read settings. See docs/10-sharing.md.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use lumos_model::{ErrorCode, ToolError, ToolResult};
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
pub const FOLDER_NAME: &str = "Lumos";
const CLIENT_FILE: &str = "google-client.json";
const TOKEN_FILE: &str = "google-drive.json";
/// Upload chunks must be a multiple of 256 KiB.
const CHUNK_UNIT: usize = 256 * 1024;
const DEFAULT_CHUNK: usize = 32 * CHUNK_UNIT; // 8 MiB
const MAX_RETRIES: u32 = 6;

/// Google's endpoints. Each can be pointed elsewhere with an env var (tests use a fake server).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    pub revoke: String,
    /// `https://www.googleapis.com`: Drive metadata calls go to `<api>/drive/v3/...`.
    pub api: String,
    /// Uploads go to `<upload>/upload/drive/v3/files`.
    pub upload: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let var = |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
        Self {
            auth: var("LUMOS_GOOGLE_AUTH_URL", "https://accounts.google.com/o/oauth2/v2/auth"),
            token: var("LUMOS_GOOGLE_TOKEN_URL", "https://oauth2.googleapis.com/token"),
            revoke: var("LUMOS_GOOGLE_REVOKE_URL", "https://oauth2.googleapis.com/revoke"),
            api: var("LUMOS_GOOGLE_API_URL", "https://www.googleapis.com").trim_end_matches('/').to_string(),
            upload: var("LUMOS_GOOGLE_UPLOAD_URL", "https://www.googleapis.com").trim_end_matches('/').to_string(),
        }
    }
}

/// The OAuth client Lumos signs in as. For a "Desktop app" client Google treats the secret as public.
#[derive(Debug, Clone)]
pub struct ClientCreds {
    pub client_id: String,
    pub client_secret: Option<String>,
}

/// Accepts the JSON Google Cloud downloads (`{"installed": {...}}`) or a flat `{client_id, client_secret}`.
pub fn parse_client_json(body: &[u8]) -> ToolResult<ClientCreds> {
    let v: Value = serde_json::from_slice(body).map_err(|e| ToolError::invalid_params(format!("not a Google client JSON file: {e}")))?;
    if v.get("web").is_some() {
        return Err(ToolError::invalid_params(
            "this is a \"Web application\" client; create an OAuth client of type \"Desktop app\" instead",
        ));
    }
    let o = v.get("installed").unwrap_or(&v);
    let client_id = o["client_id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| ToolError::invalid_params("the client JSON has no client_id"))?;
    Ok(ClientCreds { client_id: client_id.to_string(), client_secret: o["client_secret"].as_str().map(str::to_owned) })
}

/// What `connect` saves. Mode 0600.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub client_id: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Uploaded {
    pub file_id: String,
    pub url: String,
    pub bytes: u64,
}

pub struct Drive {
    http: reqwest::Client,
    pub ep: Endpoints,
    home: PathBuf,
    chunk: usize,
    access: tokio::sync::Mutex<Option<(String, Instant)>>,
    /// Ends the sign-in that is waiting, when a new one starts.
    pending: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

fn http_err(what: &str, e: reqwest::Error) -> ToolError {
    ToolError::backend(format!("{what}: couldn't reach Google ({e})"))
}

/// A readable error from a Google JSON error body.
async fn google_err(what: &str, resp: reqwest::Response) -> ToolError {
    let status = resp.status();
    let body: Value = body_json(resp).await;
    let message = body["error"]["message"].as_str().or(body["error_description"].as_str()).or(body["error"].as_str()).unwrap_or("").to_string();
    let reason = body["error"]["errors"][0]["reason"].as_str().unwrap_or("");
    let text = match (status.as_u16(), reason) {
        (403, "storageQuotaExceeded") => "your Google Drive is full".to_string(),
        (401, _) => return ToolError::auth_required(format!("{what}: Google sign-in is no longer valid; reconnect Google Drive")),
        _ if message.is_empty() => format!("HTTP {status}"),
        _ => message,
    };
    ToolError::backend(format!("{what}: {text}")).with_data(json!({ "httpStatus": status.as_u16(), "reason": reason }))
}

/// The response body as JSON, or `null`.
async fn body_json(resp: reqwest::Response) -> Value {
    let bytes = resp.bytes().await.unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// `application/x-www-form-urlencoded`.
fn form(pairs: &[(&str, &str)]) -> String {
    let mut u = Url::parse("http://x/").expect("static url");
    u.query_pairs_mut().extend_pairs(pairs);
    u.query().unwrap_or("").to_string()
}

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("system random source");
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(unix)]
fn write_private(path: &Path, body: &[u8]) -> ToolResult<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| ToolError::backend(format!("write {}: {e}", tmp.display())))?;
    std::io::Write::write_all(&mut f, body).map_err(|e| ToolError::backend(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| ToolError::backend(format!("rename {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &[u8]) -> ToolResult<()> {
    std::fs::write(path, body).map_err(|e| ToolError::backend(format!("write {}: {e}", path.display())))
}

fn open_browser(url: &str) {
    let r = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = r {
        tracing::warn!("couldn't open the browser: {e}");
    }
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Lumos</title><body style=\"font:16px -apple-system,sans-serif;padding:48px;text-align:center\"><h2>Lumos is connected to Google Drive</h2><p>You can close this tab.</p></body>";
const PAGE_DENIED: &str = "<!doctype html><meta charset=utf-8><title>Lumos</title><body style=\"font:16px -apple-system,sans-serif;padding:48px;text-align:center\"><h2>Google Drive was not connected</h2><p>You can close this tab and try again from Lumos.</p></body>";
const PAGE_BAD: &str = "<!doctype html><meta charset=utf-8><title>Lumos</title><body>This sign-in link is not the one Lumos is waiting for.</body>";

async fn respond(sock: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.shutdown().await;
}

/// Wait on the loopback listener for Google's redirect and return the code.
/// Requests with the wrong `state` get a 400 and are ignored, so a stray request can't end the sign-in.
async fn wait_for_code(listener: TcpListener, state: &str) -> ToolResult<String> {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else { continue };
        let mut buf = Vec::with_capacity(2048);
        let mut chunk = [0u8; 1024];
        let read = tokio::time::timeout(Duration::from_secs(5), async {
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
        })
        .await;
        if read.is_err() {
            continue;
        }
        let head = String::from_utf8_lossy(&buf);
        let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(&mut sock, "400 Bad Request", PAGE_BAD).await;
            continue;
        };
        let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        if q("state").as_deref() != Some(state) {
            respond(&mut sock, if url.query().is_none() { "404 Not Found" } else { "400 Bad Request" }, PAGE_BAD).await;
            continue;
        }
        if let Some(err) = q("error") {
            respond(&mut sock, "200 OK", PAGE_DENIED).await;
            return Err(ToolError::new(ErrorCode::Cancelled, format!("Google sign-in was not completed ({err})")));
        }
        match q("code") {
            Some(code) => {
                respond(&mut sock, "200 OK", PAGE_OK).await;
                return Ok(code);
            }
            None => respond(&mut sock, "400 Bad Request", PAGE_BAD).await,
        }
    }
}

impl Drive {
    pub fn new(home: PathBuf) -> Self {
        let chunk = std::env::var("LUMOS_UPLOAD_CHUNK_KB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|kb| (kb * 1024).max(CHUNK_UNIT) / CHUNK_UNIT * CHUNK_UNIT)
            .unwrap_or(DEFAULT_CHUNK);
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(120))
            .user_agent(concat!("Lumos/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Self { http, ep: Endpoints::from_env(), home, chunk, access: Default::default(), pending: Default::default() }
    }

    fn client_path(&self) -> PathBuf {
        self.home.join(CLIENT_FILE)
    }
    fn token_path(&self) -> PathBuf {
        self.home.join(TOKEN_FILE)
    }

    /// The OAuth client: `LUMOS_GOOGLE_CLIENT_ID`, then the saved client JSON, then one built in at compile time.
    pub fn client(&self) -> Option<ClientCreds> {
        if let Ok(id) = std::env::var("LUMOS_GOOGLE_CLIENT_ID").map(|s| s.trim().to_string()) {
            if !id.is_empty() {
                return Some(ClientCreds { client_id: id, client_secret: std::env::var("LUMOS_GOOGLE_CLIENT_SECRET").ok() });
            }
        }
        if let Ok(body) = std::fs::read(self.client_path()) {
            match parse_client_json(&body) {
                Ok(c) => return Some(c),
                Err(e) => tracing::warn!("ignoring {}: {}", self.client_path().display(), e.message),
            }
        }
        option_env!("LUMOS_BUILTIN_GOOGLE_CLIENT_ID")
            .filter(|s| !s.is_empty())
            .map(|id| ClientCreds { client_id: id.into(), client_secret: option_env!("LUMOS_BUILTIN_GOOGLE_CLIENT_SECRET").map(str::to_owned) })
    }

    /// Validate a downloaded client JSON and keep a private copy, so later sign-ins don't need the file.
    pub fn save_client_json(&self, path: &Path) -> ToolResult<ClientCreds> {
        let body = std::fs::read(path).map_err(|e| ToolError::invalid_params(format!("can't read {}: {e}", path.display())))?;
        let creds = parse_client_json(&body)?;
        std::fs::create_dir_all(&self.home).map_err(|e| ToolError::backend(format!("create {}: {e}", self.home.display())))?;
        write_private(&self.client_path(), &body)?;
        Ok(creds)
    }

    pub fn stored(&self) -> Option<Stored> {
        let body = std::fs::read(self.token_path()).ok()?;
        serde_json::from_slice(&body).ok()
    }

    fn save_stored(&self, s: &Stored) -> ToolResult<()> {
        std::fs::create_dir_all(&self.home).map_err(|e| ToolError::backend(format!("create {}: {e}", self.home.display())))?;
        let body = serde_json::to_vec_pretty(s).map_err(|e| ToolError::backend(e.to_string()))?;
        write_private(&self.token_path(), &body)
    }

    async fn forget(&self) {
        let _ = std::fs::remove_file(self.token_path());
        *self.access.lock().await = None;
    }

    /// Run the browser sign-in. `on_url` gets the link to open (also opened here unless `open` is false).
    pub async fn connect(&self, open: bool, timeout: Duration, on_url: impl FnOnce(&str)) -> ToolResult<Stored> {
        let client = self.client().ok_or_else(not_configured)?;
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| ToolError::backend(format!("can't listen for the sign-in: {e}")))?;
        let port = listener.local_addr().map_err(|e| ToolError::backend(e.to_string()))?.port();
        let redirect = format!("http://127.0.0.1:{port}");
        let verifier = random_b64(48);
        let state = random_b64(24);
        let url = Url::parse_with_params(
            &self.ep.auth,
            &[
                ("client_id", client.client_id.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("response_type", "code"),
                ("scope", SCOPE),
                ("code_challenge", pkce_challenge(&verifier).as_str()),
                ("code_challenge_method", "S256"),
                ("state", state.as_str()),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ],
        )
        .map_err(|e| ToolError::backend(format!("bad auth URL: {e}")))?;

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        if let Some(old) = self.pending.lock().unwrap().replace(cancel_tx) {
            let _ = old.send(());
        }
        on_url(url.as_str());
        if open {
            open_browser(url.as_str());
        }
        let code = tokio::select! {
            r = tokio::time::timeout(timeout, wait_for_code(listener, &state)) => match r {
                Ok(r) => r?,
                Err(_) => return Err(ToolError::new(ErrorCode::Timeout, "Google sign-in timed out; try again")),
            },
            _ = cancel_rx => return Err(ToolError::new(ErrorCode::Cancelled, "a newer sign-in replaced this one")),
        };

        let mut pairs = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", client.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ];
        if let Some(secret) = &client.client_secret {
            pairs.push(("client_secret", secret));
        }
        let resp = self
            .http
            .post(&self.ep.token)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form(&pairs))
            .send()
            .await
            .map_err(|e| http_err("sign-in", e))?;
        if !resp.status().is_success() {
            return Err(google_err("sign-in", resp).await);
        }
        let tok = body_json(resp).await;
        let access = tok["access_token"].as_str().ok_or_else(|| ToolError::backend("sign-in: Google sent no access token"))?.to_string();
        let refresh = tok["refresh_token"]
            .as_str()
            .ok_or_else(|| ToolError::backend("sign-in: Google sent no refresh token; remove Lumos at myaccount.google.com/permissions and connect again"))?
            .to_string();
        let expires = tok["expires_in"].as_u64().unwrap_or(3600);
        *self.access.lock().await = Some((access, Instant::now() + Duration::from_secs(expires)));

        // Keep the Lumos folder from an earlier connection to the same account, if any.
        let mut stored = Stored { client_id: client.client_id, refresh_token: refresh, account: None, folder_id: None };
        self.save_stored(&stored)?;
        stored.account = self.account().await;
        self.save_stored(&stored)?;
        Ok(stored)
    }

    /// Sign out: revoke the token at Google (best effort) and delete it here.
    pub async fn disconnect(&self) {
        if let Some(s) = self.stored() {
            let r = self
                .http
                .post(&self.ep.revoke)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(form(&[("token", s.refresh_token.as_str())]))
                .send()
                .await;
            if let Err(e) = r {
                tracing::warn!("couldn't revoke the Google token: {e}");
            }
        }
        self.forget().await;
    }

    async fn access_token(&self, refresh: bool) -> ToolResult<String> {
        let mut cache = self.access.lock().await;
        if !refresh {
            if let Some((t, exp)) = cache.as_ref() {
                if *exp > Instant::now() + Duration::from_secs(60) {
                    return Ok(t.clone());
                }
            }
        }
        let stored = self.stored().ok_or_else(not_connected)?;
        let client = self.client().ok_or_else(not_configured)?;
        if client.client_id != stored.client_id {
            return Err(ToolError::auth_required("Lumos's Google client changed since you connected; reconnect Google Drive"));
        }
        let mut pairs = vec![("grant_type", "refresh_token"), ("refresh_token", stored.refresh_token.as_str()), ("client_id", client.client_id.as_str())];
        if let Some(secret) = &client.client_secret {
            pairs.push(("client_secret", secret));
        }
        let resp = self
            .http
            .post(&self.ep.token)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form(&pairs))
            .send()
            .await
            .map_err(|e| http_err("Google sign-in", e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = body_json(resp).await;
            if body["error"] == "invalid_client" {
                // The client JSON is wrong or was deleted in Google Cloud; signing in again won't help.
                return Err(ToolError::invalid_state(
                    "Google rejected Lumos's OAuth client; check the client JSON from Google Cloud, then connect Google Drive again",
                ));
            }
            if body["error"] == "invalid_grant" || status == StatusCode::UNAUTHORIZED {
                // Revoked, or expired (apps in Google's "testing" mode get 7-day sign-ins).
                drop(cache);
                self.forget().await;
                return Err(ToolError::auth_required("your Google Drive sign-in expired or was revoked; connect Google Drive again"));
            }
            return Err(ToolError::backend(format!("Google sign-in: {}", body["error_description"].as_str().unwrap_or("refresh failed"))));
        }
        let tok = body_json(resp).await;
        let access = tok["access_token"].as_str().ok_or_else(|| ToolError::backend("Google sent no access token"))?.to_string();
        let expires = tok["expires_in"].as_u64().unwrap_or(3600);
        *cache = Some((access.clone(), Instant::now() + Duration::from_secs(expires)));
        Ok(access)
    }

    /// Send an authorized request; on 401 refresh the token once and retry.
    async fn send(&self, make: impl Fn(&str) -> reqwest::RequestBuilder) -> ToolResult<reqwest::Response> {
        let token = self.access_token(false).await?;
        let resp = make(&token).send().await.map_err(|e| http_err("Google Drive", e))?;
        if resp.status() != StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let token = self.access_token(true).await?;
        make(&token).send().await.map_err(|e| http_err("Google Drive", e))
    }

    /// The signed-in account's email, if Drive reports it.
    async fn account(&self) -> Option<String> {
        let url = format!("{}/drive/v3/about?fields=user(emailAddress,displayName)", self.ep.api);
        let resp = self.send(|t| self.http.get(&url).bearer_auth(t)).await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v = body_json(resp).await;
        v["user"]["emailAddress"].as_str().or(v["user"]["displayName"].as_str()).map(str::to_owned)
    }

    /// The "Lumos" folder in Drive, created on first use and again if the user deletes it.
    async fn folder(&self) -> ToolResult<String> {
        let mut stored = self.stored().ok_or_else(not_connected)?;
        if let Some(id) = &stored.folder_id {
            let url = format!("{}/drive/v3/files/{id}?fields=id,trashed", self.ep.api);
            let resp = self.send(|t| self.http.get(&url).bearer_auth(t)).await?;
            if resp.status().is_success() {
                if body_json(resp).await["trashed"] != Value::Bool(true) {
                    return Ok(id.clone());
                }
            } else if resp.status() != StatusCode::NOT_FOUND {
                return Err(google_err("find the Lumos folder", resp).await);
            }
        }
        let url = format!("{}/drive/v3/files?fields=id", self.ep.api);
        let body = serde_json::to_vec(&json!({ "name": FOLDER_NAME, "mimeType": "application/vnd.google-apps.folder" })).unwrap_or_default();
        let resp = self.send(|t| self.http.post(&url).bearer_auth(t).header(CONTENT_TYPE, "application/json; charset=UTF-8").body(body.clone())).await?;
        if !resp.status().is_success() {
            return Err(google_err("create the Lumos folder in Drive", resp).await);
        }
        let id = body_json(resp).await["id"].as_str().ok_or_else(|| ToolError::backend("Drive sent no folder id"))?.to_string();
        // Re-read so a concurrent change (another upload) isn't lost.
        if let Some(latest) = self.stored() {
            stored = latest;
        }
        stored.folder_id = Some(id.clone());
        self.save_stored(&stored)?;
        Ok(id)
    }

    /// Ask the upload session how many bytes it has. `Ok(None)` means the upload already finished.
    async fn uploaded_so_far(&self, session: &str, total: u64) -> Result<Option<(u64, Option<Value>)>, reqwest::Error> {
        let resp = self.http.put(session).header(CONTENT_RANGE, format!("bytes */{total}")).header(CONTENT_LENGTH, 0).body(Vec::new()).send().await?;
        match resp.status().as_u16() {
            200 | 201 => Ok(Some((total, Some(body_json(resp).await)))),
            308 => Ok(Some((next_offset(&resp), None))),
            _ => Ok(None),
        }
    }

    /// Upload `file` into the Lumos folder, make it viewable by anyone with the link, and return the link.
    pub async fn upload(&self, file: &Path, name: &str, mime: &str, progress: impl Fn(f64)) -> ToolResult<Uploaded> {
        let total = tokio::fs::metadata(file).await.map_err(|e| ToolError::invalid_state(format!("can't read {}: {e}", file.display())))?.len();
        if total == 0 {
            return Err(ToolError::invalid_state("the recording's file is empty"));
        }
        let folder = self.folder().await?;
        let start_url = format!("{}/upload/drive/v3/files?uploadType=resumable&fields=id,webViewLink", self.ep.upload);
        let meta = serde_json::to_vec(&json!({ "name": name, "parents": [folder], "mimeType": mime })).unwrap_or_default();
        let resp = self
            .send(|t| {
                self.http
                    .post(&start_url)
                    .bearer_auth(t)
                    .header(CONTENT_TYPE, "application/json; charset=UTF-8")
                    .header("X-Upload-Content-Type", mime)
                    .header("X-Upload-Content-Length", total)
                    .body(meta.clone())
            })
            .await?;
        if !resp.status().is_success() {
            return Err(google_err("start the upload", resp).await);
        }
        let session = resp
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ToolError::backend("Drive didn't return an upload session"))?
            .to_string();

        let mut f = tokio::fs::File::open(file).await.map_err(|e| ToolError::backend(format!("open {}: {e}", file.display())))?;
        let mut offset = 0u64;
        let mut failures = 0u32;
        let mut buf = vec![0u8; self.chunk];
        let done: Value = loop {
            progress(offset as f64 / total as f64);
            let len = (total - offset).min(self.chunk as u64) as usize;
            f.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| ToolError::backend(format!("read {}: {e}", file.display())))?;
            f.read_exact(&mut buf[..len]).await.map_err(|e| ToolError::backend(format!("read {}: {e}", file.display())))?;
            let sent = self
                .http
                .put(&session)
                .header(CONTENT_RANGE, format!("bytes {}-{}/{total}", offset, offset + len as u64 - 1))
                .header(CONTENT_LENGTH, len)
                .body(buf[..len].to_vec())
                .send()
                .await;
            let retry = match sent {
                Ok(r) if r.status().is_success() => break body_json(r).await,
                Ok(r) if r.status().as_u16() == 308 => {
                    offset = next_offset(&r);
                    failures = 0;
                    continue;
                }
                Ok(r) if r.status() == StatusCode::NOT_FOUND || r.status() == StatusCode::GONE => {
                    return Err(ToolError::backend("the Drive upload session expired; try sharing again"));
                }
                Ok(r) if r.status().is_server_error() || r.status() == StatusCode::TOO_MANY_REQUESTS => format!("HTTP {}", r.status()),
                Ok(r) => return Err(google_err("upload", r).await),
                Err(e) => e.to_string(),
            };
            failures += 1;
            if failures > MAX_RETRIES {
                return Err(ToolError::backend(format!("upload to Drive failed after {MAX_RETRIES} retries ({retry})")));
            }
            tracing::warn!("upload chunk failed ({retry}); retry {failures}");
            tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(failures.min(5)))).await;
            match self.uploaded_so_far(&session, total).await {
                Ok(Some((_, Some(v)))) => break v,
                Ok(Some((n, None))) => offset = n,
                Ok(None) => return Err(ToolError::backend("the Drive upload session ended; try sharing again")),
                Err(_) => {} // still offline: resend from the same offset
            }
        };
        progress(1.0);

        let file_id = done["id"].as_str().ok_or_else(|| ToolError::backend("Drive didn't return the file id"))?.to_string();
        let perm_url = format!("{}/drive/v3/files/{file_id}/permissions?fields=id", self.ep.api);
        let perm = serde_json::to_vec(&json!({ "type": "anyone", "role": "reader" })).unwrap_or_default();
        let resp = self.send(|t| self.http.post(&perm_url).bearer_auth(t).header(CONTENT_TYPE, "application/json; charset=UTF-8").body(perm.clone())).await?;
        if !resp.status().is_success() {
            let e = google_err("uploaded, but couldn't turn on the public link (a Google Workspace admin may block public sharing)", resp).await;
            return Err(ToolError { data: Some(json!({ "fileId": file_id })), ..e });
        }
        let url = done["webViewLink"].as_str().map(str::to_owned).unwrap_or_else(|| format!("https://drive.google.com/file/d/{file_id}/view"));
        Ok(Uploaded { file_id, url, bytes: total })
    }
}

/// After a 308, the next byte to send: one past the end of `Range: bytes=0-N`, or 0 when absent.
fn next_offset(resp: &reqwest::Response) -> u64 {
    resp.headers()
        .get(RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.rsplit('-').next())
        .and_then(|n| n.trim().parse::<u64>().ok())
        .map(|end| end + 1)
        .unwrap_or(0)
}

fn not_configured() -> ToolError {
    ToolError::invalid_state(
        "Google Drive isn't set up in this build of Lumos: pass the OAuth client JSON from Google Cloud (`lumos share connect --client-json <file>`). See docs/10-sharing.md",
    )
}

fn not_connected() -> ToolError {
    ToolError::auth_required("connect Google Drive first: Lumos → Settings → Sharing, or `lumos share connect`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_download_and_rejects_web_clients() {
        let c = parse_client_json(br#"{"installed":{"client_id":"abc.apps.googleusercontent.com","client_secret":"s","redirect_uris":["http://localhost"]}}"#).unwrap();
        assert_eq!(c.client_id, "abc.apps.googleusercontent.com");
        assert_eq!(c.client_secret.as_deref(), Some("s"));
        assert!(parse_client_json(br#"{"web":{"client_id":"x"}}"#).is_err());
        assert!(parse_client_json(b"not json").is_err());
        assert_eq!(parse_client_json(br#"{"client_id":"flat"}"#).unwrap().client_id, "flat");
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn form_encodes() {
        assert_eq!(form(&[("a", "x y"), ("b", "1&2")]), "a=x+y&b=1%262");
    }
}
