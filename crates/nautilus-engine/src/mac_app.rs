//! macOS: make winit's `NSApplication` the one CEF requires.
//!
//! CEF's `cef_application_mac.h` says every client must subclass
//! `NSApplication` and implement `CefAppProtocol`: `isHandlingSendEvent`
//! and `setHandlingSendEvent:`, kept true while `sendEvent:` is on the
//! stack, which Chromium's main-thread message pump reads to tell a
//! nested run loop from its own. winit already owns the application
//! object (its own subclass, created before the engine starts), so the
//! protocol is added to that class at run time: the two methods, the
//! protocols themselves, and a `sendEvent:` that sets the flag around
//! whatever `sendEvent:` was there (winit's own override).

use std::cell::Cell;
use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

use objc2::encode::Encode;
use objc2::runtime::{AnyObject, Bool, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};

type SendEvent = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject);

/// What `sendEvent:` did before (winit's override, or AppKit's).
static ORIGINAL_SEND_EVENT: OnceLock<SendEvent> = OnceLock::new();

thread_local! {
    /// Set while `sendEvent:` is on the stack. The application object
    /// is a singleton used on the main thread only.
    static HANDLING_SEND_EVENT: Cell<bool> = const { Cell::new(false) };
}

extern "C-unwind" fn is_handling_send_event(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::new(HANDLING_SEND_EVENT.with(Cell::get))
}

extern "C-unwind" fn set_handling_send_event(_this: &AnyObject, _cmd: Sel, handling: Bool) {
    HANDLING_SEND_EVENT.with(|h| h.set(handling.as_bool()));
}

extern "C-unwind" fn send_event(this: &AnyObject, cmd: Sel, event: &AnyObject) {
    let was = HANDLING_SEND_EVENT.with(|h| h.replace(true));
    if let Some(original) = ORIGINAL_SEND_EVENT.get() {
        // SAFETY: the implementation taken from the class's own
        // `sendEvent:` method, with its real signature.
        unsafe { original(this, cmd, event) };
    }
    HANDLING_SEND_EVENT.with(|h| h.set(was));
}

/// Give the shared `NSApplication` CEF's protocol. Call on the main
/// thread, after the application object exists and before
/// `cef::initialize`. Idempotent.
pub(crate) fn adopt_cef_app_protocol() {
    // SAFETY: plain Objective-C runtime calls on the main thread; the
    // shared application is a live object, and the methods added have
    // the signatures their type encodings declare.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            log::warn!("no NSApplication to give the CEF protocol to");
            return;
        }
        let class = ffi::object_getClass(app).cast_mut();
        if class.is_null() {
            return;
        }
        if ffi::class_respondsToSelector(class, sel!(isHandlingSendEvent)).as_bool() {
            return;
        }
        let bool_encoding = Bool::ENCODING.to_string();
        let getter_types = format!("{bool_encoding}@:");
        let setter_types = format!("v@:{bool_encoding}");
        let getter_types = std::ffi::CString::new(getter_types).expect("no NUL");
        let setter_types = std::ffi::CString::new(setter_types).expect("no NUL");
        let getter: extern "C-unwind" fn(&AnyObject, Sel) -> Bool = is_handling_send_event;
        let setter: extern "C-unwind" fn(&AnyObject, Sel, Bool) = set_handling_send_event;
        ffi::class_addMethod(
            class,
            sel!(isHandlingSendEvent),
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel) -> Bool, Imp>(getter),
            getter_types.as_ptr(),
        );
        ffi::class_addMethod(
            class,
            sel!(setHandlingSendEvent:),
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, Bool), Imp>(setter),
            setter_types.as_ptr(),
        );
        // The protocols are registered by the framework that declares
        // them (Chromium's base for the first two); one that is not
        // registered yet is simply not adopted.
        for name in [c"CrAppProtocol", c"CrAppControlProtocol", c"CefAppProtocol"] {
            let protocol = ffi::objc_getProtocol(name.as_ptr());
            if protocol.is_null() {
                log::debug!("protocol {} is not registered", name.to_string_lossy());
                continue;
            }
            ffi::class_addProtocol(class, protocol);
        }
        let method = ffi::class_getInstanceMethod(class, sel!(sendEvent:));
        if method.is_null() {
            log::warn!("NSApplication has no sendEvent: method");
            return;
        }
        let replacement: SendEvent = send_event;
        let replacement = std::mem::transmute::<SendEvent, Imp>(replacement);
        let original = ffi::method_setImplementation(method, replacement);
        if let Some(original) = original {
            let original = std::mem::transmute::<Imp, SendEvent>(original);
            let _ = ORIGINAL_SEND_EVENT.set(original);
        }
        log::debug!(
            "NSApplication class {} adopted CEF's application protocol",
            class_name(class)
        );
    }
}

unsafe fn class_name(class: *const objc2::runtime::AnyClass) -> String {
    // SAFETY: the runtime returns a NUL-terminated name for a live class.
    let name: *const c_char = unsafe { ffi::class_getName(class) };
    if name.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}
