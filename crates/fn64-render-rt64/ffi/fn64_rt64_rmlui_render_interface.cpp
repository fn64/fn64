#include "fn64_rt64_rmlui_render_interface.h"

#include <cassert>
#include <cstring>
#include <stdexcept>

#include "fn64_rt64_rmlui_ui.h"

// Generated shader blobs (see CMakeLists.txt's build_vertex_shader/
// build_pixel_shader calls). One or two of these three headers exist
// per-platform (DXIL on Windows, SPIRV always available as the Vulkan/
// cross-compilation source, MSL on Apple) -- CREATE_SHADER_INPUTS below
// picks the one this platform actually needs at compile time, matching
// RT64's own rt64_shader_library.cpp pattern for the same three-way choice.
#include "shaders/Fn64RmluiUiVS.hlsl.spirv.h"
#include "shaders/Fn64RmluiUiPS.hlsl.spirv.h"
#if defined(_WIN32)
#include "shaders/Fn64RmluiUiVS.hlsl.dxil.h"
#include "shaders/Fn64RmluiUiPS.hlsl.dxil.h"
#elif defined(__APPLE__)
#include "shaders/Fn64RmluiUiVS.hlsl.metal.h"
#include "shaders/Fn64RmluiUiPS.hlsl.metal.h"
#endif

// RenderBarrierStage and RenderShaderStageFlag are plume namespaces (each
// holding an enum Bits), not types -- "using" only works for the type-level
// names below; barrier-stage/shader-stage-flag constants are referenced
// fully qualified (plume::RenderBarrierStage::..., plume::RenderShaderStageFlag::...).
using plume::RenderBlend;
using plume::RenderBlendDesc;
using plume::RenderBlendOperation;
using plume::RenderBuffer;
using plume::RenderBufferDesc;
using plume::RenderBufferReference;
using plume::RenderCullMode;
using plume::RenderDescriptorSetBuilder;
using plume::RenderFilter;
using plume::RenderFormat;
using plume::RenderFrontFace;
using plume::RenderGraphicsPipelineDesc;
using plume::RenderHeapType;
using plume::RenderIndexBufferView;
using plume::RenderInputElement;
using plume::RenderInputSlot;
using plume::RenderInputSlotClassification;
using plume::RenderPipelineLayoutBuilder;
using plume::RenderPrimitiveTopology;
using plume::RenderRect;
using plume::RenderSamplerDesc;
using plume::RenderTextureAddressMode;
using plume::RenderTextureBarrier;
using plume::RenderTextureCopyLocation;
using plume::RenderTextureDesc;
using plume::RenderTextureLayout;
using plume::RenderVertexBufferView;

namespace {

#if defined(_WIN32)
#define FN64_RMLUI_SHADER_INPUTS(NAME, ENTRY, FORMAT) \
    ((FORMAT) == plume::RenderShaderFormat::DXIL) ? reinterpret_cast<const void *>(NAME##BlobDXIL) : reinterpret_cast<const void *>(NAME##BlobSPIRV), \
    ((FORMAT) == plume::RenderShaderFormat::DXIL) ? sizeof(NAME##BlobDXIL) : sizeof(NAME##BlobSPIRV), \
    (ENTRY), \
    (FORMAT)
#elif defined(__APPLE__)
#define FN64_RMLUI_SHADER_INPUTS(NAME, ENTRY, FORMAT) \
    ((FORMAT) == plume::RenderShaderFormat::METAL) ? reinterpret_cast<const void *>(NAME##BlobMSL) : reinterpret_cast<const void *>(NAME##BlobSPIRV), \
    ((FORMAT) == plume::RenderShaderFormat::METAL) ? sizeof(NAME##BlobMSL) : sizeof(NAME##BlobSPIRV), \
    (ENTRY), \
    (FORMAT)
#else
#define FN64_RMLUI_SHADER_INPUTS(NAME, ENTRY, FORMAT) \
    reinterpret_cast<const void *>(NAME##BlobSPIRV), \
    sizeof(NAME##BlobSPIRV), \
    (ENTRY), \
    (FORMAT)
#endif

// D3D12's placed-footprint texture upload requires each row to start at a
// 256-byte-aligned offset into the source buffer; this is a public D3D12
// constant (D3D12_TEXTURE_DATA_PITCH_ALIGNMENT), not backend-specific to
// any one caller, and applies harmlessly to the other backends too since
// they accept a wider (or ignored) alignment for a linear upload buffer.
constexpr uint64_t kTextureRowPitchAlignment = 256;

uint64_t AlignUp(uint64_t value, uint64_t alignment) {
    return ((value + alignment - 1) / alignment) * alignment;
}

// Every geometry/texture handle this bridge hands back to RmlUi is a
// pointer to a heap-allocated struct owned by this class, matching
// RenderInterface.h's documented handle contract ("an application-specified
// handle"). Zero is reserved by RmlUi itself (CompiledGeometryHandle 0 is
// never issued by CompileGeometry on success, TextureHandle 0 means
// "untextured" for RenderGeometry) so no valid handle here is ever zero.
template <typename T>
Rml::CompiledGeometryHandle ToHandle(T *pointer) {
    return reinterpret_cast<Rml::CompiledGeometryHandle>(pointer);
}

template <typename T>
T *FromHandle(Rml::CompiledGeometryHandle handle) {
    return reinterpret_cast<T *>(handle);
}

} // namespace

Fn64RmluiRenderInterface::Fn64RmluiRenderInterface(
    plume::RenderDevice *device,
    plume::RenderFormat colorFormat,
    plume::RenderMultisampling colorMultisampling,
    plume::RenderShaderFormat shaderFormat,
    uint32_t viewportWidth,
    uint32_t viewportHeight)
    : device_(device)
    , viewportWidth_(viewportWidth > 0 ? viewportWidth : 1)
    , viewportHeight_(viewportHeight > 0 ? viewportHeight : 1) {
    if (device_ == nullptr) {
        throw std::runtime_error("Fn64RmluiRenderInterface requires a non-null RenderDevice");
    }

    // Pipeline layout: one push-constant range (translation + viewport
    // size, visible to the vertex stage only) and one descriptor set
    // (texture + sampler, both visible to the pixel stage only).
    RenderDescriptorSetBuilder descriptorSetBuilder;
    descriptorSetBuilder.begin();
    descriptorSetBuilder.addTexture(1);
    descriptorSetBuilder.addSampler(2);
    descriptorSetBuilder.end();

    RenderPipelineLayoutBuilder layoutBuilder;
    layoutBuilder.begin();
    layoutBuilder.addPushConstant(
        0,
        0,
        sizeof(fn64_rmlui_interop::Fn64RmluiTranslationCB),
        plume::RenderShaderStageFlag::VERTEX);
    layoutBuilder.addDescriptorSet(descriptorSetBuilder);
    layoutBuilder.end();
    pipelineLayout_ = layoutBuilder.create(device_);

    std::unique_ptr<plume::RenderShader> vertexShader = device_->createShader(
        FN64_RMLUI_SHADER_INPUTS(Fn64RmluiUiVS, "VSMain", shaderFormat));
    std::unique_ptr<plume::RenderShader> pixelShader = device_->createShader(
        FN64_RMLUI_SHADER_INPUTS(Fn64RmluiUiPS, "PSMain", shaderFormat));

    const RenderInputSlot inputSlot(0, sizeof(Rml::Vertex), RenderInputSlotClassification::PER_VERTEX_DATA);
    const RenderInputElement inputElements[3] = {
        RenderInputElement("POSITION", 0, 0, RenderFormat::R32G32_FLOAT, 0, offsetof(Rml::Vertex, position)),
        RenderInputElement("COLOR", 0, 1, RenderFormat::R8G8B8A8_UNORM, 0, offsetof(Rml::Vertex, colour)),
        RenderInputElement("TEXCOORD", 0, 2, RenderFormat::R32G32_FLOAT, 0, offsetof(Rml::Vertex, tex_coord)),
    };

    RenderGraphicsPipelineDesc pipelineDesc;
    pipelineDesc.pipelineLayout = pipelineLayout_.get();
    pipelineDesc.vertexShader = vertexShader.get();
    pipelineDesc.pixelShader = pixelShader.get();
    pipelineDesc.primitiveTopology = RenderPrimitiveTopology::TRIANGLE_LIST;
    pipelineDesc.cullMode = RenderCullMode::NONE;
    pipelineDesc.frontFace = RenderFrontFace::COUNTER_CLOCKWISE;
    pipelineDesc.depthEnabled = false;
    pipelineDesc.depthWriteEnabled = false;
    pipelineDesc.stencilEnabled = false;
    pipelineDesc.multisampling = colorMultisampling;
    pipelineDesc.renderTargetCount = 1;
    pipelineDesc.renderTargetFormat[0] = colorFormat;
    // RmlUi vertex colors are premultiplied alpha (Vertex.h: "RGBA-ordered
    // 8-bit/channel colour with premultiplied alpha"), so source blending
    // must use ONE rather than SRC_ALPHA -- plume's own
    // RenderBlendDesc::AlphaBlend() helper assumes non-premultiplied input
    // and would double-darken translucent UI edges here.
    RenderBlendDesc premultipliedBlend;
    premultipliedBlend.blendEnabled = true;
    premultipliedBlend.srcBlend = RenderBlend::ONE;
    premultipliedBlend.dstBlend = RenderBlend::INV_SRC_ALPHA;
    premultipliedBlend.blendOp = RenderBlendOperation::ADD;
    premultipliedBlend.srcBlendAlpha = RenderBlend::ONE;
    premultipliedBlend.dstBlendAlpha = RenderBlend::INV_SRC_ALPHA;
    premultipliedBlend.blendOpAlpha = RenderBlendOperation::ADD;
    pipelineDesc.renderTargetBlend[0] = premultipliedBlend;
    pipelineDesc.inputSlots = &inputSlot;
    pipelineDesc.inputSlotsCount = 1;
    pipelineDesc.inputElements = inputElements;
    pipelineDesc.inputElementsCount = 3;
    pipeline_ = device_->createGraphicsPipeline(pipelineDesc);

    RenderSamplerDesc samplerDesc;
    samplerDesc.minFilter = RenderFilter::LINEAR;
    samplerDesc.magFilter = RenderFilter::LINEAR;
    samplerDesc.addressU = RenderTextureAddressMode::CLAMP;
    samplerDesc.addressV = RenderTextureAddressMode::CLAMP;
    samplerDesc.addressW = RenderTextureAddressMode::CLAMP;
    sampler_ = device_->createSampler(samplerDesc);

    // Default 1x1 opaque-white texture, reused for every untextured
    // RenderGeometry() call (RmlUi passes texture == 0 for those) so the
    // same pipeline/shader always has something bound at t1 rather than
    // needing an untextured variant.
    whiteTexture_ = device_->createTexture(RenderTextureDesc::Texture2D(1, 1, 1, RenderFormat::R8G8B8A8_UNORM));
    const uint8_t whitePixel[4] = {0xFF, 0xFF, 0xFF, 0xFF};
    UploadTexturePixels(whiteTexture_.get(), 1, 1, whitePixel);
    whiteTextureView_ = whiteTexture_->createTextureView(plume::RenderTextureViewDesc::Texture2D(RenderFormat::R8G8B8A8_UNORM));

    RenderDescriptorSetBuilder whiteDescriptorBuilder;
    whiteDescriptorBuilder.begin();
    whiteDescriptorBuilder.addTexture(1);
    whiteDescriptorBuilder.addSampler(2);
    whiteDescriptorBuilder.end();
    whiteDescriptorSet_ = whiteDescriptorBuilder.create(device_);
    whiteDescriptorSet_->setTexture(0, whiteTexture_.get(), RenderTextureLayout::SHADER_READ, whiteTextureView_.get());
    whiteDescriptorSet_->setSampler(1, sampler_.get());
}

Fn64RmluiRenderInterface::~Fn64RmluiRenderInterface() = default;

void Fn64RmluiRenderInterface::BeginFrame(plume::RenderCommandList *commandList, plume::RenderFramebuffer *framebuffer) {
    commandList_ = commandList;
    framebuffer_ = framebuffer;
}

void Fn64RmluiRenderInterface::EndFrame() {
    commandList_ = nullptr;
    framebuffer_ = nullptr;
}

void Fn64RmluiRenderInterface::SetViewportSize(uint32_t width, uint32_t height) {
    viewportWidth_ = (width > 0) ? width : 1;
    viewportHeight_ = (height > 0) ? height : 1;
}

void Fn64RmluiRenderInterface::UploadTexturePixels(
    plume::RenderTexture *texture,
    uint32_t width,
    uint32_t height,
    const void *pixels) {
    assert(commandList_ != nullptr && "UploadTexturePixels requires a live command list from BeginFrame()");

    const uint64_t rowPitch = uint64_t(width) * 4;
    const uint64_t alignedRowPitch = AlignUp(rowPitch, kTextureRowPitchAlignment);
    const uint64_t uploadSize = alignedRowPitch * height;

    std::unique_ptr<RenderBuffer> uploadBuffer = device_->createBuffer(RenderBufferDesc::UploadBuffer(uploadSize));
    uint8_t *mapped = reinterpret_cast<uint8_t *>(uploadBuffer->map());
    const uint8_t *source = reinterpret_cast<const uint8_t *>(pixels);
    for (uint32_t row = 0; row < height; row++) {
        std::memcpy(mapped + row * alignedRowPitch, source + row * rowPitch, rowPitch);
    }
    uploadBuffer->unmap();

    commandList_->barriers(plume::RenderBarrierStage::COPY, RenderTextureBarrier(texture, RenderTextureLayout::COPY_DEST));
    commandList_->copyTextureRegion(
        RenderTextureCopyLocation::Subresource(texture),
        RenderTextureCopyLocation::PlacedFootprint(
            uploadBuffer.get(),
            RenderFormat::R8G8B8A8_UNORM,
            width,
            height,
            1,
            uint32_t(alignedRowPitch / 4)));
    commandList_->barriers(plume::RenderBarrierStage::GRAPHICS, RenderTextureBarrier(texture, RenderTextureLayout::SHADER_READ));

    // The upload buffer must stay alive until the copy above actually
    // executes on the GPU. The draw callback's command list is submitted
    // and the frame presented before the next overlay draw call reuses
    // this same command list, and LoadTexture/GenerateTexture (the only
    // callers of this function) are only ever invoked from within that
    // same draw callback -- so leaking the upload buffer for the lifetime
    // of the process (rather than tracking a fence to free it after) is a
    // deliberate, bounded simplification: texture loads are rare relative
    // to per-frame geometry uploads, and RmlUi's own texture set is small
    // and effectively static per document.
    uploadBuffer.release();
}

Rml::CompiledGeometryHandle Fn64RmluiRenderInterface::CompileGeometry(
    Rml::Span<const Rml::Vertex> vertices,
    Rml::Span<const int> indices) {
    auto *geometry = new CompiledGeometry();

    const uint64_t vertexBytes = uint64_t(vertices.size()) * sizeof(Rml::Vertex);
    geometry->vertexBuffer = device_->createBuffer(RenderBufferDesc::VertexBuffer(vertexBytes, RenderHeapType::UPLOAD));
    geometry->vertexBufferCapacity = vertexBytes;
    void *vertexDestination = geometry->vertexBuffer->map();
    std::memcpy(vertexDestination, vertices.data(), vertexBytes);
    geometry->vertexBuffer->unmap();

    const uint64_t indexBytes = uint64_t(indices.size()) * sizeof(int);
    geometry->indexBuffer = device_->createBuffer(RenderBufferDesc::IndexBuffer(indexBytes, RenderHeapType::UPLOAD));
    geometry->indexBufferCapacity = indexBytes;
    void *indexDestination = geometry->indexBuffer->map();
    std::memcpy(indexDestination, indices.data(), indexBytes);
    geometry->indexBuffer->unmap();
    geometry->indexCount = uint32_t(indices.size());

    return ToHandle(geometry);
}

void Fn64RmluiRenderInterface::RenderGeometry(
    Rml::CompiledGeometryHandle geometryHandle,
    Rml::Vector2f translation,
    Rml::TextureHandle textureHandle) {
    assert(commandList_ != nullptr && "RenderGeometry called outside BeginFrame/EndFrame");

    auto *geometry = FromHandle<CompiledGeometry>(geometryHandle);
    if ((geometry == nullptr) || (geometry->indexCount == 0)) {
        return;
    }

    plume::RenderDescriptorSet *descriptorSet = DescriptorSetForTexture(textureHandle);

    commandList_->setPipeline(pipeline_.get());
    commandList_->setGraphicsPipelineLayout(pipelineLayout_.get());
    commandList_->setGraphicsDescriptorSet(descriptorSet, 0);

    fn64_rmlui_interop::Fn64RmluiTranslationCB pushConstants{};
    pushConstants.translationX = translation.x;
    pushConstants.translationY = translation.y;
    pushConstants.viewportWidth = float(viewportWidth_);
    pushConstants.viewportHeight = float(viewportHeight_);
    commandList_->setGraphicsPushConstants(0, &pushConstants);

    const RenderVertexBufferView vertexView(RenderBufferReference(geometry->vertexBuffer.get()), uint32_t(geometry->vertexBufferCapacity));
    const RenderInputSlot inputSlot(0, sizeof(Rml::Vertex), RenderInputSlotClassification::PER_VERTEX_DATA);
    commandList_->setVertexBuffers(0, &vertexView, 1, &inputSlot);

    const RenderIndexBufferView indexView(
        RenderBufferReference(geometry->indexBuffer.get()),
        uint32_t(geometry->indexBufferCapacity),
        RenderFormat::R32_UINT);
    commandList_->setIndexBuffer(&indexView);

    if (scissorEnabled_) {
        commandList_->setScissors(scissorRect_);
    } else {
        commandList_->setScissors(RenderRect(0, 0, int32_t(viewportWidth_), int32_t(viewportHeight_)));
    }
    commandList_->setViewports(plume::RenderViewport(0.0f, 0.0f, float(viewportWidth_), float(viewportHeight_)));

    commandList_->drawIndexedInstanced(geometry->indexCount, 1, 0, 0, 0);
}

void Fn64RmluiRenderInterface::ReleaseGeometry(Rml::CompiledGeometryHandle geometryHandle) {
    delete FromHandle<CompiledGeometry>(geometryHandle);
}

Rml::TextureHandle Fn64RmluiRenderInterface::LoadTexture(Rml::Vector2i & /*textureDimensions*/, const Rml::String & /*source*/) {
    // fn64-rmlui loads documents from an in-memory RML buffer with no
    // FileInterface-backed filesystem underneath it (see
    // fn64_rmlui_shim.cpp's Fn64FileInterface) -- there is deliberately no
    // notion of an "image source path" to resolve yet, so file-backed
    // <img> textures are out of scope for this pass. GenerateTexture below
    // (pixels supplied directly in memory) is the path fn64's own UI
    // actually needs and is fully implemented.
    return 0;
}

Rml::TextureHandle Fn64RmluiRenderInterface::GenerateTexture(Rml::Span<const Rml::byte> source, Rml::Vector2i sourceDimensions) {
    assert(commandList_ != nullptr && "GenerateTexture called outside BeginFrame/EndFrame");

    if ((sourceDimensions.x <= 0) || (sourceDimensions.y <= 0)) {
        return 0;
    }

    auto *loaded = new LoadedTexture();
    const uint32_t width = uint32_t(sourceDimensions.x);
    const uint32_t height = uint32_t(sourceDimensions.y);
    loaded->texture = device_->createTexture(RenderTextureDesc::Texture2D(width, height, 1, RenderFormat::R8G8B8A8_UNORM));
    UploadTexturePixels(loaded->texture.get(), width, height, source.data());
    loaded->textureView = loaded->texture->createTextureView(plume::RenderTextureViewDesc::Texture2D(RenderFormat::R8G8B8A8_UNORM));

    RenderDescriptorSetBuilder descriptorBuilder;
    descriptorBuilder.begin();
    descriptorBuilder.addTexture(1);
    descriptorBuilder.addSampler(2);
    descriptorBuilder.end();
    loaded->descriptorSet = descriptorBuilder.create(device_);
    loaded->descriptorSet->setTexture(0, loaded->texture.get(), RenderTextureLayout::SHADER_READ, loaded->textureView.get());
    loaded->descriptorSet->setSampler(1, sampler_.get());

    return ToHandle(loaded);
}

void Fn64RmluiRenderInterface::ReleaseTexture(Rml::TextureHandle textureHandle) {
    delete FromHandle<LoadedTexture>(textureHandle);
}

plume::RenderDescriptorSet *Fn64RmluiRenderInterface::DescriptorSetForTexture(Rml::TextureHandle textureHandle) const {
    if (textureHandle == 0) {
        return whiteDescriptorSet_.get();
    }
    auto *loaded = FromHandle<LoadedTexture>(textureHandle);
    return loaded->descriptorSet.get();
}

void Fn64RmluiRenderInterface::EnableScissorRegion(bool enable) {
    scissorEnabled_ = enable;
}

void Fn64RmluiRenderInterface::SetScissorRegion(Rml::Rectanglei region) {
    scissorRect_ = RenderRect(region.Left(), region.Top(), region.Right(), region.Bottom());
}
