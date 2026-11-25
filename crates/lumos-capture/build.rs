//! Compiles `swift/helper.swift` into `lumos-capture-helper` on macOS.
//!
//! The path is baked in as `LUMOS_HELPER_BUILD_PATH` for development runs. If
//! swiftc is missing or fails, the build still succeeds with a warning and the
//! macOS backend reports that the helper is unavailable.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=swift/helper.swift");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let out = out_dir.join("lumos-capture-helper");
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("swift/helper.swift");

    let compile = |extra: &[String]| -> Result<(), String> {
        let o = Command::new("xcrun")
            .args(["swiftc", "-O", "-swift-version", "5", "-parse-as-library"])
            .args(extra)
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .output()
            .map_err(|e| format!("cannot run xcrun swiftc: {e}"))?;
        if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).lines().take(6).collect::<Vec<_>>().join(" | ")) }
    };

    let mut result = compile(&[]);
    if result.is_err() {
        // Command Line Tools 16 ships a duplicate SwiftBridging modulemap. Hide one copy for this compile only.
        let empty = out_dir.join("empty.modulemap");
        let overlay = out_dir.join("vfs.yaml");
        let hidden = "/Library/Developer/CommandLineTools/usr/include/swift/module.modulemap";
        let yaml = format!(
            r#"{{ "version": 0, "case-sensitive": "false", "roots": [ {{ "name": "{hidden}", "type": "file", "external-contents": "{}" }} ] }}"#,
            empty.display()
        );
        if std::fs::write(&empty, "").is_ok() && std::fs::write(&overlay, yaml).is_ok() {
            let o = overlay.display().to_string();
            result = compile(&["-vfsoverlay".into(), o.clone(), "-Xcc".into(), "-ivfsoverlay".into(), "-Xcc".into(), o]);
        }
    }
    match result {
        Ok(()) => println!("cargo:rustc-env=LUMOS_HELPER_BUILD_PATH={}", out.display()),
        Err(e) => println!("cargo:warning=could not build lumos-capture-helper: {e}"),
    }
}
