//! `share.*` tools: public links through the user's own Google Drive.
//!
//! Each person connects their own Drive, so Lumos never hosts or pays for anyone's videos.
//! Nothing uploads until someone asks for a link. See docs/10-sharing.md.

pub(crate) mod google;

use std::sync::Arc;
use std::time::Duration;

use lumos_model::api::*;
use lumos_model::events;
use lumos_model::jobs::JobKind;
use lumos_model::manifest::ShareLink;
use lumos_model::{ToolError, ToolResult, now_rfc3339};
use serde_json::json;

use crate::engine::{Inner, MAX_WAIT_MS};
use crate::library::find;

pub(crate) const PROVIDER: &str = "googleDrive";
const DEFAULT_CONNECT_MS: u64 = 5 * 60 * 1000;

fn mime_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("gif") => "image/gif",
        Some("m4a") => "audio/mp4",
        _ => "video/mp4",
    }
}

impl Inner {
    pub(crate) fn share_status(&self) -> ShareStatus {
        let configured = self.drive.client().is_some();
        let stored = self.drive.stored();
        let hint = match (configured, &stored) {
            (_, Some(_)) => None,
            (true, None) => Some("Connect Google Drive to share recordings by link.".to_string()),
            (false, None) => Some("Pick the Google OAuth client file (Desktop app) from Google Cloud to connect. See docs/10-sharing.md.".to_string()),
        };
        ShareStatus {
            provider: PROVIDER.into(),
            configured,
            connected: stored.is_some(),
            account: stored.and_then(|s| s.account),
            folder_name: google::FOLDER_NAME.into(),
            hint,
        }
    }

    pub(crate) async fn share_connect(&self, p: ShareConnectParams) -> ToolResult<ShareStatus> {
        if let Some(path) = &p.client_json {
            self.drive.save_client_json(&crate::fsutil::expand_tilde(path))?;
        }
        let timeout = Duration::from_millis(p.timeout_ms.unwrap_or(DEFAULT_CONNECT_MS).min(MAX_WAIT_MS));
        let stored = self.drive.connect(!p.no_browser, timeout, |url| self.emit(events::SHARE_SIGN_IN_URL, json!({ "url": url }))).await?;
        self.emit(events::SHARE_CONNECTED, json!({ "provider": PROVIDER, "account": stored.account }));
        Ok(self.share_status())
    }

    pub(crate) async fn share_disconnect(&self) -> ShareStatus {
        let was = self.drive.stored().is_some();
        self.drive.disconnect().await;
        if was {
            self.emit(events::SHARE_DISCONNECTED, json!({ "provider": PROVIDER }));
        }
        self.share_status()
    }

    pub(crate) async fn share_upload(self: &Arc<Self>, p: ShareUploadParams) -> ToolResult<ShareUploadResult> {
        let e = find(self.entries(), &p.id)?;
        if !p.force {
            if let Some(s) = e.manifest.shares.iter().rev().find(|s| s.provider == PROVIDER) {
                return Ok(ShareUploadResult { job_id: None, url: Some(s.url.clone()), file_id: Some(s.file_id.clone()), existing: true });
            }
        }
        if self.drive.stored().is_none() {
            return Err(ToolError::auth_required("connect Google Drive first: Lumos → Settings → Sharing, or `lumos share connect`"));
        }
        let src = e.output().filter(|o| o.exists()).ok_or_else(|| ToolError::invalid_state("the recording's video file is missing"))?;
        let id = e.manifest.id.clone();
        let job = self.jobs.create(JobKind::Upload, Some(id.clone()));
        let (inner, job_id) = (self.clone(), job.id.clone());
        tokio::spawn(async move {
            inner.jobs.start(&job_id);
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{id}.mp4"));
            let r = inner.drive.upload(&src, &name, mime_for(&src), |f| inner.jobs.progress(&job_id, f * 0.98)).await;
            let r = match r {
                Ok(up) => inner.record_share(&id, &up).map(|_| json!({ "recordingId": id, "url": up.url, "fileId": up.file_id, "bytes": up.bytes })),
                Err(e) => Err(e),
            };
            if let Ok(v) = &r {
                inner.emit(events::SHARE_UPLOADED, v);
                inner.emit(events::LIBRARY_CHANGED, json!({ "id": id }));
            }
            inner.jobs.finish(&job_id, r);
        });
        let waited = self.wait_result(&job.id, p.wait, p.timeout_ms).await?;
        Ok(ShareUploadResult {
            job_id: Some(job.id),
            url: waited.as_ref().and_then(|v| v["url"].as_str().map(str::to_owned)),
            file_id: waited.as_ref().and_then(|v| v["fileId"].as_str().map(str::to_owned)),
            existing: false,
        })
    }

    /// Save the link in the manifest. Re-reads the recording, which may have been renamed meanwhile.
    fn record_share(&self, id: &str, up: &google::Uploaded) -> ToolResult<()> {
        let mut e = find(self.entries(), id).map_err(|_| {
            ToolError::backend(format!("uploaded ({}), but the recording was removed before the link could be saved", up.url))
        })?;
        e.manifest.shares.push(ShareLink {
            provider: PROVIDER.into(),
            file_id: up.file_id.clone(),
            url: up.url.clone(),
            created_at: now_rfc3339(),
            bytes: up.bytes,
        });
        e.save()
    }
}
