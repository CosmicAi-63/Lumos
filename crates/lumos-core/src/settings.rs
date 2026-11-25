//! `settings.json`: load, read by dotted path, validated patches, atomic save.

use std::path::PathBuf;

use lumos_model::settings::Settings;
use lumos_model::{ToolError, ToolResult};
use serde_json::{Map, Value};

use crate::fsutil::write_json_atomic;

pub struct SettingsStore {
    path: PathBuf,
    current: Settings,
}

impl SettingsStore {
    /// Missing file = defaults. A broken file is an error, never silently replaced.
    pub fn load(path: PathBuf) -> ToolResult<Self> {
        let current = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| ToolError::invalid_params(format!("{} is not valid settings: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(ToolError::backend(format!("read {}: {e}", path.display()))),
        };
        Ok(Self { path, current })
    }

    pub fn get(&self) -> &Settings {
        &self.current
    }

    pub fn get_path(&self, path: Option<&str>) -> ToolResult<Value> {
        let all = to_value(&self.current);
        match path {
            None => Ok(all),
            Some(p) => lookup(&all, p).cloned().ok_or_else(|| ToolError::not_found(format!("no setting '{p}'"))),
        }
    }

    /// Apply dotted-path changes. All or nothing.
    pub fn set(&mut self, patch: &Map<String, Value>) -> ToolResult<Vec<String>> {
        let mut next = to_value(&self.current);
        let mut changed = Vec::new();
        for (path, value) in patch {
            let slot = lookup_mut(&mut next, path).ok_or_else(|| ToolError::invalid_params(format!("unknown setting '{path}'")))?;
            if slot.is_object() && !is_leaf_object(path) {
                return Err(ToolError::invalid_params(format!("'{path}' is a section; set its fields one by one")));
            }
            if slot != value {
                *slot = value.clone();
                changed.push(path.clone());
            }
        }
        let parsed: Settings = serde_json::from_value(next).map_err(|e| ToolError::invalid_params(format!("invalid value: {e}")))?;
        validate(&parsed)?;
        if !changed.is_empty() {
            self.save(&parsed)?;
            self.current = parsed;
        }
        Ok(changed)
    }

    pub fn reset(&mut self, path: Option<&str>) -> ToolResult<Vec<String>> {
        let defaults = to_value(&Settings::default());
        match path {
            None => {
                let changed = diff_paths(&to_value(&self.current), &defaults, "");
                self.save(&Settings::default())?;
                self.current = Settings::default();
                Ok(changed)
            }
            Some(p) => {
                let v = lookup(&defaults, p).cloned().ok_or_else(|| ToolError::not_found(format!("no setting '{p}'")))?;
                let mut patch = Map::new();
                flatten(&v, p, &mut patch);
                self.set(&patch)
            }
        }
    }

    pub fn as_value(&self) -> Value {
        to_value(&self.current)
    }

    fn save(&self, s: &Settings) -> ToolResult<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ToolError::backend(format!("create {}: {e}", dir.display())))?;
        }
        write_json_atomic(&self.path, s)
    }
}

fn validate(s: &Settings) -> ToolResult<()> {
    if ![24, 30, 60].contains(&s.capture.fps) {
        return Err(ToolError::invalid_params("capture.fps must be 24, 30 or 60"));
    }
    if s.recording.countdown_seconds > 10 {
        return Err(ToolError::invalid_params("recording.countdownSeconds must be 0–10"));
    }
    if s.storage.save_folder.trim().is_empty() {
        return Err(ToolError::invalid_params("storage.saveFolder cannot be empty"));
    }
    if !(1..=8).contains(&s.jobs.max_concurrent) {
        return Err(ToolError::invalid_params("jobs.maxConcurrent must be 1–8"));
    }
    let k = &s.storage.keep_source_tracks;
    let ok = k == "always" || k == "never" || k.strip_prefix("days:").and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n > 0);
    if !ok {
        return Err(ToolError::invalid_params("storage.keepSourceTracks must be always, never or days:N"));
    }
    Ok(())
}

fn to_value(s: &Settings) -> Value {
    serde_json::to_value(s).expect("settings serialize")
}

fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, key| cur.as_object()?.get(key))
}

fn lookup_mut<'a>(v: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    path.split('.').try_fold(v, |cur, key| cur.as_object_mut()?.get_mut(key))
}

/// Objects that are values, not sections: they are set as a whole.
fn is_leaf_object(path: &str) -> bool {
    matches!(path, "capture.defaultSource" | "camera.position" | "ui.toolbarPosition")
}

fn flatten(v: &Value, prefix: &str, out: &mut Map<String, Value>) {
    match v.as_object() {
        Some(obj) if !is_leaf_object(prefix) => {
            for (k, child) in obj {
                flatten(child, &format!("{prefix}.{k}"), out);
            }
        }
        _ => {
            out.insert(prefix.to_string(), v.clone());
        }
    }
}

fn diff_paths(a: &Value, b: &Value, prefix: &str) -> Vec<String> {
    match (a.as_object(), b.as_object()) {
        (Some(x), Some(y)) if !is_leaf_object(prefix) => x
            .iter()
            .flat_map(|(k, av)| {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                y.get(k).map(|bv| diff_paths(av, bv, &p)).unwrap_or_default()
            })
            .collect(),
        _ if a != b => vec![prefix.to_string()],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let d = tempfile::tempdir().unwrap();
        let s = SettingsStore::load(d.path().join("settings.json")).unwrap();
        (d, s)
    }

    #[test]
    fn set_get_reset_round_trip() {
        let (d, mut s) = store();
        let patch = json!({ "storage.saveFolder": "/tmp/x", "capture.fps": 60 });
        assert_eq!(s.set(patch.as_object().unwrap()).unwrap().len(), 2);
        assert_eq!(s.get_path(Some("capture.fps")).unwrap(), json!(60));

        let reloaded = SettingsStore::load(d.path().join("settings.json")).unwrap();
        assert_eq!(reloaded.get().storage.save_folder, "/tmp/x");

        assert_eq!(s.reset(Some("capture")).unwrap(), vec!["capture.fps"]);
        assert_eq!(s.reset(None).unwrap(), vec!["storage.saveFolder"]);
    }

    #[test]
    fn rejects_bad_patches_without_saving() {
        let (d, mut s) = store();
        for bad in [json!({ "capture.fps": 25 }), json!({ "nope.x": 1 }), json!({ "capture": {} }), json!({ "capture.fps": "x" })] {
            assert_eq!(s.set(bad.as_object().unwrap()).unwrap_err().code, lumos_model::ErrorCode::InvalidParams, "{bad}");
        }
        assert!(!d.path().join("settings.json").exists());
    }

    #[test]
    fn whole_value_objects_can_be_set() {
        let (_d, mut s) = store();
        let p = json!({ "capture.defaultSource": { "kind": "display", "displayId": "2" } });
        assert_eq!(s.set(p.as_object().unwrap()).unwrap(), vec!["capture.defaultSource"]);
    }
}
