# 10. Sharing

**Status: built, tested against a fake Google server.** It hasn't been run against a real Google account yet. The only missing piece is a Google OAuth client, which needs a Google Cloud project (see [Setup](#setup-one-time-per-developer)).

Lumos makes Loom-style links without running a server. Each person connects **their own Google Drive**. When they ask for a link, Lumos uploads that recording into a "Lumos" folder in their Drive and turns on "anyone with the link can view". The link opens in Google's video player, and nobody needs Lumos or a Google account to watch.

- Lumos never hosts anyone's videos and pays for no storage, however many people use it.
- Nothing uploads until someone clicks **Share link** (or runs `lumos share upload`). There is no auto-upload.
- Lumos asks Google only for `drive.file`. With that scope, Lumos can see and change only the files it created, and nothing else in the person's Drive.

## Using it

**In the app**
1. Go to **Settings → Sharing → Connect Google Drive…** The browser opens Google's sign-in, and you click Allow.
2. After a recording, click the link icon in the toolbar's "✓ Saved" state, or **Share link** on any library card. The toolbar shows "Uploading N%", then "✓ Link copied".
3. A recording that already has a link shows **Copy link**, and re-sharing reuses it.

**From the CLI**

```bash
lumos share connect --client-json ~/Downloads/client_secret_XXXX.json   # once; later just `lumos share connect`
lumos record start
lumos record stop
lumos share upload last            # prints https://drive.google.com/file/d/…/view
lumos share status
lumos share disconnect
```

Agents (MCP) can call `share.upload` only after **Settings → Automation → Let AI agents share recordings by link** is turned on. They can never connect an account.

## Setup (one time, per developer)

Google requires every app that signs in with Google to have an OAuth client in a Google Cloud project. It's free.

1. Go to [console.cloud.google.com](https://console.cloud.google.com) and create a project, for example "Lumos".
2. **APIs & Services → Library → Google Drive API → Enable.**
3. **Google Auth Platform → Get started.** Set the app name to Lumos and choose your email as the support email. Choose **External** as the audience, then finish.
4. **Audience → Test users → Add users.** Add every Google account that will connect while the app is in testing.
5. **Clients → Create client.** Choose **Desktop app** as the application type, then **Create**, then **Download JSON**.
6. Give that file to Lumos once: **Settings → Sharing → Choose client file…**, or `lumos share connect --client-json <file>`. Lumos keeps a private copy, so later sign-ins don't need the file.

While the app is in **testing**, Google shows a "Google hasn't verified this app" screen (click **Continue**), allows at most 100 test users, and **expires sign-ins after 7 days**. When a sign-in expires, Lumos asks you to connect again.

### Before other people use it

1. **Publish the app** (Google Auth Platform → Audience → Publish app). `drive.file` is not a sensitive or restricted scope, so Google needs only brand verification: a home page, a privacy policy and a verified domain for those pages. It doesn't need the paid security assessment that full-Drive access requires. After publishing, sign-ins stop expiring.
2. **Build the client into release builds,** so users never see a client file:
   ```bash
   LUMOS_BUILTIN_GOOGLE_CLIENT_ID=… LUMOS_BUILTIN_GOOGLE_CLIENT_SECRET=… RELEASE=1 scripts/make-app.sh
   ```
   Google treats the secret of a Desktop-app client as public (it can't be kept secret inside an app), and PKCE protects the sign-in. Still, keep it out of the repository.

## How it works

```
 Settings → Connect ──▶ share.connect ──▶ browser: accounts.google.com (PKCE, state, drive.file)
                                            │ Allow
                                            ▼
                         http://127.0.0.1:<random port>/?code=…&state=…  (engine's one-shot listener)
                                            │ code + verifier
                                            ▼
                         oauth2.googleapis.com/token ──▶ refresh token → <LUMOS_HOME>/google-drive.json (0600)

 Share link ──▶ share.upload (job) ──▶ find/create "Lumos" folder
                                      ──▶ resumable upload: POST session, PUT 8 MiB chunks (308 = resume here)
                                      ──▶ permission { type: anyone, role: reader }
                                      ──▶ webViewLink saved in manifest.shares[], event share.uploaded
```

- **Sign-in** uses the OAuth loopback flow for desktop apps. The engine listens on `127.0.0.1` on a random port for one redirect and waits up to 5 minutes. A request with the wrong `state` gets a 400 and doesn't end the sign-in. A newer sign-in cancels an older one that is still waiting.
- **Tokens** live in `<LUMOS_HOME>/google-drive.json`, mode 0600, and the client copy in `google-client.json`. They are never in `settings.json`, which agents can read. Access tokens stay in memory and are refreshed when they expire. A 401 from Drive refreshes once and retries. `invalid_grant` from Google (expired or revoked) deletes the token file and returns `AUTH_REQUIRED`.
- **Uploads** are resumable. On a network error, a 5xx or a 429, the engine asks the session how much it has (`Content-Range: bytes */N`) and continues from there, retrying up to 6 times with backoff. Progress is reported as `job.progress`.
- **The folder** is created on first use and again if the user deletes or trashes it.
- **Errors** worth knowing: Drive full (`storageQuotaExceeded`); a Google Workspace admin who blocks public links (the file is uploaded but stays private, and `data.fileId` names it); a deleted OAuth client (`invalid_client`).

## Testing

`crates/lumos-core/tests/share_flow.rs` runs the whole flow against a fake Google server:
- the OAuth loopback, with PKCE verified and a wrong `state` rejected;
- an upload where the server keeps half of the first chunk and then fails with a 503;
- an access token that expires mid-flow;
- the link saved in the manifest and reused;
- disconnect.

The endpoints come from environment variables, which also work for manual runs against a local fake:

| Variable | Default |
|---|---|
| `LUMOS_GOOGLE_AUTH_URL` | `https://accounts.google.com/o/oauth2/v2/auth` |
| `LUMOS_GOOGLE_TOKEN_URL` | `https://oauth2.googleapis.com/token` |
| `LUMOS_GOOGLE_REVOKE_URL` | `https://oauth2.googleapis.com/revoke` |
| `LUMOS_GOOGLE_API_URL` | `https://www.googleapis.com` |
| `LUMOS_GOOGLE_UPLOAD_URL` | `https://www.googleapis.com` |
| `LUMOS_UPLOAD_CHUNK_KB` | `8192` (rounded to a multiple of 256) |
| `LUMOS_GOOGLE_CLIENT_ID` / `LUMOS_GOOGLE_CLIENT_SECRET` | Use this client instead of the saved client file |

## Later

- Keep tokens in the macOS Keychain once builds have a stable Developer ID signature. With ad-hoc signing, each rebuild would make macOS ask for Keychain access again.
- Other providers behind the same `share.*` tools: Dropbox, or a self-hostable Lumos share server with its own viewer page.
- "Stop sharing" (remove the public permission) and "Delete from Drive" from the library.
