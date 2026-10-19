//! `nautilus-helper`: the executable Chromium's helper processes run as on
//! macOS, where the browser's own executable may not serve as the renderer
//! (the app bundle holds one helper app per process kind, each a copy of
//! this binary; see README.md, Packaging). It only hands the process to
//! Chromium. On Windows the browser executable itself plays this role and
//! this binary is unused.

fn main() {
    std::process::exit(nautilus_engine::run_helper());
}
