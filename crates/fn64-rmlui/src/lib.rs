//! Rust binding for RmlUi, scoped to fn64's own settings-menu needs.
//!
//! All RmlUi/plume C++ state stays behind the opaque handles declared in
//! `ffi/fn64_rmlui_shim.h`, mirroring `fn64-render-rt64`'s own convention of
//! quarantining C++ interop to one crate rather than letting it leak into
//! the rest of the workspace.
//!
//! The `rmlui` feature gates the native build; without it this crate
//! compiles to nothing callable, matching `fn64-render-rt64`'s own
//! default-off `rt64` feature so CI/no-GPU hosts stay pure Rust.
//!
//! ## Wrapper shape
//!
//! [`Context`], [`Document`], and [`Element`] each wrap one
//! `NonNull<ffi::Raw*>`, mirroring `fn64-render-rt64/src/ffi/context.rs`'s
//! `pub(crate) struct Context(NonNull<RawContext>)` pattern exactly (same
//! `NonNull` newtype shape, same "one Rust type per opaque C handle, `Drop`
//! calls the matching `_destroy`" discipline). Two differences from that
//! sibling crate, both forced by this shim's own C ABI rather than a
//! independent stylistic choice:
//!
//! - Error propagation pulls from a thread-local
//!   (`fn64_rmlui_last_error()`) instead of writing into an out-parameter
//!   buffer every call takes, because that is the ABI `fn64_rmlui_shim.h`
//!   actually exposes (see its own header comment on
//!   `fn64_rmlui_last_error`). [`Error`] wraps that pulled string.
//! - `Element` has no `_destroy` function in the C ABI at all --
//!   `fn64_rmlui_document_get_element_by_id` hands back a heap-allocated
//!   `Fn64RmluiElement*` wrapper with no matching free function, so
//!   `Element` is intentionally `Copy`-free but ALSO does not implement
//!   `Drop`: it is a small (one-pointer) intentional leak scoped to the
//!   lifetime of the owning `Document` (the same shape RmlUi's own
//!   `Element*` has -- element lifetime is owned by the document tree, not
//!   by whoever looked it up). fn64's settings menu looks up a fixed,
//!   small set of elements once at document-load time and keeps them for
//!   the document's lifetime, so this leak is bounded by construction, not
//!   unbounded per-frame churn.

#![cfg_attr(not(feature = "rmlui"), allow(dead_code))]

#[cfg(feature = "rmlui")]
mod ffi;
pub mod keys;

#[cfg(feature = "rmlui")]
use std::ffi::{c_void, CStr, CString};
#[cfg(feature = "rmlui")]
use std::fmt;
#[cfg(feature = "rmlui")]
use std::ptr::NonNull;

pub use keys::{KeyIdentifier, KeyModifiers};

/// Error pulled from `fn64_rmlui_last_error()` after a failing call.
#[cfg(feature = "rmlui")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

#[cfg(feature = "rmlui")]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "rmlui")]
impl std::error::Error for Error {}

#[cfg(feature = "rmlui")]
fn last_error(fallback: &str) -> Error {
    // SAFETY: `fn64_rmlui_last_error` returns a pointer to a thread-local,
    // null-terminated buffer that stays valid until the next fn64_rmlui_*
    // call on this thread (the header's own documented contract) -- long
    // enough to copy out of immediately, which is all this does.
    let raw = unsafe { ffi::fn64_rmlui_last_error() };
    let message = if raw.is_null() {
        String::new()
    } else {
        // SAFETY: non-null per the check above, and the shim always writes a
        // NUL-terminated buffer (empty string when there is no error).
        unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned()
    };
    Error(if message.is_empty() {
        fallback.to_string()
    } else {
        message
    })
}

/// One RmlUi context, sized to a logical pixel viewport and bound to an
/// existing RT64 render device. Owns everything RmlUi allocates for that
/// viewport (its `Rml::Context`, fn64-rmlui's own render-interface bridge,
/// and the RT64 overlay-draw registration) and tears all of it down on drop.
#[cfg(feature = "rmlui")]
pub struct Context(NonNull<ffi::RawContext>);

#[cfg(feature = "rmlui")]
impl Context {
    /// `rt64` is the raw `Fn64Rt64Context*` a caller already holds (via
    /// whatever route `fn64-render-rt64`'s own Rust surface exposes one --
    /// see that crate's own FFI wrapper). This crate does not create,
    /// validate, or own that pointer; it only forwards it into
    /// `fn64_rmlui_context_create`, matching the header's own documented
    /// ownership split (fn64-rmlui never creates or owns an RT64 device of
    /// its own).
    ///
    /// # Safety
    /// `rt64` must be a live `Fn64Rt64Context*` for the duration of this
    /// call (the shim only reads through it synchronously here; it does not
    /// retain the pointer itself beyond registering a draw callback keyed to
    /// it, per the header's lifecycle documentation).
    pub unsafe fn create(rt64: *mut c_void, width: u32, height: u32) -> Result<Self, Error> {
        // SAFETY: `rt64` validity is the caller's obligation per this
        // function's own safety doc; `fn64_rmlui_context_create` returns
        // either a uniquely-owned context or null with a diagnostic in the
        // thread-local error string.
        let raw = unsafe {
            ffi::fn64_rmlui_context_create(rt64.cast::<ffi::RawRt64Context>(), width, height)
        };
        NonNull::new(raw)
            .map(Self)
            .ok_or_else(|| last_error("fn64_rmlui_context_create failed without a diagnostic"))
    }

    /// Call once per resize (window resize, resolution-scale settings
    /// change).
    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe { ffi::fn64_rmlui_context_set_dimensions(self.0.as_ptr(), width, height) };
    }

    /// Load a document from an in-memory RML buffer (fn64 embeds its own UI
    /// markup via `include_str!` rather than shipping loose files).
    /// `source_url` is only used for RmlUi's own diagnostic messages.
    pub fn load_document_from_memory(
        &mut self,
        rml_source: &str,
        source_url: &str,
    ) -> Result<Document, Error> {
        let source_url =
            CString::new(source_url).map_err(|_| Error("source_url contains a NUL".into()))?;
        // SAFETY: `rml_source`'s pointer/len describe a live Rust `&str`
        // slice for the duration of this synchronous call; `source_url` is
        // a valid NUL-terminated C string kept alive for the same call.
        let raw = unsafe {
            ffi::fn64_rmlui_load_document_from_memory(
                self.0.as_ptr(),
                rml_source.as_ptr().cast(),
                rml_source.len(),
                source_url.as_ptr(),
            )
        };
        NonNull::new(raw).map(Document).ok_or_else(|| {
            last_error("fn64_rmlui_load_document_from_memory failed without a diagnostic")
        })
    }

    /// Process layout/animation/queued Update()-time work. Call once per
    /// tick from wherever the caller's own UI tick runs.
    pub fn update(&mut self) {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe { ffi::fn64_rmlui_context_update(self.0.as_ptr()) };
    }

    /// Forward a mouse-move event. `x`/`y` are logical pixels relative to
    /// the context's own viewport origin. Returns `true` if RmlUi's own
    /// `Context::ProcessMouseMove` returned true (an unobstructed element
    /// under the cursor did NOT request default propagation be blocked --
    /// see `Context.h`'s own return-value documentation), which callers can
    /// use as a "should I also forward this to the game" signal, though
    /// fn64's own menu-open gating (route nothing to the game while the
    /// menu is open) does not need it.
    pub fn process_mouse_move(&mut self, x: i32, y: i32, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_mouse_move(
                self.0.as_ptr(),
                x,
                y,
                modifiers.as_i32(),
            ) != 0
        }
    }

    /// `button` follows RmlUi's own convention: 0 = left, 1 = right, 2 =
    /// middle (`Context.h`'s `ProcessMouseButtonDown` documentation).
    pub fn process_mouse_button_down(&mut self, button: i32, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_mouse_button_down(
                self.0.as_ptr(),
                button,
                modifiers.as_i32(),
            ) != 0
        }
    }

    pub fn process_mouse_button_up(&mut self, button: i32, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_mouse_button_up(
                self.0.as_ptr(),
                button,
                modifiers.as_i32(),
            ) != 0
        }
    }

    pub fn process_mouse_wheel(&mut self, delta: f32, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_mouse_wheel(
                self.0.as_ptr(),
                delta,
                modifiers.as_i32(),
            ) != 0
        }
    }

    pub fn process_key_down(&mut self, key: KeyIdentifier, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_key_down(
                self.0.as_ptr(),
                key.as_i32(),
                modifiers.as_i32(),
            ) != 0
        }
    }

    pub fn process_key_up(&mut self, key: KeyIdentifier, modifiers: KeyModifiers) -> bool {
        // SAFETY: the context is alive and uniquely borrowed.
        unsafe {
            ffi::fn64_rmlui_context_process_key_up(
                self.0.as_ptr(),
                key.as_i32(),
                modifiers.as_i32(),
            ) != 0
        }
    }
}

#[cfg(feature = "rmlui")]
impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: `Context` is the unique owner of the pointer returned by
        // `fn64_rmlui_context_create` and calls destroy exactly once. The
        // shim's own destroy unregisters the RT64 overlay-draw callback
        // before releasing anything else, so no draw callback can fire
        // against a half-torn-down context.
        unsafe { ffi::fn64_rmlui_context_destroy(self.0.as_ptr()) };
    }
}

/// One loaded RML document. `Close()` is deferred to the owning context's
/// next `Update()` call (RmlUi's own documented behavior, carried through by
/// the shim), so a `Document` must not outlive the `Context` it came from.
#[cfg(feature = "rmlui")]
pub struct Document(NonNull<ffi::RawDocument>);

#[cfg(feature = "rmlui")]
impl Document {
    pub fn show(&mut self) {
        // SAFETY: the document is alive and uniquely borrowed.
        unsafe { ffi::fn64_rmlui_document_show(self.0.as_ptr()) };
    }

    pub fn hide(&mut self) {
        // SAFETY: the document is alive and uniquely borrowed.
        unsafe { ffi::fn64_rmlui_document_hide(self.0.as_ptr()) };
    }

    /// Look up a descendant element by its `id` attribute. Returns `None`
    /// if no such element exists (not an `Error`: a missing optional element
    /// in fn64's own markup is a caller bug to assert on, not a runtime
    /// condition to propagate -- see [`Document::require_element`] for the
    /// asserting form callers actually want at document-load time).
    pub fn get_element_by_id(&self, id: &str) -> Option<Element> {
        let id = CString::new(id).ok()?;
        // SAFETY: the document is alive; `id` is a valid NUL-terminated C
        // string kept alive for the call.
        let raw =
            unsafe { ffi::fn64_rmlui_document_get_element_by_id(self.0.as_ptr(), id.as_ptr()) };
        NonNull::new(raw).map(Element)
    }

    /// Same as [`Document::get_element_by_id`], but panics with the missing
    /// id in the message. fn64's settings menu binds a fixed set of element
    /// ids from its own embedded RML at document-load time; a miss there
    /// means the RML and the Rust binding code have drifted out of sync,
    /// which is a programmer error worth failing loudly on rather than
    /// threading an `Option` through every call site.
    pub fn require_element(&self, id: &str) -> Element {
        self.get_element_by_id(id)
            .unwrap_or_else(|| panic!("fn64-rmlui: settings.rml has no element with id {id:?}"))
    }
}

#[cfg(feature = "rmlui")]
impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: `Document` is the unique owner of the pointer returned by
        // `fn64_rmlui_load_document_from_memory` and calls close exactly
        // once.
        unsafe { ffi::fn64_rmlui_document_close(self.0.as_ptr()) };
    }
}

/// One RmlUi element handle. See this module's top-level doc comment for why
/// `Element` intentionally does not implement `Drop`.
#[cfg(feature = "rmlui")]
pub struct Element(NonNull<ffi::RawElement>);

#[cfg(feature = "rmlui")]
impl Element {
    pub fn set_text(&mut self, text: &str) {
        // SAFETY: the element is alive; `text`'s pointer/len describe a live
        // Rust `&str` slice for the duration of this synchronous call.
        unsafe {
            ffi::fn64_rmlui_element_set_text(self.0.as_ptr(), text.as_ptr().cast(), text.len())
        };
    }

    pub fn set_attribute(&mut self, name: &str, value: &str) {
        let Ok(name) = CString::new(name) else {
            return;
        };
        let Ok(value) = CString::new(value) else {
            return;
        };
        // SAFETY: the element is alive; both C strings are kept alive for
        // the call.
        unsafe {
            ffi::fn64_rmlui_element_set_attribute(self.0.as_ptr(), name.as_ptr(), value.as_ptr())
        };
    }

    /// Read an attribute back as a UTF-8 string. RmlUi's `<select>`/
    /// `<input type="range">` controls keep their current selection/drag
    /// value live in the "value" attribute, so `attribute("value")` from
    /// inside an `on_change` callback reads what the user just set. Returns
    /// an empty string if the element has no such attribute (RmlUi's own
    /// `GetAttribute` default), or if `name` is not representable as a C
    /// string.
    pub fn attribute(&self, name: &str) -> String {
        let Ok(name) = CString::new(name) else {
            return String::new();
        };
        let mut buffer = vec![0_u8; 128];
        // SAFETY: the element is alive; `buffer` is writable for its full
        // capacity for the duration of this synchronous call.
        let needed = unsafe {
            ffi::fn64_rmlui_element_get_attribute(
                self.0.as_ptr(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if needed >= buffer.len() {
            // The shim reports the untruncated length even when it had to
            // truncate to fit; retry once with a buffer sized to it.
            buffer = vec![0_u8; needed + 1];
            // SAFETY: same as above, with a buffer now sized to hold the
            // full value plus its NUL terminator.
            unsafe {
                ffi::fn64_rmlui_element_get_attribute(
                    self.0.as_ptr(),
                    name.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
        }
        let nul_at = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
        String::from_utf8_lossy(&buffer[..nul_at]).into_owned()
    }

    pub fn set_class(&mut self, class_name: &str, enabled: bool) {
        let Ok(class_name) = CString::new(class_name) else {
            return;
        };
        // SAFETY: the element is alive; `class_name` is kept alive for the
        // call.
        unsafe {
            ffi::fn64_rmlui_element_set_class(
                self.0.as_ptr(),
                class_name.as_ptr(),
                i32::from(enabled),
            )
        };
    }

    /// Register a click callback. RmlUi's `AddEventListener` does not
    /// replace a prior listener of the same event type, so calling this
    /// more than once on the same `Element` attaches multiple independent
    /// listeners rather than replacing one -- matching the shim's own
    /// documented behavior. The closure is boxed and leaked into the C
    /// side's `user_data` slot; see this crate's trampoline module doc for
    /// why (RmlUi's `OnDetach` reliably frees the shim-side listener
    /// wrapper exactly once, but nothing on the C side ever frees Rust's
    /// `user_data` payload, so the callback must own data cheap enough to
    /// leak for a UI element's lifetime -- one screen's worth of settings
    /// controls, not a dynamic per-frame list).
    pub fn on_click(&mut self, callback: impl FnMut(&mut Element) + 'static) {
        register_callback(self.0, callback, ffi::fn64_rmlui_element_on_click);
    }

    pub fn on_change(&mut self, callback: impl FnMut(&mut Element) + 'static) {
        register_callback(self.0, callback, ffi::fn64_rmlui_element_on_change);
    }
}

/// Trampoline registration shared by `on_click`/`on_change`.
///
/// RmlUi's C ABI callback slot is a plain `extern "C" fn` pointer plus one
/// `void *user_data` (it cannot capture state directly -- the same
/// constraint `fn64_rt64_shim.h`'s own `fn64_rt64_register_overlay_draw`
/// documents for its raw-function-pointer callback slot). A Rust closure is
/// not FFI-safe on its own, so the standard shape is used: box the closure
/// as a trait object, box THAT boxed trait object again into a thin
/// `*mut c_void` the C side can hold opaquely, and use one
/// monomorphization-free `extern "C" fn` per registration function
/// (`trampoline`) that reconstructs the outer `Box` from the raw pointer and
/// calls through it. No existing FFI call in this workspace takes a Rust
/// closure (`fn64-render-rt64`'s own callback-taking export,
/// `fn64_rt64_register_overlay_draw`, is only ever called from
/// `fn64_rmlui_shim.cpp`'s C++ side, never from Rust), so this is designed
/// directly against `Fn64RmluiEventCallback`'s C signature rather than
/// copied from a precedent.
#[cfg(feature = "rmlui")]
fn register_callback(
    element: NonNull<ffi::RawElement>,
    callback: impl FnMut(&mut Element) + 'static,
    register: unsafe extern "C" fn(*mut ffi::RawElement, ffi::EventCallback, *mut c_void),
) {
    type BoxedCallback = Box<dyn FnMut(&mut Element)>;
    let boxed: Box<BoxedCallback> = Box::new(Box::new(callback));
    let user_data = Box::into_raw(boxed).cast::<c_void>();

    extern "C" fn trampoline(element: *mut ffi::RawElement, user_data: *mut c_void) {
        let Some(element) = NonNull::new(element) else {
            return;
        };
        let mut element = Element(element);
        // SAFETY: `user_data` was produced by `Box::into_raw` above, from a
        // `Box<BoxedCallback>` this trampoline is the only reader of. The
        // shim never frees or otherwise inspects `user_data` (its own doc
        // comment on `Fn64RmluiEventCallback` says so); it only stores and
        // passes it back on each event, so reconstructing a `&mut` borrow
        // per call (rather than consuming the `Box`) is sound as long as no
        // two calls run concurrently, which holds here: every
        // `fn64_rmlui_*` callback fires synchronously from within
        // `fn64_rmlui_context_update()`, which the caller only ever invokes
        // from one thread (its own UI tick).
        let callback: &mut BoxedCallback = unsafe { &mut *user_data.cast::<BoxedCallback>() };
        callback(&mut element);
    }

    // SAFETY: `element` is alive; `trampoline` matches
    // `Fn64RmluiEventCallback`'s signature exactly; `user_data` stays valid
    // for the element's lifetime because it is intentionally leaked (see
    // this function's own doc comment) rather than freed here.
    unsafe { register(element.as_ptr(), trampoline, user_data) };
}
