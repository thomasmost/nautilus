//! Nautilus: a desktop browser on Chromium (through CEF) with the osmena
//! shell. This crate is the brand, the engine's process preamble, the
//! command line, and the packaging metadata; the browser itself is
//! `osmena_shell` over `nautilus_engine`.

// Release builds on Windows are GUI-subsystem apps, so the Start menu shortcut
// opens no console window. Debug builds keep the console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod brand;

use std::process::ExitCode;

use clap::Parser;
use osmena_shell::{CliArgs, Launch};

#[derive(Parser, Debug)]
#[command(name = "nautilus", version, about = "Nautilus desktop browser")]
struct Cli {
    #[command(flatten)]
    base: CliArgs,
}

fn main() -> ExitCode {
    // Chromium's helper processes (renderers, GPU, network, utilities) are
    // this executable run again with Chromium's own arguments. They must be
    // dispatched before anything else: before the command line is parsed
    // (clap would reject their arguments), before logging, before windows.
    if let Some(code) = nautilus_engine::run_subprocess() {
        std::process::exit(code);
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    osmena_shell::run(Launch {
        brand: &brand::BRAND,
        engine: &nautilus_engine::INFO,
        args: cli.base,
        start_engine: Box::new(nautilus_engine::Engine::new),
    })
}
