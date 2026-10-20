# Upstream issues

Nautilus is a shell around CEF (Chromium), egui, and winit. Some things it
cannot do, or does badly, are upstream limits rather than Nautilus bugs.
This file tracks each one: what it is, which features depend on it, what
Nautilus does in the meantime, and what to do when it lands.

_State recorded as of 2026-10-19, against CEF 154.0.34 (`cef` crate
154.5.0), egui 0.34, winit 0.30._

## How to maintain this file

- Add an entry when a bug or a planned feature turns out to depend on an
  upstream change. Give it the next id in its group (`C` CEF and the `cef`
  crate, `E` egui and winit, `T` toolchain and packaging). Link the
  upstream issue if one exists; say "no upstream issue found" otherwise.
- On every CEF bump, re-check each `C` entry.
- `README.md` lists the user-visible limitations in one sentence each and
  points here for the detail.

## CEF and the `cef` crate

### C1: Sandbox needs CEF's bootstrap executable on Windows

CEF 128+ runs the sandboxed browser process from its own `bootstrap.exe`
(or `bootstrapc.exe`), which loads the application as a DLL exporting
`RunWinMain`; a plain executable cannot enable the sandbox. The `cef`
crate's `bundle-cef-app` tool and `sandbox` feature support that layout.
Nautilus starts with `no_sandbox` and a normal executable. **Blocks:** the
renderer sandbox (ROADMAP Phase 2). **Workaround:** none; site isolation
and separate renderer processes still apply.

### C2: Windowless browsers paint through system memory

The default off-screen path (`on_paint`) hands BGRA buffers that Nautilus
uploads to GL textures every frame. CEF's `shared_texture_enabled` path
(`on_accelerated_paint`) shares a D3D11/Metal texture instead; the `cef`
crate exposes it behind the `accelerated_osr` feature with wgpu. **Blocks:**
smooth video and scrolling at 4K. **Workaround:** the software path.

### C3: macOS needs an app bundle with helper apps

CEF on macOS loads `Chromium Embedded Framework.framework` from the bundle
and runs helper processes from per-kind helper apps; `cargo run` of a bare
binary cannot start. The `cef` crate's `bundle-cef-app` builds such a
bundle for its examples. **Blocks:** `cargo run` on macOS.
**Workaround (2026-10-20):** `cargo xtask bundle-mac` builds the bundle
(`xtask/src/bundle_mac.rs`); developing on macOS means rebuilding the
bundle and running the executable inside it.

## egui and winit

### E1: IME is enabled only for egui's own text fields

egui-winit calls `set_ime_allowed(false)` whenever no egui widget has
keyboard focus, so winit sends no `Ime` events while the page has focus.
**Blocks:** composed input (CJK) in page text fields. **Workaround:** the
`Ime` arm in `BrowserWindow::handle_event` is ready; the engine side
(`ime_set_composition`, `ime_commit_text`) is implemented.

## Toolchain and packaging

### T1: cargo-packager has no macOS helper-app support

cargo-packager builds a single-executable `.app`; CEF's layout (framework
in `Contents/Frameworks`, five helper apps) needs a custom step. **Blocks:**
the macOS `.dmg` through cargo-packager. **Workaround (2026-10-20):** the
bundle is laid out by `cargo xtask bundle-mac` without cargo-packager; a
`.dmg` would be `hdiutil` over that bundle.
