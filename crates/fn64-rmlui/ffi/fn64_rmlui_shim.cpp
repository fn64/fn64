#include "fn64_rmlui_shim.h"

#include <RmlUi/Core.h>
#include <RmlUi/Core/SystemInterface.h>
#include <RmlUi/Core/FileInterface.h>

#include <cstring>
#include <string>
#include <chrono>
#include <mutex>

// fn64_rt64_shim.h for Fn64Rt64Context and the overlay-draw registration
// calls. This crate depends on fn64-render-rt64's shim at the C++ level
// only (see fn64-rmlui/build.rs) -- it is not a Cargo dependency, since
// fn64-rmlui's Rust surface never calls into fn64-render-rt64's Rust
// surface, only its C ABI, matching how the two crates' native builds are
// deliberately independent (see build.rs's own comment on why RT64 is
// built twice rather than shared across the two crates' OUT_DIRs).
#include "fn64_rt64_shim.h"

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
    // No RenderInterface is set here (Rml::SetRenderInterface is skipped):
    // the render-interface bridge against plume/RT64 is deferred to a
    // later pass (see the header's "Per-frame lifecycle" comment on why
    // fn64_rmlui_context_create still succeeds without one -- Update()
    // and layout work without a render interface; only Render() needs it,
    // and this skeleton does not yet register a draw callback that would
    // call Render()).
    if (!Rml::Initialise()) {
        std::terminate();
    }
    g_rmlui_initialised = true;
}

} // namespace

struct Fn64RmluiContext {
    Rml::Context *context = nullptr;
    Fn64Rt64Context *rt64 = nullptr;
};

struct Fn64RmluiDocument {
    Rml::ElementDocument *document = nullptr;
};

struct Fn64RmluiElement {
    Rml::Element *element = nullptr;
};

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

    // Context names must be unique per Rml::CreateContext call; fn64
    // currently creates at most one RmlUi context per process (one
    // settings menu), so a fixed name is sufficient. A second concurrent
    // context (unlikely given fn64's single-window shell) would need a
    // real name allocator here.
    Rml::Context *rml_context = Rml::CreateContext(
        "fn64_settings_menu",
        Rml::Vector2i(int(width), int(height)));
    if (rml_context == nullptr) {
        set_last_error("Rml::CreateContext failed");
        return nullptr;
    }

    // TODO(fn64-rmlui render-interface pass): register this context's
    // draw callback with fn64_rt64_register_overlay_draw so RT64's
    // present thread actually calls into RmlUi's Render(). Deferred to
    // the render-interface implementation pass -- this skeleton proves
    // the context/document/element lifecycle and build/link path first,
    // per the scoping decision to split render-interface work out.

    auto *out = new Fn64RmluiContext{rml_context, rt64};
    return out;
}

extern "C" void fn64_rmlui_context_destroy(Fn64RmluiContext *context) {
    if (context == nullptr) {
        return;
    }
    // fn64_rt64_unregister_overlay_draw(context->rt64, nullptr, 0) belongs
    // here once fn64_rmlui_context_create actually registers a callback
    // (see the TODO above) -- until then there is nothing to unregister.
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

namespace {

// TODO(fn64-rmlui event-callback pass): wire Fn64RmluiEventCallback into
// RmlUi's Rml::EventListener via Element::AddEventListener("click"/
// "change", ...). Deferred alongside the render-interface work: this
// skeleton's fn64_rmlui_element_on_click/on_change intentionally
// no-op for now rather than half-wiring an event-listener class with no
// way to test it end-to-end without a working render path to see the
// element being clicked in the first place.

} // namespace

extern "C" void fn64_rmlui_element_on_click(
    Fn64RmluiElement * /*element*/,
    Fn64RmluiEventCallback /*callback*/,
    void * /*user_data*/) {
    // See TODO above; not yet implemented.
}

extern "C" void fn64_rmlui_element_on_change(
    Fn64RmluiElement * /*element*/,
    Fn64RmluiEventCallback /*callback*/,
    void * /*user_data*/) {
    // See TODO above; not yet implemented.
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
