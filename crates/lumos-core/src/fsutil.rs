use std::path::{Path, PathBuf};

use lumos_model::{ErrorCode, ToolError, ToolResult};
use serde::Serialize;

/// `~` and `~/x` → the home folder.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    PathBuf::from(p)
}

/// Write JSON via a temp file and rename, so readers never see half a file.
pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> ToolResult<()> {
    let body = serde_json::to_vec_pretty(value).map_err(|e| ToolError::backend(format!("serialize: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| io_err("write", &tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_err("rename", path, e))
}

pub fn io_err(what: &str, path: &Path, e: std::io::Error) -> ToolError {
    if e.raw_os_error() == Some(libc::ENOSPC) {
        return ToolError::new(ErrorCode::DiskLow, format!("disk full while trying to {what} {}", path.display()));
    }
    ToolError::backend(format!("{what} {}: {e}", path.display()))
}

/// Free space on the volume holding `path`, in MB.
pub fn free_mb(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized out-param.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64) / (1024 * 1024))
}

/// True if a file can be created in `dir`.
pub fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".lumos-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Characters that are unsafe in file names on any OS we target.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_string();
    if trimmed.is_empty() { "Recording".into() } else { trimmed }
}

/// `{title}` and `{date:<strftime>}` placeholders.
pub fn render_file_name(template: &str, title: &str, at: chrono::DateTime<chrono::Local>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let token = &rest[open + 1..open + close];
        match token.split_once(':') {
            _ if token == "title" => out.push_str(title),
            Some(("date", fmt)) => {
                use std::fmt::Write;
                // An invalid strftime string would panic inside Display; fall back to a fixed format.
                let items: Vec<_> = chrono::format::StrftimeItems::new(fmt).collect();
                if items.iter().any(|i| matches!(i, chrono::format::Item::Error)) {
                    out.push_str(&at.format("%Y-%m-%d %H.%M").to_string());
                } else {
                    let _ = write!(out, "{}", at.format_with_items(items.into_iter()));
                }
            }
            _ => out.push_str(&rest[open..open + close + 1]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    sanitize_file_name(&out)
}

/// `base`, or `base 2`, `base 3`… so that no `base.<ext>` exists for any ext.
pub fn unique_stem(dir: &Path, base: &str, exts: &[&str]) -> String {
    let free = |stem: &str| exts.iter().all(|e| !dir.join(format!("{stem}.{e}")).exists());
    if free(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} {n}")).find(|s| free(s)).expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn renders_template() {
        let at = chrono::Local.with_ymd_and_hms(2026, 11, 25, 14, 3, 0).unwrap();
        assert_eq!(render_file_name("{title} — {date:%Y-%m-%d %H.%M}", "Demo", at), "Demo — 2026-11-25 14.03");
        assert_eq!(render_file_name("{title}/{nope}", "a:b", at), "a-b-{nope}");
        assert_eq!(render_file_name("{date:%Q}", "x", at), "2026-11-25 14.03");
    }

    #[test]
    fn unique_names() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("A.mp4"), b"").unwrap();
        std::fs::create_dir(d.path().join("A 2.lumos")).unwrap();
        assert_eq!(unique_stem(d.path(), "A", &["mp4", "lumos"]), "A 3");
        assert_eq!(unique_stem(d.path(), "B", &["mp4", "lumos"]), "B");
    }
}
