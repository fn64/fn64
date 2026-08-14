#ifndef FN64_RT64_RMLUI_RENDER_INTERFACE_H
#define FN64_RT64_RMLUI_RENDER_INTERFACE_H

#include <RmlUi/Core/RenderInterface.h>

#include <memory>
#include <vector>

#include "plume_render_interface.h"

// Bridges RmlUi's RenderInterface abstraction directly onto plume's public
// RenderDevice/RenderCommandList API. Lives in fn64-render-rt64 (not
// fn64-rmlui) because implementing Rml::RenderInterface using plume types
// unavoidably needs BOTH RmlUi's and RT64/plume's headers at once -- there
// is no way to build this one class without both, regardless of which
// crate's source tree it lives in, so it lives where plume is already a
// first-class dependency rather than making fn64-rmlui (which otherwise has
// zero RT64/plume knowledge) pull them in just for this. One instance is
// constructed per Fn64RmluiContext, at overlay-registration time (see
// fn64_rt64_rmlui_bridge.cpp's fn64_rt64_create_rmlui_render_interface),
// when a live plume RenderDevice* is first available. The pipeline,
// descriptor-set layout, and default white texture/sampler are all built
// once in the constructor; nothing here allocates a plume::RenderPipeline
// or plume::RenderSampler per frame or per draw call.
//
// Threading/lifetime: every method below (including the 8 pure virtuals
// RmlUi requires) is only ever called either from fn64-rmlui's own
// single-threaded Rust-driven Update() path (which touches no plume state)
// or from within the RT64 present-thread draw callback registered via
// this crate's own overlay-draw registry, which is where CompileGeometry/
// RenderGeometry/LoadTexture/GenerateTexture/ReleaseGeometry/ReleaseTexture
// actually touch plume objects (RmlUi calls these synchronously from inside
// Rml::Context::Render(), which this class's owner calls from that same
// draw callback -- see BeginFrame() below). There is therefore exactly one
// thread ever inside this class's plume-touching methods at a time; no
// internal locking is needed beyond what plume's own objects already do.
class Fn64RmluiRenderInterface : public Rml::RenderInterface {
public:
    // `colorFormat`/`colorMultisampling` describe the swapchain framebuffer
    // the registered draw callback will target (RT64's swapchain is always
    // single-sample plume::RenderFormat::B8G8R8A8_UNORM -- see
    // RT64::Application::setup()'s RenderSwapChainDesc construction -- the
    // caller passes the real values rather than this class hardcoding them,
    // so a future swapchain format change only needs updating at the one
    // call site in fn64_rt64_rmlui_bridge.cpp).
    Fn64RmluiRenderInterface(
        plume::RenderDevice *device,
        plume::RenderFormat colorFormat,
        plume::RenderMultisampling colorMultisampling,
        plume::RenderShaderFormat shaderFormat,
        uint32_t viewportWidth,
        uint32_t viewportHeight);
    ~Fn64RmluiRenderInterface() override;

    Fn64RmluiRenderInterface(const Fn64RmluiRenderInterface &) = delete;
    Fn64RmluiRenderInterface &operator=(const Fn64RmluiRenderInterface &) = delete;

    // Called once per frame by fn64_rt64_rmlui_bridge.cpp's registered draw-hook
    // trampoline, bracketing the Rml::Context::Render() call that in turn
    // invokes the RenderInterface virtuals below. Stashes the live
    // command list/framebuffer as member state, since none of RmlUi's
    // RenderInterface virtuals take a command-list parameter of their own.
    void BeginFrame(plume::RenderCommandList *commandList, plume::RenderFramebuffer *framebuffer);
    void EndFrame();

    // Must be called whenever the context's logical pixel dimensions change
    // (mirrors fn64_rmlui_context_set_dimensions on the fn64-rmlui side of
    // the boundary, via fn64_rt64_rmlui_render_interface_set_viewport_size),
    // since the vertex shader's pixel-to-clip-space mapping depends on the
    // viewport size.
    void SetViewportSize(uint32_t width, uint32_t height);

    // Rml::RenderInterface's 8 required pure virtuals. RmlUi's optional
    // methods (clip masks, layers, filters, custom shaders, SetTransform)
    // are intentionally left at their RenderInterface:: default no-op
    // implementations -- this bridge does not override them.
    Rml::CompiledGeometryHandle CompileGeometry(Rml::Span<const Rml::Vertex> vertices, Rml::Span<const int> indices) override;
    void RenderGeometry(Rml::CompiledGeometryHandle geometry, Rml::Vector2f translation, Rml::TextureHandle texture) override;
    void ReleaseGeometry(Rml::CompiledGeometryHandle geometry) override;

    Rml::TextureHandle LoadTexture(Rml::Vector2i &textureDimensions, const Rml::String &source) override;
    Rml::TextureHandle GenerateTexture(Rml::Span<const Rml::byte> source, Rml::Vector2i sourceDimensions) override;
    void ReleaseTexture(Rml::TextureHandle texture) override;

    void EnableScissorRegion(bool enable) override;
    void SetScissorRegion(Rml::Rectanglei region) override;

private:
    struct CompiledGeometry {
        std::unique_ptr<plume::RenderBuffer> vertexBuffer;
        uint64_t vertexBufferCapacity = 0;
        std::unique_ptr<plume::RenderBuffer> indexBuffer;
        uint64_t indexBufferCapacity = 0;
        uint32_t indexCount = 0;
    };

    struct LoadedTexture {
        std::unique_ptr<plume::RenderTexture> texture;
        std::unique_ptr<plume::RenderTextureView> textureView;
        std::unique_ptr<plume::RenderDescriptorSet> descriptorSet;
    };

    void UploadTexturePixels(
        plume::RenderTexture *texture,
        uint32_t width,
        uint32_t height,
        const void *pixels);
    plume::RenderDescriptorSet *DescriptorSetForTexture(Rml::TextureHandle texture) const;

    plume::RenderDevice *device_ = nullptr;
    plume::RenderCommandList *commandList_ = nullptr;
    plume::RenderFramebuffer *framebuffer_ = nullptr;
    uint32_t viewportWidth_ = 1;
    uint32_t viewportHeight_ = 1;
    bool scissorEnabled_ = false;
    plume::RenderRect scissorRect_;

    std::unique_ptr<plume::RenderPipelineLayout> pipelineLayout_;
    std::unique_ptr<plume::RenderPipeline> pipeline_;
    std::unique_ptr<plume::RenderSampler> sampler_;
    std::unique_ptr<plume::RenderTexture> whiteTexture_;
    std::unique_ptr<plume::RenderTextureView> whiteTextureView_;
    std::unique_ptr<plume::RenderDescriptorSet> whiteDescriptorSet_;
};

#endif // FN64_RT64_RMLUI_RENDER_INTERFACE_H
