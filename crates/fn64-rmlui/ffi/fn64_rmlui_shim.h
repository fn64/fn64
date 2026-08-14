#ifndef FN64_RMLUI_SHIM_H
#define FN64_RMLUI_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handles. All RmlUi/plume state stays on the C++ side of this
 * boundary -- fn64-rmlui's Rust wrapper only ever holds these pointers,
 * matching fn64-render-rt64's existing FFI convention. */
typedef struct Fn64RmluiContext Fn64RmluiContext;
typedef struct Fn64RmluiDocument Fn64RmluiDocument;
typedef struct Fn64RmluiElement Fn64RmluiElement;

/* Bound to RT64's plume::RenderInterface/RenderDevice, the same objects
 * fn64_rt64_shim.h already exposes an opaque Fn64Rt64Context for. The
 * caller passes that existing context in; this shim does not create or own
 * an RT64 device of its own. */
typedef struct Fn64Rt64Context Fn64Rt64Context;

/* Fn64RmluiContext::create/destroy -------------------------------------- */

/* Creates one RmlUi context sized to (width, height) logical pixels,
 * rendering into the RT64 context's existing device/swapchain, and
 * registers this context's draw callback with
 * fn64_rt64_register_overlay_draw (fn64_rt64_shim.h) so it fires every
 * frame from RT64's present thread, after present-capture's own readback
 * and before the frame is finalized/presented -- there is no
 * fn64_rmlui_context_render() the Rust caller invokes directly, since
 * fn64-rmlui never has the live command list/framebuffer to hand it; RT64
 * hands them to the registered callback instead. See the "Per-frame
 * lifecycle" section below.
 *
 * Returns NULL on failure (RmlUi::Initialise, Rml::CreateContext, or the
 * fn64_rt64_register_overlay_draw call failing); check the last-error
 * string via fn64_rmlui_last_error(). */
Fn64RmluiContext *fn64_rmlui_context_create(Fn64Rt64Context *rt64, uint32_t width, uint32_t height);

/* Calls fn64_rt64_unregister_overlay_draw internally before releasing
 * RmlUi's own context, so no further draw callback can fire once this
 * returns. */
void fn64_rmlui_context_destroy(Fn64RmluiContext *context);

/* Call once per resize (window resize, resolution-scale settings change). */
void fn64_rmlui_context_set_dimensions(Fn64RmluiContext *context, uint32_t width, uint32_t height);

/* Document loading -------------------------------------------------------
 * RML source is passed as an in-memory buffer, not a filesystem path --
 * fn64 embeds its own UI markup in the binary rather than shipping loose
 * .rml/.rcss files at runtime, mirroring recompui's own embedded
 * base_rcss.cpp pattern. */

Fn64RmluiDocument *fn64_rmlui_load_document_from_memory(
    Fn64RmluiContext *context,
    const char *rml_source,
    size_t rml_source_len,
    const char *source_url /* for RmlUi's own error messages; may be empty */
);
void fn64_rmlui_document_show(Fn64RmluiDocument *document);
void fn64_rmlui_document_hide(Fn64RmluiDocument *document);
void fn64_rmlui_document_close(Fn64RmluiDocument *document);

/* Element lookup/construction ---------------------------------------------
 * Imperative, not DataModel-based: recompui's own reference implementation
 * (recompui/src/elements/ui_button.cpp etc.) builds its widget tree this
 * way, not through RmlUi's declarative {{binding}} DataModel system, so
 * fn64-rmlui matches that shape rather than the (unused-in-practice)
 * DataModel API. */

Fn64RmluiElement *fn64_rmlui_document_get_element_by_id(Fn64RmluiDocument *document, const char *id);
void fn64_rmlui_element_set_text(Fn64RmluiElement *element, const char *text, size_t text_len);
void fn64_rmlui_element_set_attribute(Fn64RmluiElement *element, const char *name, const char *value);
void fn64_rmlui_element_set_class(Fn64RmluiElement *element, const char *class_name, int enabled);

/* Event callbacks ---------------------------------------------------------
 * `user_data` is an opaque pointer the Rust side controls (typically a
 * boxed closure or a slot index into a Rust-side table); this shim never
 * inspects it, only stores and passes it back. The callback fires
 * synchronously from within fn64_rmlui_context_update(). */

typedef void (*Fn64RmluiEventCallback)(Fn64RmluiElement *element, void *user_data);

void fn64_rmlui_element_on_click(Fn64RmluiElement *element, Fn64RmluiEventCallback callback, void *user_data);
void fn64_rmlui_element_on_change(Fn64RmluiElement *element, Fn64RmluiEventCallback callback, void *user_data);

/* Per-frame lifecycle ------------------------------------------------------
 * Only Update() is caller-driven, mirroring Rml::Context::Update() directly
 * -- call this once per tick (from wherever the caller's own game/UI tick
 * runs, e.g. wm2000-shell's about_to_wait) to process layout, animations,
 * and any queued Update()-time RmlUi work.
 *
 * There is deliberately no fn64_rmlui_context_render() the caller invokes:
 * fn64-rmlui never has the live plume::RenderCommandList /
 * plume::RenderFramebuffer pointers to hand it (fn64-render-rt64's own
 * Rust layer never has them either --
 * traced the real present path, examples/wm2000-block-boot/src/shell.rs's
 * present_rt64() only reads back already-presented pixels, and
 * fn64_rt64_present() is one opaque synchronous call all the way down to
 * RT64::Application::updateScreen()). Instead, fn64_rmlui_context_create()
 * registers this context's own draw function with RT64's present-thread
 * hook via fn64_rt64_register_overlay_draw (fn64_rt64_shim.h); RmlUi's
 * actual Render() call happens inside that registered callback, called
 * asynchronously by RT64's present thread, not synchronously from Rust. */

void fn64_rmlui_context_update(Fn64RmluiContext *context);

/* Input ---------------------------------------------------------------- */

int fn64_rmlui_context_process_mouse_move(Fn64RmluiContext *context, int32_t x, int32_t y, int32_t key_modifier_state);
int fn64_rmlui_context_process_mouse_button_down(Fn64RmluiContext *context, int32_t button, int32_t key_modifier_state);
int fn64_rmlui_context_process_mouse_button_up(Fn64RmluiContext *context, int32_t button, int32_t key_modifier_state);
int fn64_rmlui_context_process_mouse_wheel(Fn64RmluiContext *context, float delta, int32_t key_modifier_state);
int fn64_rmlui_context_process_key_down(Fn64RmluiContext *context, int32_t key_identifier, int32_t key_modifier_state);
int fn64_rmlui_context_process_key_up(Fn64RmluiContext *context, int32_t key_identifier, int32_t key_modifier_state);

/* Error reporting ------------------------------------------------------- */

/* Returns a pointer to a thread-local, null-terminated UTF-8 string
 * describing the most recent failure on this thread, or an empty string
 * if none. Valid until the next fn64_rmlui_* call on the same thread. */
const char *fn64_rmlui_last_error(void);

#ifdef __cplusplus
}
#endif

#endif /* FN64_RMLUI_SHIM_H */
