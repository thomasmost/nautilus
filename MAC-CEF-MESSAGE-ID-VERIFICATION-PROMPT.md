# Prompt: verify CEF's DevTools message ids, then simplify the relay

Give this file to a session on the Mac, in the `nautilus` checkout. It
is a deferred item from the bridge cleanup (PR #2, branch
`bridge-cleanup`, 2026-11-19). It was deferred because the only
end-to-end test of the code it changes is the scripted harness under
`ci/side-panel/`, which runs on macOS only. Delete this file in the
commit that settles the question, either way.

## The question

Can the relay choose its own DevTools message ids, as the rest of the
engine already does?

When the engine evaluates a script it calls
`BrowserHost::execute_dev_tools_method(message_id, ...)`
(`devtools::evaluate`, `crates/nautilus-engine/src/devtools.rs`). CEF
may deliver the answer (`on_dev_tools_method_result`) *inside* that
call, before it has returned. There are two ways to cope, and the
engine uses both:

- **Own id, recorded first.** The extension manager
  (`ExtensionManager::run` in `extensions.rs`, `next_message_id`) and
  the screenshot code (`Hub::capture` in `bridge/hub.rs`, ids from
  `CAPTURE_IDS`) pick the id themselves and record the pending entry
  before the call. An answer that arrives inside the call finds its
  entry. No extra machinery.
- **Id 0, matched afterwards.** The relay passes 0 and lets CEF pick
  (`HiddenPage::evaluate` in `bridge/hidden_page.rs`), so it cannot
  record the entry under its id beforehand. It therefore carries:
  - `Sending`, `Bridge::reserve` and `Bridge::settle` in
    `bridge/relay.rs`: a reservation made before the call and turned
    into a pending entry after it;
  - in `Bridge::page_result`, a fallback that hands an unknown id to
    the most recent open reservation for the same page (the
    `.rev().find(...)` over `inner.sending`). This is a guess: it is
    right only while calls on one page do not nest;
  - the `sending` half of `drop_pending`;
  - for each tab's `Target.getTargetInfo`, `Probe` with `RESERVED`,
    `NOT_SENT` and its `early` buffer, in `Bridge::track` and
    `TabProbe`.

If own ids are reliable, the second approach and all of that machinery
(roughly 100 lines, the subtlest in the file) can go.

## The assumption to verify

CEF keeps a caller's `message_id` when it is greater than the last id
used on that browser, and replaces it only when it is not.

Evidence so far, none of it conclusive:

- The manager and the screenshot code depend on it today and work.
- CEF's header (`include/cef_browser.h`, `ExecuteDevToolsMethod`) says
  the id is "an incremental number that uniquely identifies the
  message (pass 0 to have the next number assigned automatically based
  on previous values)" and that the call returns the assigned id.
- A comment in `HiddenPage::evaluate` says "a caller's own id may be
  raised, so the returned one is what counts". Whoever wrote it may
  have seen CEF replace an id. Find out when.

## What to do

1. **Read CEF's source for the pinned version** (the `cef` entry in
   `Cargo.lock` gives it; 154.0.34 when this was written). The rule is
   in the DevTools controller: look for `ExecuteDevToolsMethod` in
   `libcef/browser/devtools/devtools_controller.cc`. Write down the
   exact rule for a caller's id: when it is kept, when it is replaced,
   and whether the counter is per browser. Note also who else sends on
   a browser's DevTools channel (the hidden pages are the bridge's
   own; a tab's browser also carries the hub's screenshot ids and the
   tab probe).

2. **Decide.** If a caller's id above the browser's last is always
   kept, go on. If not, stop: record the rule in
   `docs/upstream-issues.md`, replace the comment in
   `HiddenPage::evaluate` with what you found, delete this file, and
   leave the machinery.

3. **Make the change**, smallest first:
   - Give `Bridge` one increasing counter for message ids (one counter
     for every hidden page satisfies "increasing per browser").
   - In `Bridge::evaluate`, take an id, insert the pending entry under
     `(key, id)`, then call `devtools::evaluate(&host, id, expression)`.
     If the call is not sent, remove the entry. If it returns a
     different id (CEF replaced it), move the entry to the returned id
     and log a warning: that should never happen, and the log must say
     so if it does.
   - Remove `Sending`, `reserve`, `settle`, the reservation fallback in
     `page_result`, and the `sending` part of `drop_pending`.
   - For the tab probe, give `Target.getTargetInfo` an id from a range
     the hub's screenshots do not use (they start at `CAPTURE_IDS`),
     record it before the call, and remove `RESERVED`, `NOT_SENT` and
     `Probe::early`. Mind that a tab's browser shares one id sequence
     between the probe and the screenshots: the probe is sent once, at
     the tab's creation, before any screenshot.
   - Keep every log line's words: `ci/side-panel/driver.py` reads them.

4. **Prove it.**
   - `cargo fmt --all --check`; `cargo clippy --workspace --all-targets
     -- -D warnings` and `cargo test --workspace`, each also with
     `--all-features`; `node ci/side-panel/bridge_check.mjs`.
   - `cargo xtask bundle-mac --features test-hooks`, then every mode of
     `ci/side-panel/run.sh` (`spike`, `pump`, `reload`, `zombie`,
     `broken`, `stop`, `idle`) and `python3 ci/side-panel/phase5.py`.
     Judge each by its exit status, never through a pipe.
   - Run the same harness on the commit before the change first, so a
     failure can be told from one that was already there. PR #2's
     other commits (one delivery path for clicks and events, the
     helper's page in the relays' map, the single platform gate) have
     not been through the harness either: this run is their test too.
   - In the logs of the runs, look for the warning from step 3 (an id
     CEF replaced). One occurrence means the assumption is false:
     revert the change and record what happened.

5. **Write it up.** A `CHANGELOG.md` entry is not needed (nothing a
   user sees). Add a dated handoff section to `CLAUDE.md` (and the same
   in `AGENTS.md`) with the rule you found in CEF's source, what was
   run and what was not, and delete this file.

## Do not

- Do not change the manager's or the hub's ids; they are the model.
- Do not remove the generation check (`is_current`) on a page's
  callbacks: it guards a different race (a callback from a page that
  has been replaced).
- Do not merge on unit tests alone. The harness is the test of this
  path.
