//! Where the engine keeps its files. Shared by the engine and every client so
//! they agree on the socket location.
//!
//! Overrides (used by tests and dev runs): `LUMOS_HOME`, `LUMOS_SOCKET`, `LUMOS_TRASH_DIR`.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub settings_file: PathBuf,
    pub socket: PathBuf,
    pub logs_dir: PathBuf,
    /// Where cancelled and deleted recordings go.
    pub trash_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::var_os("LUMOS_HOME").map(PathBuf::from).unwrap_or_else(default_home);
        let socket = std::env::var_os("LUMOS_SOCKET").map(PathBuf::from).unwrap_or_else(|| home.join("engine.sock"));
        let trash_dir = std::env::var_os("LUMOS_TRASH_DIR").map(PathBuf::from).unwrap_or_else(default_trash);
        Self::with(home, socket, trash_dir)
    }

    /// Everything under one folder. Used by tests.
    pub fn under(home: PathBuf) -> Self {
        let socket = home.join("engine.sock");
        let trash = home.join("trash");
        Self::with(home, socket, trash)
    }

    fn with(home: PathBuf, socket: PathBuf, trash_dir: PathBuf) -> Self {
        Self { settings_file: home.join("settings.json"), logs_dir: home.join("logs"), home, socket, trash_dir }
    }
}

fn default_home() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("Lumos")
}

fn default_trash() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") { home.join(".Trash") } else { home.join(".local/share/Trash/files") }
}
