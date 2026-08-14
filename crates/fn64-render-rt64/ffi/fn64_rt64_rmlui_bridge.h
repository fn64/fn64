#ifndef FN64_RT64_RMLUI_BRIDGE_H
#define FN64_RT64_RMLUI_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

/* Only compiled/declared when this crate's `rmlui` Cargo feature is
 * enabled (see ffi/CMakeLists.txt) -- fn64_rt64_shim.h itself stays free
 * of any RmlUi dependency for every other build. fn64-rmlui is the one
 * and only intended caller of this header; it links against the same
 * RmlUi checkout this crate does, which is what makes the `void*` handles
 * below (really `Rml::RenderInterface*` under the hood) safe to hand
 * across this boundary and cast back to real RmlUi types on the other
 * side, the same convention fn64_rt64_shim.h already uses for `plume`
 * types it does not want to name directly in a C-compatible header. */

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Fn64Rt64Context Fn64Rt64Context;

/* Constructs an RmlUi render-interface bridge (implements RmlUi's
 * `Rml::RenderInterface`) bound to `context`'s live RT64 device, sized to
 * `width`x`height` logical pixels, and registers its per-frame draw
 * callback via this crate's own overlay-draw registry (mirrors what a
 * caller would have done itself with fn64_rt64_register_overlay_draw,
 * except the callback and its state are both owned entirely on this side
 * of the boundary now).
 *
 * Returns an opaque pointer that IS a `Rml::RenderInterface*` (safe to
 * `static_cast<Rml::RenderInterface*>` on the caller's side, since both
 * this crate and fn64-rmlui compile against the identical RmlUi checkout
 * and ABI) -- pass it directly as `Rml::CreateContext`'s `render_interface`
 * parameter. Returns NULL and sets `error` on failure (construction
 * exception, or the overlay-draw registry rejecting registration for the
 * same reasons fn64_rt64_register_overlay_draw's own doc comment
 * describes). */
void *fn64_rt64_create_rmlui_render_interface(
    Fn64Rt64Context *context,
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity);

/* Unregisters the draw callback and destroys the render interface created
 * by fn64_rt64_create_rmlui_render_interface. Call this before releasing
 * whatever RmlUi context/document was built against the render interface
 * -- same "unregister before freeing the state a still-live present-thread
 * callback could reach" ordering discipline fn64_rt64_unregister_overlay_draw
 * itself requires of its own callers. */
int fn64_rt64_destroy_rmlui_render_interface(
    Fn64Rt64Context *context,
    void *render_interface,
    char *error,
    size_t error_capacity);

/* Call once per resize (window resize, resolution-scale settings change),
 * mirroring fn64_rmlui_context_set_dimensions' own per-frame-lifecycle
 * contract on the fn64-rmlui side of this boundary. */
void fn64_rt64_rmlui_render_interface_set_viewport_size(
    void *render_interface,
    uint32_t width,
    uint32_t height);

/* Brackets one Rml::Context::Render() call, which the CALLER (fn64-rmlui,
 * the only side holding a typed Rml::Context*) invokes in between these
 * two calls -- this crate has no RmlUi Context type of its own to call
 * Render() through, so it cannot bracket the call itself the way the
 * pre-migration single-crate trampoline did. `command_list`/`framebuffer`
 * are opaque `plume::RenderCommandList*`/`plume::RenderFramebuffer*`,
 * matching every other command-list/framebuffer crossing in
 * fn64_rt64_shim.h -- the caller receives these same two pointers,
 * unmodified, from its own fn64_rt64_register_overlay_draw callback and
 * simply forwards them here without needing to know their real types.
 * Must be called from the same registered overlay-draw callback these
 * pointers came from, in BeginFrame/Render()/EndFrame order, every frame
 * the render interface is asked to draw. */
void fn64_rt64_rmlui_render_interface_begin_frame(
    void *render_interface,
    void *command_list,
    void *framebuffer);
void fn64_rt64_rmlui_render_interface_end_frame(void *render_interface);

#ifdef __cplusplus
}
#endif

#endif /* FN64_RT64_RMLUI_BRIDGE_H */
