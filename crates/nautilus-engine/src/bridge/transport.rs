//! How a bridge call arrives and how answers and events leave.

use osmena_core::TabId;
use serde_json::Value;

/// Names one call within the lane it arrived on.
pub(crate) type CallId = u64;

/// Which extension context asked, as far as it decides "the current
/// window".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asker {
    /// The service worker, which is in no window.
    Worker,
    /// A page in the view `0`: a tab of a strip, or a window's panel.
    View(TabId),
    /// An action popup opened while `over` was its window's active tab.
    Popup { over: TabId },
}

/// One call of a bridged `chrome.*` function.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BridgeCall {
    /// The lane's id for the call; the hub replaces it with its own and keeps
    /// the lane's id (`Hub::routes`). For a worker call the lane's id is the
    /// Bridge-local id from `CallIds` (Task 11, Step 1a).
    pub id: CallId,
    /// The calling extension's id.
    pub extension: String,
    pub asker: Asker,
    /// "tabs", "windows", or "bridge" for the script's own messages.
    pub namespace: String,
    pub method: String,
    /// The arguments without a trailing callback; `undefined` is `null`.
    pub args: Vec<Value>,
}

/// One way to reach extension contexts: the relay for service workers,
/// the page lane for extension pages.
pub(crate) trait BridgeTransport {
    /// Settle the promise of `call`: `Ok(value)` resolves it (JSON `null`
    /// is `undefined`), `Err(message)` rejects it with that message.
    fn reply(&self, call: CallId, result: Result<Value, String>);
    /// Run the listeners of `event` (for example "tabs.onCreated") in the
    /// contexts of `extension` this transport serves, starting its worker
    /// if it is stopped and delivering exactly once.
    fn emit(&self, extension: &str, event: &str, args: &[Value]);
    /// Start `extension`'s worker if it is stopped; deliver nothing.
    fn wake(&self, extension: &str);
}
