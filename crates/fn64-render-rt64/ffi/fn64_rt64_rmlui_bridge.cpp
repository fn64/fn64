#include "fn64_rt64_rmlui_bridge.h"

#include <cstring>
#include <string>

#include "fn64_rt64_shim.h"
#include "fn64_rt64_rmlui_render_interface.h"

namespace {

// Local to this translation unit, matching fn64_rt64_shim.cpp's own
// self-contained per-file `set_error` helper (that one lives in an
// unnamed namespace scoped to its own .cpp, not exported for reuse here).
void set_error(char *error, size_t capacity, const std::string &message) {
    if ((error == nullptr) || (capacity == 0)) {
        return;
    }
    const size_t copy_len = message.size() < (capacity - 1) ? message.size() : (capacity - 1);
    std::memcpy(error, message.data(), copy_len);
    error[copy_len] = '\0';
}

} // namespace

extern "C" void *fn64_rt64_create_rmlui_render_interface(
    Fn64Rt64Context *context,
    uint32_t width,
    uint32_t height,
    char *error,
    size_t error_capacity) {
    if (context == nullptr) {
        set_error(error, error_capacity, "null RT64 context");
        return nullptr;
    }

    char device_error[256] = {0};
    void *raw_device = fn64_rt64_get_render_device(context, device_error, sizeof(device_error));
    if (raw_device == nullptr) {
        set_error(error, error_capacity, std::string("fn64_rt64_get_render_device failed: ") + device_error);
        return nullptr;
    }
    auto *device = static_cast<plume::RenderDevice *>(raw_device);

    // RT64's swap chain is always created as single-sample
    // plume::RenderFormat::B8G8R8A8_UNORM (see
    // RT64::Application::setup()'s RenderSwapChainDesc construction) --
    // this is what the overlay draw callback's plume::RenderFramebuffer
    // actually targets, so the render interface's pipeline is built
    // against those exact values rather than guessing or re-deriving them
    // from the N64 framebuffer's own (HDR-capable, potentially
    // multisampled) color format.
#if defined(_WIN32)
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::DXIL;
#elif defined(__APPLE__)
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::METAL;
#else
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::SPIRV;
#endif

    try {
        // Ownership transfers to the caller (fn64-rmlui), which stores
        // this as the render_interface member of its own Fn64RmluiContext
        // and destroys it via fn64_rt64_destroy_rmlui_render_interface
        // below -- this function does not register any draw-hook callback
        // itself. Registration stays the caller's responsibility (via
        // fn64_rt64_register_overlay_draw directly, unchanged from before
        // this bridge existed) because only the caller has the
        // Rml::Context* whose Render() method the registered callback
        // must invoke; this bridge has no RmlUi::Context type to call
        // through and deliberately does not try to.
        auto *render_interface = new Fn64RmluiRenderInterface(
            device,
            plume::RenderFormat::B8G8R8A8_UNORM,
            plume::RenderMultisampling(),
            shader_format,
            width,
            height);
        // The caller only ever treats this as an opaque
        // Rml::RenderInterface* -- upcasting here is what makes that cast
        // valid on the other side of the boundary (see this header's own
        // comment on why that upcast/downcast pair is safe: both crates
        // link the identical RmlUi checkout and ABI).
        return static_cast<Rml::RenderInterface *>(render_interface);
    } catch (const std::exception &exception) {
        set_error(error, error_capacity, std::string("Fn64RmluiRenderInterface construction failed: ") + exception.what());
        return nullptr;
    } catch (...) {
        set_error(error, error_capacity, "Fn64RmluiRenderInterface construction failed with an unknown C++ exception");
        return nullptr;
    }
}

extern "C" int fn64_rt64_destroy_rmlui_render_interface(
    Fn64Rt64Context *context,
    void *render_interface,
    char *error,
    size_t error_capacity) {
    (void)context;
    if (render_interface == nullptr) {
        return 1;
    }
    // The caller is responsible for having already called
    // fn64_rt64_unregister_overlay_draw(context, ...) before this, per
    // this header's own doc comment -- this function only frees the
    // render-interface object itself, mirroring how
    // fn64_rt64_create_rmlui_render_interface only constructs it and
    // leaves registration to the caller.
    (void)error;
    (void)error_capacity;
    auto *typed_render_interface = static_cast<Fn64RmluiRenderInterface *>(render_interface);
    delete typed_render_interface;
    return 1;
}

extern "C" void fn64_rt64_rmlui_render_interface_set_viewport_size(
    void *render_interface,
    uint32_t width,
    uint32_t height) {
    if (render_interface == nullptr) {
        return;
    }
    static_cast<Fn64RmluiRenderInterface *>(render_interface)->SetViewportSize(width, height);
}

extern "C" void fn64_rt64_rmlui_render_interface_begin_frame(
    void *render_interface,
    void *command_list,
    void *framebuffer) {
    if (render_interface == nullptr) {
        return;
    }
    static_cast<Fn64RmluiRenderInterface *>(render_interface)->BeginFrame(
        static_cast<plume::RenderCommandList *>(command_list),
        static_cast<plume::RenderFramebuffer *>(framebuffer));
}

extern "C" void fn64_rt64_rmlui_render_interface_end_frame(void *render_interface) {
    if (render_interface == nullptr) {
        return;
    }
    static_cast<Fn64RmluiRenderInterface *>(render_interface)->EndFrame();
}
