//! Workspace helper tasks, run as `cargo xtask <task>` (alias in
//! `.cargo/config.toml`): `icons` (osmena's icon task; this binary names
//! the workspace root and the icon set) and `bundle-mac` (the macOS
//! application bundle, `bundle_mac.rs`).

mod bundle_mac;

use std::path::{Path, PathBuf};

fn main() {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("icons") => {
            osmena_xtask::icons(&project_root(), "nautilus").map_err(|e| e.to_string())
        }
        Some("bundle-mac") => bundle_mac::bundle_mac(&project_root(), env!("CARGO_PKG_VERSION"))
            .map(|app| {
                println!("bundled {}", app.display());
            }),
        _ => {
            eprintln!("usage: cargo xtask <icons | bundle-mac>");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}
