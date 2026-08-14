//! Raw declarations for `ffi/fn64_rmlui_shim.h`, one `unsafe extern "C"`
//! block matching the header signature-for-signature. Mirrors
//! `fn64-render-rt64/src/ffi/config_wire.rs`'s declaration shape (modern
//! `unsafe extern "C" { .. }` block, `pub(super) fn`, no `#[link]` attribute
//! since `build.rs` already emits the link directives).
//!
//! Unlike fn64-render-rt64's shim, this header reports errors through one
//! thread-local pull (`fn64_rmlui_last_error`) rather than an out-parameter
//! buffer per call -- so there is no `RawContext`-style zero-sized opaque
//! marker needed for an error buffer convention; the three opaque handle
//! types below stand in for `Fn64RmluiContext`/`Fn64RmluiDocument`/
//! `Fn64RmluiElement` the same way `RawContext` stands in for
//! fn64-render-rt64's opaque context.

use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub(crate) struct RawContext {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct RawDocument {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct RawElement {
    _private: [u8; 0],
}

/// Opaque `Fn64Rt64Context*`. fn64-rmlui never constructs or inspects this --
/// it only forwards the pointer a caller already holds (via
/// `fn64-render-rt64`'s own Rust wrapper) into `fn64_rmlui_context_create`.
#[repr(C)]
pub(crate) struct RawRt64Context {
    _private: [u8; 0],
}

pub(crate) type EventCallback = extern "C" fn(*mut RawElement, *mut c_void);

unsafe extern "C" {
    pub(crate) fn fn64_rmlui_context_create(
        rt64: *mut RawRt64Context,
        width: u32,
        height: u32,
    ) -> *mut RawContext;
    pub(crate) fn fn64_rmlui_context_destroy(context: *mut RawContext);
    pub(crate) fn fn64_rmlui_context_set_dimensions(
        context: *mut RawContext,
        width: u32,
        height: u32,
    );

    pub(crate) fn fn64_rmlui_load_document_from_memory(
        context: *mut RawContext,
        rml_source: *const c_char,
        rml_source_len: usize,
        source_url: *const c_char,
    ) -> *mut RawDocument;
    pub(crate) fn fn64_rmlui_document_show(document: *mut RawDocument);
    pub(crate) fn fn64_rmlui_document_hide(document: *mut RawDocument);
    pub(crate) fn fn64_rmlui_document_close(document: *mut RawDocument);

    pub(crate) fn fn64_rmlui_document_get_element_by_id(
        document: *mut RawDocument,
        id: *const c_char,
    ) -> *mut RawElement;
    pub(crate) fn fn64_rmlui_element_set_text(
        element: *mut RawElement,
        text: *const c_char,
        text_len: usize,
    );
    pub(crate) fn fn64_rmlui_element_set_attribute(
        element: *mut RawElement,
        name: *const c_char,
        value: *const c_char,
    );
    pub(crate) fn fn64_rmlui_element_set_class(
        element: *mut RawElement,
        class_name: *const c_char,
        enabled: c_int,
    );

    pub(crate) fn fn64_rmlui_element_on_click(
        element: *mut RawElement,
        callback: EventCallback,
        user_data: *mut c_void,
    );
    pub(crate) fn fn64_rmlui_element_on_change(
        element: *mut RawElement,
        callback: EventCallback,
        user_data: *mut c_void,
    );

    pub(crate) fn fn64_rmlui_context_update(context: *mut RawContext);

    pub(crate) fn fn64_rmlui_context_process_mouse_move(
        context: *mut RawContext,
        x: i32,
        y: i32,
        key_modifier_state: i32,
    ) -> c_int;
    pub(crate) fn fn64_rmlui_context_process_mouse_button_down(
        context: *mut RawContext,
        button: i32,
        key_modifier_state: i32,
    ) -> c_int;
    pub(crate) fn fn64_rmlui_context_process_mouse_button_up(
        context: *mut RawContext,
        button: i32,
        key_modifier_state: i32,
    ) -> c_int;
    pub(crate) fn fn64_rmlui_context_process_mouse_wheel(
        context: *mut RawContext,
        delta: f32,
        key_modifier_state: i32,
    ) -> c_int;
    pub(crate) fn fn64_rmlui_context_process_key_down(
        context: *mut RawContext,
        key_identifier: i32,
        key_modifier_state: i32,
    ) -> c_int;
    pub(crate) fn fn64_rmlui_context_process_key_up(
        context: *mut RawContext,
        key_identifier: i32,
        key_modifier_state: i32,
    ) -> c_int;

    pub(crate) fn fn64_rmlui_last_error() -> *const c_char;
}
