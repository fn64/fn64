#include "fn64_rmlui_shim.h"

#include <RmlUi/Core.h>
#include <RmlUi/Core/SystemInterface.h>
#include <RmlUi/Core/FileInterface.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <string>
#include <chrono>
#include <memory>
#include <mutex>

// fn64_rt64_shim.h for Fn64Rt64Context and the overlay-draw registration
// calls. This crate depends on fn64-render-rt64's shim at the C++ level
// only (see fn64-rmlui/build.rs) -- it is not a Cargo dependency, since
// fn64-rmlui's Rust surface never calls into fn64-render-rt64's Rust
// surface, only its C ABI, matching how the two crates' native builds are
// deliberately independent (see build.rs's own comment on why RT64 is
// built twice rather than shared across the two crates' OUT_DIRs).
#include "fn64_rt64_shim.h"

#include "plume_render_interface.h"
#include "fn64_rmlui_render_interface.h"

namespace {

thread_local std::string g_last_error;

void set_last_error(const std::string &message) {
    g_last_error = message;
}

void clear_last_error() {
    g_last_error.clear();
}

// Minimal SystemInterface. RmlUi's own default implementations cover
// translation (no-op, 0 translations) and logging (stderr) adequately for
// a first pass; only elapsed time needs a real implementation, since
// RmlUi uses it for animation/transition timing.
class Fn64SystemInterface : public Rml::SystemInterface {
public:
    double GetElapsedTime() override {
        static const auto start = std::chrono::steady_clock::now();
        return std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
    }
};

// Minimal FileInterface. fn64-rmlui loads documents from an in-memory RML
// buffer via fn64_rmlui_load_document_from_memory ->
// Rml::Context::LoadDocumentFromMemory, which does not call through
// FileInterface at all -- but RmlUi::Initialise still requires a
// FileInterface instance to be registered before use (RmlUi's own
// samples/backends always install one). This one's Open/Read/etc. are
// therefore unreachable in fn64-rmlui's current design and exist only to
// satisfy that requirement; if a future version needs to load .rml/.rcss
// from real files (e.g. HD texture-pack-style community UI themes), this
// is the class to extend rather than the one to replace.
class Fn64FileInterface : public Rml::FileInterface {
public:
    Rml::FileHandle Open(const Rml::String & /*path*/) override {
        return 0;
    }
    void Close(Rml::FileHandle /*file*/) override {}
    size_t Read(void * /*buffer*/, size_t /*size*/, Rml::FileHandle /*file*/) override {
        return 0;
    }
    bool Seek(Rml::FileHandle /*file*/, long /*offset*/, int /*origin*/) override {
        return false;
    }
    size_t Tell(Rml::FileHandle /*file*/) override {
        return 0;
    }
};

Fn64SystemInterface g_system_interface;
Fn64FileInterface g_file_interface;
std::mutex g_rmlui_init_mutex;
bool g_rmlui_initialised = false;

// RmlUi's Initialise()/Shutdown() are process-global (Rml::CreateContext
// takes no interface parameters beyond render_interface -- system/file are
// set once via Rml::SetSystemInterface/SetFileInterface, matching a
// process-wide single-set convention, not a per-context one). This shim
// therefore does its Initialise() lazily on the first
// fn64_rmlui_context_create call and never calls Shutdown(): tearing down
// RmlUi's global state while a second Fn64RmluiContext might still be
// live has no clear ownership answer with this shim's per-context handle
// design, so process-exit cleanup is left to normal process teardown, the
// same way fn64-render-rt64 does not call any equivalent "shutdown RT64
// globally" step from an individual Fn64Rt64Context's destructor either.
void ensure_rmlui_initialised() {
    std::scoped_lock lock(g_rmlui_init_mutex);
    if (g_rmlui_initialised) {
        return;
    }
    Rml::SetSystemInterface(&g_system_interface);
    Rml::SetFileInterface(&g_file_interface);
    // No RenderInterface is set here: Rml::SetRenderInterface would be a
    // process-global default, but Rml::CreateContext also accepts a
    // render_interface argument scoped to just that one context, which is
    // the shape fn64_rmlui_context_create actually uses below (each
    // Fn64RmluiContext constructs and owns its own Fn64RmluiRenderInterface
    // bound to the specific Fn64Rt64Context/RenderDevice it was created
    // with, rather than relying on one shared global instance).
    if (!Rml::Initialise()) {
        std::terminate();
    }
    g_rmlui_initialised = true;
}

} // namespace

struct Fn64RmluiContext {
    Rml::Context *context = nullptr;
    Fn64Rt64Context *rt64 = nullptr;
    std::unique_ptr<Fn64RmluiRenderInterface> render_interface;
};

struct Fn64RmluiDocument {
    Rml::ElementDocument *document = nullptr;
};

struct Fn64RmluiElement {
    Rml::Element *element = nullptr;
};

namespace {

// Plain C function pointer trampoline for fn64_rt64_register_overlay_draw's
// callback slot, which cannot capture state (see fn64_rt64_shim.h's own
// comment on why it's a raw function pointer + user_data rather than
// std::function). `user_data` is the owning Fn64RmluiContext*, set at
// registration time in fn64_rmlui_context_create.
void fn64_rmlui_draw_hook_trampoline(void *command_list, void *framebuffer, void *user_data) {
    auto *context = static_cast<Fn64RmluiContext *>(user_data);
    if ((context == nullptr) || (context->context == nullptr) || !context->render_interface) {
        return;
    }
    auto *plume_command_list = static_cast<plume::RenderCommandList *>(command_list);
    auto *plume_framebuffer = static_cast<plume::RenderFramebuffer *>(framebuffer);
    // Rml::RenderInterface's virtuals (CompileGeometry/RenderGeometry/...)
    // take no command-list parameter of their own -- Context::Render()
    // calls them synchronously and expects the render interface to already
    // know where to draw, so the live command list/framebuffer are stashed
    // as member state on the render interface for the duration of this one
    // Render() call, mirroring the header's documented per-frame lifecycle
    // (fn64-rmlui never hands RT64's command list/framebuffer to Rust; they
    // stay entirely on the C++ side of this boundary).
    context->render_interface->BeginFrame(plume_command_list, plume_framebuffer);
    context->context->Render();
    context->render_interface->EndFrame();
}

} // namespace

extern "C" Fn64RmluiContext *fn64_rmlui_context_create(
    Fn64Rt64Context *rt64,
    uint32_t width,
    uint32_t height) {
    clear_last_error();
    if (rt64 == nullptr) {
        set_last_error("null RT64 context");
        return nullptr;
    }
    ensure_rmlui_initialised();

    char rt64_error[256] = {0};
    void *raw_device = fn64_rt64_get_render_device(rt64, rt64_error, sizeof(rt64_error));
    if (raw_device == nullptr) {
        set_last_error(std::string("fn64_rt64_get_render_device failed: ") + rt64_error);
        return nullptr;
    }
    auto *device = static_cast<plume::RenderDevice *>(raw_device);

    // RT64's swap chain is always created as single-sample
    // plume::RenderFormat::B8G8R8A8_UNORM (see
    // RT64::Application::setup()'s RenderSwapChainDesc construction) --
    // this is what the overlay draw callback's plume::RenderFramebuffer
    // actually targets, so the render interface's pipeline is built against
    // those exact values rather than guessing or re-deriving them from the
    // N64 framebuffer's own (HDR-capable, potentially multisampled) color
    // format.
#if defined(_WIN32)
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::DXIL;
#elif defined(__APPLE__)
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::METAL;
#else
    const plume::RenderShaderFormat shader_format = plume::RenderShaderFormat::SPIRV;
#endif

    std::unique_ptr<Fn64RmluiRenderInterface> render_interface;
    try {
        render_interface = std::make_unique<Fn64RmluiRenderInterface>(
            device,
            plume::RenderFormat::B8G8R8A8_UNORM,
            plume::RenderMultisampling(),
            shader_format,
            width,
            height);
    } catch (const std::exception &exception) {
        set_last_error(std::string("Fn64RmluiRenderInterface construction failed: ") + exception.what());
        return nullptr;
    }

    // Context names must be unique per Rml::CreateContext call; fn64
    // currently creates at most one RmlUi context per process (one
    // settings menu), so a fixed name is sufficient. A second concurrent
    // context (unlikely given fn64's single-window shell) would need a
    // real name allocator here.
    Rml::Context *rml_context = Rml::CreateContext(
        "fn64_settings_menu",
        Rml::Vector2i(int(width), int(height)),
        render_interface.get());
    if (rml_context == nullptr) {
        set_last_error("Rml::CreateContext failed");
        return nullptr;
    }

    auto *out = new Fn64RmluiContext{rml_context, rt64, std::move(render_interface)};

    if (!fn64_rt64_register_overlay_draw(
            rt64,
            fn64_rmlui_draw_hook_trampoline,
            out,
            rt64_error,
            sizeof(rt64_error))) {
        set_last_error(std::string("fn64_rt64_register_overlay_draw failed: ") + rt64_error);
        Rml::RemoveContext(rml_context->GetName());
        delete out;
        return nullptr;
    }

    return out;
}

extern "C" void fn64_rmlui_context_destroy(Fn64RmluiContext *context) {
    if (context == nullptr) {
        return;
    }
    // Unregister first, so the draw-hook trampoline can never fire again
    // while `context` is being torn down -- it may still run concurrently
    // from RT64's present thread up until this call returns, matching the
    // same ordering discipline fn64_rt64_shim.cpp's own context destructor
    // already uses for its registries (unregister the callback before
    // freeing the state it points at). The render interface (and the
    // plume pipeline/textures/buffers it owns) is only destroyed after
    // this returns, once no further callback invocation is possible.
    char rt64_error[256] = {0};
    if (!fn64_rt64_unregister_overlay_draw(context->rt64, rt64_error, sizeof(rt64_error))) {
        // Best-effort: context teardown must proceed regardless, but this
        // is unexpected enough (the register call above always succeeds
        // before this destroy could run) to leave a trace for debugging.
        std::fprintf(stderr, "fn64-rmlui: fn64_rt64_unregister_overlay_draw failed: %s\n", rt64_error);
    }
    if (context->context != nullptr) {
        Rml::RemoveContext(context->context->GetName());
    }
    delete context;
}

extern "C" void fn64_rmlui_context_set_dimensions(
    Fn64RmluiContext *context,
    uint32_t width,
    uint32_t height) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return;
    }
    context->context->SetDimensions(Rml::Vector2i(int(width), int(height)));
    if (context->render_interface) {
        context->render_interface->SetViewportSize(width, height);
    }
}

extern "C" Fn64RmluiDocument *fn64_rmlui_load_document_from_memory(
    Fn64RmluiContext *context,
    const char *rml_source,
    size_t rml_source_len,
    const char *source_url) {
    clear_last_error();
    if ((context == nullptr) || (context->context == nullptr)) {
        set_last_error("null RmlUi context");
        return nullptr;
    }
    if (rml_source == nullptr) {
        set_last_error("null RML source buffer");
        return nullptr;
    }
    const std::string rml(rml_source, rml_source_len);
    const std::string url = (source_url != nullptr) ? source_url : "";
    Rml::ElementDocument *document =
        context->context->LoadDocumentFromMemory(rml, url.empty() ? "[fn64 document from memory]" : url);
    if (document == nullptr) {
        set_last_error("Rml::Context::LoadDocumentFromMemory failed");
        return nullptr;
    }
    return new Fn64RmluiDocument{document};
}

extern "C" void fn64_rmlui_document_show(Fn64RmluiDocument *document) {
    if ((document != nullptr) && (document->document != nullptr)) {
        document->document->Show();
    }
}

extern "C" void fn64_rmlui_document_hide(Fn64RmluiDocument *document) {
    if ((document != nullptr) && (document->document != nullptr)) {
        document->document->Hide();
    }
}

extern "C" void fn64_rmlui_document_close(Fn64RmluiDocument *document) {
    if (document == nullptr) {
        return;
    }
    if (document->document != nullptr) {
        // Close() defers actual destruction to the owning Context's next
        // Update() call (RmlUi's own documented behavior), matching the
        // header's note on fn64_rmlui_document_close.
        document->document->Close();
    }
    delete document;
}

extern "C" Fn64RmluiElement *fn64_rmlui_document_get_element_by_id(
    Fn64RmluiDocument *document,
    const char *id) {
    clear_last_error();
    if ((document == nullptr) || (document->document == nullptr) || (id == nullptr)) {
        set_last_error("null document or element id");
        return nullptr;
    }
    Rml::Element *element = document->document->GetElementById(id);
    if (element == nullptr) {
        set_last_error(std::string("no element with id \"") + id + "\"");
        return nullptr;
    }
    return new Fn64RmluiElement{element};
}

extern "C" void fn64_rmlui_element_set_text(
    Fn64RmluiElement *element,
    const char *text,
    size_t text_len) {
    if ((element == nullptr) || (element->element == nullptr) || (text == nullptr)) {
        return;
    }
    element->element->SetInnerRML(std::string(text, text_len));
}

extern "C" void fn64_rmlui_element_set_attribute(
    Fn64RmluiElement *element,
    const char *name,
    const char *value) {
    if ((element == nullptr) || (element->element == nullptr) ||
        (name == nullptr) || (value == nullptr)) {
        return;
    }
    element->element->SetAttribute(name, value);
}

extern "C" void fn64_rmlui_element_set_class(
    Fn64RmluiElement *element,
    const char *class_name,
    int enabled) {
    if ((element == nullptr) || (element->element == nullptr) || (class_name == nullptr)) {
        return;
    }
    element->element->SetClass(class_name, enabled != 0);
}

extern "C" size_t fn64_rmlui_element_get_attribute(
    Fn64RmluiElement *element,
    const char *name,
    char *buffer,
    size_t buffer_capacity) {
    if ((buffer != nullptr) && (buffer_capacity > 0)) {
        buffer[0] = '\0';
    }
    if ((element == nullptr) || (element->element == nullptr) || (name == nullptr)) {
        return 0;
    }
    const Rml::String value = element->element->GetAttribute<Rml::String>(name, Rml::String());
    if ((buffer == nullptr) || (buffer_capacity == 0)) {
        return value.size();
    }
    const size_t copy_len = std::min(value.size(), buffer_capacity - 1);
    std::memcpy(buffer, value.data(), copy_len);
    buffer[copy_len] = '\0';
    return value.size();
}

namespace {

// Bridges one Fn64RmluiEventCallback to RmlUi's Rml::EventListener
// interface. AddEventListener does not take ownership (see Element.h's own
// comment: a listener is only detached, e.g. on element destruction or an
// explicit RemoveEventListener call with the same parameters) -- this class
// owns itself instead, freeing itself from OnDetach so callers never need a
// matching "destroy listener" call. Each fn64_rmlui_element_on_click/
// on_change call creates and attaches its own instance, so calling either
// more than once on the same element attaches multiple independent
// listeners rather than replacing a prior one -- the natural RmlUi
// behavior, since Element::AddEventListener has no "replace" concept either.
class Fn64RmluiEventListener : public Rml::EventListener {
public:
    Fn64RmluiEventListener(Fn64RmluiEventCallback callback, void *user_data)
        : callback_(callback), user_data_(user_data) {}

    void ProcessEvent(Rml::Event &event) override {
        if (callback_ == nullptr) {
            return;
        }
        Rml::Element *element = event.GetCurrentElement();
        if (element == nullptr) {
            return;
        }
        Fn64RmluiElement handle{element};
        callback_(&handle, user_data_);
    }

    void OnDetach(Rml::Element * /*element*/) override {
        delete this;
    }

private:
    Fn64RmluiEventCallback callback_ = nullptr;
    void *user_data_ = nullptr;
};

} // namespace

extern "C" void fn64_rmlui_element_on_click(
    Fn64RmluiElement *element,
    Fn64RmluiEventCallback callback,
    void *user_data) {
    if ((element == nullptr) || (element->element == nullptr) || (callback == nullptr)) {
        return;
    }
    element->element->AddEventListener("click", new Fn64RmluiEventListener(callback, user_data));
}

extern "C" void fn64_rmlui_element_on_change(
    Fn64RmluiElement *element,
    Fn64RmluiEventCallback callback,
    void *user_data) {
    if ((element == nullptr) || (element->element == nullptr) || (callback == nullptr)) {
        return;
    }
    element->element->AddEventListener("change", new Fn64RmluiEventListener(callback, user_data));
}

extern "C" void fn64_rmlui_context_update(Fn64RmluiContext *context) {
    if ((context != nullptr) && (context->context != nullptr)) {
        context->context->Update();
    }
}

extern "C" int fn64_rmlui_context_process_mouse_move(
    Fn64RmluiContext *context,
    int32_t x,
    int32_t y,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessMouseMove(int(x), int(y), int(key_modifier_state)) ? 1 : 0;
}

extern "C" int fn64_rmlui_context_process_mouse_button_down(
    Fn64RmluiContext *context,
    int32_t button,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessMouseButtonDown(int(button), int(key_modifier_state)) ? 1 : 0;
}

extern "C" int fn64_rmlui_context_process_mouse_button_up(
    Fn64RmluiContext *context,
    int32_t button,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessMouseButtonUp(int(button), int(key_modifier_state)) ? 1 : 0;
}

extern "C" int fn64_rmlui_context_process_mouse_wheel(
    Fn64RmluiContext *context,
    float delta,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessMouseWheel(delta, int(key_modifier_state)) ? 1 : 0;
}

extern "C" int fn64_rmlui_context_process_key_down(
    Fn64RmluiContext *context,
    int32_t key_identifier,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessKeyDown(
        Rml::Input::KeyIdentifier(key_identifier), int(key_modifier_state)) ? 1 : 0;
}

extern "C" int fn64_rmlui_context_process_key_up(
    Fn64RmluiContext *context,
    int32_t key_identifier,
    int32_t key_modifier_state) {
    if ((context == nullptr) || (context->context == nullptr)) {
        return 0;
    }
    return context->context->ProcessKeyUp(
        Rml::Input::KeyIdentifier(key_identifier), int(key_modifier_state)) ? 1 : 0;
}

extern "C" const char *fn64_rmlui_last_error(void) {
    return g_last_error.c_str();
}
