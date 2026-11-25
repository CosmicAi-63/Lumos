//! Repo tasks. `cargo xtask schemas` writes the JSON Schemas that clients and
//! docs read: every tool from the registry, settings.json and manifest.json.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("schemas") => schemas(args.get(1).map(PathBuf::from)),
        _ => {
            eprintln!("usage: cargo xtask schemas [out-dir]   (default: docs/schemas)");
            std::process::exit(2);
        }
    }
}

fn schemas(out: Option<PathBuf>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf();
    let dir = out.unwrap_or_else(|| root.join("docs/schemas"));
    std::fs::create_dir_all(&dir).expect("create schema folder");

    let tools: Vec<_> = lumos_tools::registry().iter().map(|t| t.describe()).collect();
    write(&dir.join("tools.json"), &serde_json::Value::Array(tools));
    write(&dir.join("settings.schema.json"), &lumos_tools::settings_schema());
    write(&dir.join("manifest.schema.json"), &schemars::schema_for!(lumos_model::manifest::Manifest));
    println!("wrote {} tools and 2 schemas to {}", lumos_tools::registry().len(), dir.display());
}

fn write(path: &std::path::Path, v: &impl serde::Serialize) {
    let mut body = serde_json::to_string_pretty(v).expect("serialize");
    body.push('\n');
    std::fs::write(path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
