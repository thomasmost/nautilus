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
        Some("bundle-mac") => match features(std::env::args().skip(2)) {
            Ok(features) => bundle_mac::bundle_mac(
                &project_root(),
                env!("CARGO_PKG_VERSION"),
                features.as_deref(),
            )
            .map(|app| {
                println!("bundled {}", app.display());
            }),
            Err(e) => Err(e),
        },
        _ => {
            eprintln!("usage: cargo xtask <icons | bundle-mac [--features <list>]>");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// The value of `--features <list>` among a task's arguments, if it is
/// there; any other argument is an error.
fn features(mut args: impl Iterator<Item = String>) -> Result<Option<String>, String> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (None, _, _) => Ok(None),
        (Some("--features"), Some(list), None) => Ok(Some(list)),
        _ => Err("usage: cargo xtask bundle-mac [--features <list>]".to_string()),
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::features;

    fn parsed(args: &[&str]) -> Result<Option<String>, String> {
        features(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn the_bundle_takes_no_argument_or_a_feature_list() {
        assert_eq!(parsed(&[]), Ok(None));
        assert_eq!(
            parsed(&["--features", "test-hooks"]),
            Ok(Some("test-hooks".to_string()))
        );
        assert!(parsed(&["--features"]).is_err());
        assert!(parsed(&["--release"]).is_err());
        assert!(parsed(&["--features", "test-hooks", "more"]).is_err());
    }
}
