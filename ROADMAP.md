# Roadmap

Numbered phases. Nautilus starts where Paladin's Phase 2 left off, on
Chromium instead of Servo, and keeps Paladin's model and chrome.

## Phase 1 — Parity with Paladin on Chromium (in progress)

Everything Paladin does, on CEF: windows and tabs with multi-selection and
moves, native menus, profiles, chrome colours, crashed-tab isolation, the
Windows installer. Shipped on Windows on 2026-10-19; macOS needs the app
bundle (framework and helper apps) before it runs there.

Design: `docs/design-plans/2026-10-19-nautilus-chromium-browser.md`.

## Phase 2 — Chromium's strengths

- Renderer sandbox: start through CEF's `bootstrap.exe` on Windows (the
  browser built as a DLL exporting `RunWinMain`), signed helper apps on
  macOS.
- Accelerated off-screen rendering (CEF's shared-texture path, `wgpu` or
  D3D11/Metal interop) instead of copying frames through system memory.
- Downloads, bookmarks, history, find in page, zoom, fullscreen, and
  permissions prompts (camera, notifications), each as commands.
- IME for page text fields.

## Phase 3 — Extensions

Chromium's extension support exists in CEF only in the Chrome runtime
style, which draws Chrome's own UI; Nautilus uses the Alloy style with its
own chrome. The route to extensions is either CEF's Chrome style with
Nautilus's tab strip over it, or a Thorium-based CEF build with the
extension APIs patched into Alloy. Decide after Phase 2.

## Engine notes

Thorium (Alex313031/Thorium) is a Chromium fork with compiler optimisations
and a handful of feature patches; it publishes browser builds, not a CEF
distribution. Using it would mean building CEF from Thorium's tree, a full
Chromium build (hours, 100+ GB). Nautilus tracks stock CEF and revisits
this when a Thorium patch is wanted badly enough to pay for the build.
