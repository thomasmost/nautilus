//! Workspace helper tasks, run as `cargo xtask <task>` (alias in
//! `.cargo/config.toml`). The tasks themselves are osmena's; this binary
//! names the workspace root and the icon set.

use std::path::{Path, PathBuf};

fn main() {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("icons") => osmena_xtask::icons(&project_root(), "nautilus"),
        _ => {
            eprintln!("usage: cargo xtask icons");
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
