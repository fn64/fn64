cmake_minimum_required(VERSION 3.20)
project(fn64_rt64_shim LANGUAGES C CXX)

if(NOT DEFINED FN64_RT64_SOURCE_DIR)
    message(FATAL_ERROR "FN64_RT64_SOURCE_DIR must name the sibling MIT RT64 checkout")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(RT64_STATIC ON CACHE BOOL "Build only RT64's static render/HLE library" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "Keep RT64 and its dependencies static" FORCE)
set(NFD_BUILD_TESTS OFF CACHE BOOL "Do not build nativefiledialog tests" FORCE)
set(NFD_INSTALL OFF CACHE BOOL "Do not generate nativefiledialog install target" FORCE)
set(ZSTD_BUILD_PROGRAMS OFF CACHE BOOL "Do not build zstd tools" FORCE)
set(ZSTD_BUILD_TESTS OFF CACHE BOOL "Do not build zstd tests" FORCE)
set(PLUME_BUILD_EXAMPLES OFF CACHE BOOL "Do not build plume examples" FORCE)

# EXCLUDE_FROM_ALL plus building only fn64_rt64_shim means CMake traverses
# RT64's `rt64` static target and its render dependencies, not its standalone
# texture tools. This source tree defines no mupen plugin target; its GPL
# mupen64plus subtree is therefore neither compiled nor linked.
add_subdirectory("${FN64_RT64_SOURCE_DIR}" rt64-build EXCLUDE_FROM_ALL)

# The pinned S2DEX implementation loads the public uObjTxtr prefix for
# G_OBJ_LDTX_RECT and then unconditionally asserts instead of drawing the
# documented uObjSprite tail. Stage one deliberately bounded implementation:
# axis-aligned, point-filtered RGBA16 block loads at 1:1, with no object-mode
# corrections or flips. Every broader shape is rejected before the texture
# load mutates RT64 state. The full-file digest and exact replacement contexts
# make a pin update a mandatory review.
set(FN64_RT64_S2DEX_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/gbi/rt64_gbi_s2dex.cpp")
file(SHA256 "${FN64_RT64_S2DEX_SOURCE}" FN64_RT64_S2DEX_SHA256)
if(NOT FN64_RT64_S2DEX_SHA256 STREQUAL
   "7c8e779092eb7e2ddc8794694c0c04176a3e29958dc288952d577750673acc3f")
    message(FATAL_ERROR
        "RT64 S2DEX source changed; review the fn64 object-rectangle overlay")
endif()
file(READ "${FN64_RT64_S2DEX_SOURCE}" FN64_RT64_S2DEX_PATCHED)
set(FN64_RT64_S2DEX_INCLUDE_ORIGINAL [=[#include "rt64_gbi_rdp.h"

//#define LOG_BGRECT_METHODS]=])
set(FN64_RT64_S2DEX_INCLUDE_PATCHED [=[#include "rt64_gbi_rdp.h"
#include "hle/rt64_interpreter.h"

#include <cstddef>
#include <cstring>
#include <stdexcept>
#include <type_traits>

//#define LOG_BGRECT_METHODS]=])
string(FIND
    "${FN64_RT64_S2DEX_PATCHED}"
    "${FN64_RT64_S2DEX_INCLUDE_ORIGINAL}"
    FN64_RT64_S2DEX_INCLUDE_OFFSET)
if(FN64_RT64_S2DEX_INCLUDE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 S2DEX include context changed; review the fn64 object-rectangle overlay")
endif()
string(REPLACE
    "${FN64_RT64_S2DEX_INCLUDE_ORIGINAL}"
    "${FN64_RT64_S2DEX_INCLUDE_PATCHED}"
    FN64_RT64_S2DEX_PATCHED
    "${FN64_RT64_S2DEX_PATCHED}")
set(FN64_RT64_S2DEX_NAMESPACE_ORIGINAL [=[namespace RT64 {
    namespace GBI_S2DEX {
        void objRenderMode(State *state, DisplayList **dl) {]=])
set(FN64_RT64_S2DEX_NAMESPACE_PATCHED [=[namespace RT64 {
    namespace GBI_S2DEX {
#pragma pack(push, 1)
        // RT64 stores the native-word-swapped RDRAM image, so each public
        // uObjSprite word is declared in its already-swapped host order.
        struct Fn64ObjSprite {
            uint16_t scaleW;
            int16_t objX;
            uint16_t paddingX;
            uint16_t imageW;
            uint16_t scaleH;
            int16_t objY;
            uint16_t paddingY;
            uint16_t imageH;
            uint16_t imageAdrs;
            uint16_t imageStride;
            uint8_t imageFlags;
            uint8_t imagePal;
            uint8_t imageSiz;
            uint8_t imageFmt;
        };
#pragma pack(pop)

        static_assert(sizeof(Fn64ObjSprite) == 24);
        static_assert(offsetof(Fn64ObjSprite, scaleW) == 0);
        static_assert(offsetof(Fn64ObjSprite, objX) == 2);
        static_assert(offsetof(Fn64ObjSprite, imageW) == 6);
        static_assert(offsetof(Fn64ObjSprite, objY) == 10);
        static_assert(offsetof(Fn64ObjSprite, imageH) == 14);
        static_assert(offsetof(Fn64ObjSprite, imageStride) == 18);
        static_assert(offsetof(Fn64ObjSprite, imageFmt) == 23);
        static_assert(std::is_trivially_copyable_v<uObjTxtr>);
        static_assert(std::is_trivially_copyable_v<Fn64ObjSprite>);

        [[noreturn]] void rejectFn64ObjLoadTxRect(const char *reason) {
            throw std::runtime_error(
                std::string("S2DEX2 G_OBJ_LDTX_RECT unsupported by bounded fn64 slice: ") +
                reason);
        }

        uint32_t resolveFn64ObjRdramSpan(
            State *state,
            uint32_t segmentedAddress,
            uint32_t byteCount,
            const char *segmentReason,
            const char *alignmentReason,
            const char *rangeReason)
        {
            // Public inputs are a 16-entry segmented address or KSEG0/KSEG1.
            // RT64's general helper deliberately aliases other upper bytes and
            // masks DMA alignment/address bits; this bounded path must reject
            // those aliases before it reads or mutates anything.
            const uint32_t addressClass = segmentedAddress >> 24;
            const bool directPhysical =
                (addressClass == 0x80U) || (addressClass == 0xA0U);
            if (!directPhysical && (addressClass > 0x0FU)) {
                rejectFn64ObjLoadTxRect(segmentReason);
            }
            const uint32_t offset = segmentedAddress & 0x00FFFFFFU;
            const uint64_t resolved = directPhysical
                ? uint64_t(offset)
                : uint64_t(state->rsp->segments[addressClass]) + uint64_t(offset);
            if ((resolved & 7U) != 0U) {
                rejectFn64ObjLoadTxRect(alignmentReason);
            }
            if ((resolved > uint64_t(RDRAMSize)) ||
                (uint64_t(byteCount) > (uint64_t(RDRAMSize) + 1U - resolved))) {
                rejectFn64ObjLoadTxRect(rangeReason);
            }
            return uint32_t(resolved);
        }

        void doFn64ObjLoadTxtr(
            State *state,
            const uObjTxtr &texture,
            uint32_t imageAddress)
        {
            const int32_t previousLoad = state->rsp->S2D.data_02AE;
            const uint32_t sid = uint8_t(texture.sid);
            const uint32_t status = state->rsp->S2D.statuses[sid / 4U];
            if ((status & texture.mask) == texture.flag) {
                return;
            }
            state->rsp->S2D.statuses[sid / 4U] =
                (status & ~texture.mask) | (texture.flag & texture.mask);
            state->rsp->S2D.data_02AE = -127;
            if (previousLoad == -127) {
                // Public S2DEX would emit a tile sync; RT64 does not need it.
            }

            const uint32_t textureWords = texture.val1;
            state->rdp->setTextureImage(
                G_IM_FMT_RGBA,
                G_IM_SIZ_16b,
                textureWords + 1U,
                imageAddress);
            const uint32_t typeMask = uint32_t(int32_t(int8_t(texture.type >> 16)));
            const uint32_t setTileWord =
                texture.tmem |
                (((textureWords + 1U) & typeMask) << 7) |
                (uint32_t(uint8_t(texture.type >> 8)) << 16);
            state->rdp->setTile(
                G_TX_LOADTILE,
                (setTileWord >> 21) & 7U,
                (setTileWord >> 19) & 3U,
                (setTileWord >> 9) & 0x1FFU,
                setTileWord & 0x1FFU,
                0,
                G_TX_WRAP | G_TX_NOMIRROR,
                G_TX_WRAP | G_TX_NOMIRROR,
                G_TX_NOMASK,
                G_TX_NOMASK,
                G_TX_NOLOD,
                G_TX_NOLOD);
            if (previousLoad >= 0) {
                // Public S2DEX would emit a load sync; RT64 does not need it.
            }
            const uint32_t loadWord =
                ((textureWords << 14) | texture.val2) & 0x00FFFFFFU;
            state->rdp->loadBlock(
                G_TX_LOADTILE,
                0,
                0,
                (loadWord >> 12) & 0xFFFU,
                loadWord & 0xFFFU);
        }

        void doFn64ObjLoadTxRect(State *state, const uint8_t *objectBytes) {
            uObjTxtr texture{};
            Fn64ObjSprite sprite{};
            std::memcpy(&texture, objectBytes, sizeof(texture));
            std::memcpy(
                &sprite,
                objectBytes + sizeof(texture),
                sizeof(sprite));
            if ((state->ext.interpreter == nullptr) ||
                (state->ext.interpreter->hleGBI == nullptr) ||
                (state->ext.interpreter->hleGBI->ucode != GBIUCode::S2DEX2)) {
                rejectFn64ObjLoadTxRect("active microcode is not S2DEX2");
            }
            if (texture.type != 0x00001033U) {
                rejectFn64ObjLoadTxRect("texture command is not exact public G_OBJLT_TXTRBLOCK");
            }
            const uint32_t sid = texture.sid;
            if ((sid > 12U) || ((sid & 3U) != 0U)) {
                rejectFn64ObjLoadTxRect("texture status sid is not public 0, 4, 8, or 12");
            }
            if (state->rsp->objRenderMode != 0U) {
                rejectFn64ObjLoadTxRect("objRenderMode is not zero");
            }
            if ((state->rdp->otherMode.textFilt() != G_TF_POINT) ||
                (state->rdp->otherMode.cycleType() != G_CYC_1CYCLE)) {
                rejectFn64ObjLoadTxRect("RDP state is not point-filtered one-cycle mode");
            }
            if ((state->rdp->otherMode.textLOD() != G_TL_TILE) ||
                (state->rdp->otherMode.textDetail() != G_TD_CLAMP) ||
                (state->rdp->otherMode.textLUT() != G_TT_NONE) ||
                (state->rdp->otherMode.textPersp() != G_TP_NONE)) {
                rejectFn64ObjLoadTxRect("RDP sampler state is not tile LOD, clamp detail, no TLUT, and no perspective");
            }
            if ((sprite.paddingX != 0U) || (sprite.paddingY != 0U)) {
                rejectFn64ObjLoadTxRect("uObjSprite reserved padding is nonzero");
            }
            if ((sprite.scaleW != 1024U) || (sprite.scaleH != 1024U)) {
                rejectFn64ObjLoadTxRect("scale is not exact 1:1 u5.10");
            }
            if ((sprite.imageW == 0U) || (sprite.imageH == 0U) ||
                ((sprite.imageW & 31U) != 0U) || ((sprite.imageH & 31U) != 0U)) {
                rejectFn64ObjLoadTxRect("image dimensions are not positive whole texels");
            }
            if ((sprite.imageFmt != G_IM_FMT_RGBA) ||
                (sprite.imageSiz != G_IM_SIZ_16b) || (sprite.imagePal != 0U)) {
                rejectFn64ObjLoadTxRect("image is not non-paletted RGBA16");
            }
            if (sprite.imageFlags != 0U) {
                rejectFn64ObjLoadTxRect("image flip flags are nonzero");
            }
            if ((sprite.imageStride == 0U) || (sprite.imageStride > 0x1FFU) ||
                (sprite.imageAdrs > 0x1FFU)) {
                rejectFn64ObjLoadTxRect("TMEM stride or address is outside the RDP tile range");
            }
            const uint32_t widthTexels = uint32_t(sprite.imageW) >> 5;
            const uint32_t heightTexels = uint32_t(sprite.imageH) >> 5;
            const uint32_t blockWords = uint32_t(texture.val1) + 1U;
            if ((widthTexels * 2U != uint32_t(sprite.imageStride) * 8U) ||
                (blockWords != uint32_t(sprite.imageStride) * heightTexels) ||
                (texture.tmem != sprite.imageAdrs)) {
                rejectFn64ObjLoadTxRect("block tsize, TMEM origin, and sprite stride/extent disagree");
            }
            if (((2048U % sprite.imageStride) != 0U) ||
                (texture.val2 != (2048U / sprite.imageStride))) {
                rejectFn64ObjLoadTxRect("block tline does not match the exact sprite stride");
            }
            if ((uint32_t(sprite.imageAdrs) + blockWords) > 512U) {
                rejectFn64ObjLoadTxRect("block load exceeds physical TMEM");
            }
            const uint32_t blockBytes = blockWords * 8U;
            const uint32_t imageAddress = resolveFn64ObjRdramSpan(
                state,
                texture.image,
                blockBytes,
                "block source uses non-public segment bits",
                "block source is not public 8-byte aligned",
                "block source range escapes physical RDRAM");
            const uint32_t widthQuarter = uint32_t(sprite.imageW) >> 3;
            const uint32_t heightQuarter = uint32_t(sprite.imageH) >> 3;
            if ((widthQuarter > 0x1000U) || (heightQuarter > 0x1000U)) {
                rejectFn64ObjLoadTxRect("image extent exceeds the RDP tile coordinate range");
            }

            doFn64ObjLoadTxtr(state, texture, imageAddress);
            RDP *rdp = state->rdp.get();
            rdp->setTile(
                G_TX_RENDERTILE, G_IM_FMT_RGBA, G_IM_SIZ_16b,
                sprite.imageStride, sprite.imageAdrs, 0,
                G_TX_CLAMP | G_TX_NOMIRROR, G_TX_CLAMP | G_TX_NOMIRROR,
                G_TX_NOMASK, G_TX_NOMASK, G_TX_NOLOD, G_TX_NOLOD);
            rdp->setTileSize(
                G_TX_RENDERTILE, 0, 0,
                uint16_t(widthQuarter - 4U), uint16_t(heightQuarter - 4U));
            state->startSpriteCommand(0);
            rdp->drawTexRect(
                sprite.objX, sprite.objY,
                int32_t(sprite.objX) + int32_t(widthQuarter),
                int32_t(sprite.objY) + int32_t(heightQuarter),
                G_TX_RENDERTILE, 0, 0,
                int16_t(sprite.scaleW), int16_t(sprite.scaleH), false);
            state->endSpriteCommand();
        }

        void objRenderMode(State *state, DisplayList **dl) {]=])
string(FIND
    "${FN64_RT64_S2DEX_PATCHED}"
    "${FN64_RT64_S2DEX_NAMESPACE_ORIGINAL}"
    FN64_RT64_S2DEX_NAMESPACE_OFFSET)
if(FN64_RT64_S2DEX_NAMESPACE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 S2DEX namespace context changed; review the fn64 object-rectangle overlay")
endif()
string(REPLACE
    "${FN64_RT64_S2DEX_NAMESPACE_ORIGINAL}"
    "${FN64_RT64_S2DEX_NAMESPACE_PATCHED}"
    FN64_RT64_S2DEX_PATCHED
    "${FN64_RT64_S2DEX_PATCHED}")
string(CONCAT FN64_RT64_S2DEX_RECT_ORIGINAL
    [=[            // Load the struct from rdram
            readS2DStruct(state, (*dl)->w1, ((*dl)->w0 & 0xFFFFFF) + 1);]=]
    "\n            \n"
    [=[            // Execute the texture load
            const uObjTxSprite* obj = (uObjTxSprite*)state->rsp->S2D.struct_buffer.data();
            doLoadTxtr(state, &obj->txtr);]=]
    "\n            \n"
    [=[            // Execute the rect draw
            // TODO call doObjRectangle(state, &obj->sprite) when it's implemented
            assert(false);]=])
set(FN64_RT64_S2DEX_RECT_PATCHED [=[            // The public compound structure is exactly 48 bytes. Reject a
            // short DMA before reading a stale sprite tail from the persistent buffer.
            if (((*dl)->w0 & 0x00FFFFFFU) != 0x2FU) {
                rejectFn64ObjLoadTxRect("DMA length low24 is not exact public 0x2f");
            }
            const uint32_t objectAddress = resolveFn64ObjRdramSpan(
                state,
                (*dl)->w1,
                0x30U,
                "compound source uses non-public segment bits",
                "compound source is not public 8-byte aligned",
                "compound source range escapes physical RDRAM");
            std::memcpy(
                state->rsp->S2D.struct_buffer.data(),
                state->fromRDRAM(objectAddress),
                0x30U);

            // Validate the public bounded contract before either half mutates
            // RT64 state, then execute the documented load followed by draw.
            doFn64ObjLoadTxRect(state, state->rsp->S2D.struct_buffer.data());]=])
string(FIND
    "${FN64_RT64_S2DEX_PATCHED}"
    "${FN64_RT64_S2DEX_RECT_ORIGINAL}"
    FN64_RT64_S2DEX_RECT_OFFSET)
if(FN64_RT64_S2DEX_RECT_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 objLoadTxRect context changed; review the fn64 object-rectangle overlay")
endif()
string(REPLACE
    "${FN64_RT64_S2DEX_RECT_ORIGINAL}"
    "${FN64_RT64_S2DEX_RECT_PATCHED}"
    FN64_RT64_S2DEX_PATCHED
    "${FN64_RT64_S2DEX_PATCHED}")
set(FN64_RT64_S2DEX_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_gbi_s2dex.cpp")
file(WRITE
    "${FN64_RT64_S2DEX_PATCHED_SOURCE}"
    "${FN64_RT64_S2DEX_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_S2DEX_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_S2DEX_PATCHED_SOURCE}")
target_include_directories(rt64 PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src/gbi")

# Compile fn64's bounded post-gamma VI shader as a source-identity overlay.
# RT64's shader helper functions are defined in the child directory; import
# their directory-local compiler variables into this caller before invoking
# them against the fn64-owned source staged beside this file.
foreach(FN64_SHADER_VARIABLE
        DXC DXC_DXIL_OPTS DXC_SPV_OPTS DXC_LB_OPTS DXC_PS_OPTS SPIRVCROSS)
    get_directory_property(
        FN64_IMPORTED_${FN64_SHADER_VARIABLE}
        DIRECTORY "${FN64_RT64_SOURCE_DIR}"
        DEFINITION ${FN64_SHADER_VARIABLE})
    set(${FN64_SHADER_VARIABLE} "${FN64_IMPORTED_${FN64_SHADER_VARIABLE}}")
endforeach()
target_include_directories(rt64 PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
build_shader(
    rt64
    "fn64_rt64_video_interface_ps.hlsl"
    "${DXC_PS_OPTS}"
    "src/shaders/Fn64VideoInterfacePSRegular.hlsl"
    "-I${CMAKE_CURRENT_SOURCE_DIR}")
build_shader(
    rt64
    "fn64_rt64_video_interface_ps.hlsl"
    "${DXC_PS_OPTS}"
    "src/shaders/Fn64VideoInterfacePSPixel.hlsl"
    "-D" "PIXEL_ANTIALIASING"
    "-I${CMAKE_CURRENT_SOURCE_DIR}")
set(FN64_RT64_VI_SHADER_OUTPUTS
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSRegular.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSPixel.hlsl.spirv.c")
if(APPLE)
    list(APPEND FN64_RT64_VI_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSRegular.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSPixel.hlsl.metal.c")
elseif(WIN32)
    list(APPEND FN64_RT64_VI_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSRegular.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64VideoInterfacePSPixel.hlsl.dxil.c")
endif()
# CMake custom-command visibility is directory-scoped while `rt64` belongs to
# the child directory. This explicit parent target publishes the generated
# shader objects before the child target compiles their C arrays.
add_custom_target(fn64_rt64_vi_shaders DEPENDS ${FN64_RT64_VI_SHADER_OUTPUTS})
add_dependencies(rt64 fn64_rt64_vi_shaders)

set(FN64_RT64_ORIGINAL_VI_SHADER_OBJECTS
    "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSRegular.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSPixel.hlsl.spirv.c")
if(APPLE)
    list(APPEND FN64_RT64_ORIGINAL_VI_SHADER_OBJECTS
        "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSRegular.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSPixel.hlsl.metal.c")
elseif(WIN32)
    list(APPEND FN64_RT64_ORIGINAL_VI_SHADER_OBJECTS
        "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSRegular.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/VideoInterfacePSPixel.hlsl.dxil.c")
endif()
set_source_files_properties(
    ${FN64_RT64_ORIGINAL_VI_SHADER_OBJECTS}
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)

# Compile one exact-source raster overlay into every native pixel-shader
# permutation selected by pinned RT64. The generated HLSL retains RT64's
# complete shader while routing one typed fragment-noise sample through the
# combiner, alpha compare, and fn64-owned pre-blend alpha quantizer.
set(FN64_RT64_RASTER_PS_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/shaders/RasterPS.hlsl")
file(SHA256
    "${FN64_RT64_RASTER_PS_SOURCE}"
    FN64_RT64_RASTER_PS_SHA256)
if(NOT FN64_RT64_RASTER_PS_SHA256 STREQUAL
   "957b283411550457573421015580f84ca750c68a050981dbc1a4fe2634507820")
    message(FATAL_ERROR
        "RT64 raster PS changed; review the fn64 RDP alpha-dither overlay")
endif()
file(READ
    "${FN64_RT64_RASTER_PS_SOURCE}"
    FN64_RT64_RASTER_PS_PATCHED)
set(FN64_RT64_RASTER_PS_INCLUDE_ORIGINAL [=[#include "Random.hlsli"
#include "TextureSampler.hlsli"]=])
set(FN64_RT64_RASTER_PS_INCLUDE_PATCHED [=[#include "Random.hlsli"
#include "TextureSampler.hlsli"
#include "fn64_rt64_raster_ps_overlay.hlsli"]=])
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "${FN64_RT64_RASTER_PS_INCLUDE_ORIGINAL}"
    FN64_RT64_RASTER_PS_INCLUDE_OFFSET)
if(FN64_RT64_RASTER_PS_INCLUDE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS include context changed; review the fn64 RDP alpha-dither overlay")
endif()
string(REPLACE
    "${FN64_RT64_RASTER_PS_INCLUDE_ORIGINAL}"
    "${FN64_RT64_RASTER_PS_INCLUDE_PATCHED}"
    FN64_RT64_RASTER_PS_PATCHED
    "${FN64_RT64_RASTER_PS_PATCHED}")
set(FN64_RT64_RASTER_PS_NOISE_SAMPLE_ORIGINAL [=[    int2 pixelPosSeed = floor(vertexPosition.xy);
    uint randomSeed = initRand(FrParams.frameCount, instanceIndex * pixelPosSeed.y * pixelPosSeed.x, 16); // TODO: Review seed.]=])
set(FN64_RT64_RASTER_PS_NOISE_SAMPLE_PATCHED [=[    int2 pixelPosSeed = floor(vertexPosition.xy);
    uint randomSeed = initRand(FrParams.frameCount, instanceIndex * pixelPosSeed.y * pixelPosSeed.x, 16); // TODO: Review seed.
    Fn64RdpFragmentNoiseSample fragmentNoise =
        Fn64RdpTakeFragmentNoiseSample(randomSeed);]=])
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "${FN64_RT64_RASTER_PS_NOISE_SAMPLE_ORIGINAL}"
    FN64_RT64_RASTER_PS_NOISE_SAMPLE_OFFSET)
if(FN64_RT64_RASTER_PS_NOISE_SAMPLE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS random-seed context changed; review the fn64 shared fragment-noise overlay")
endif()
string(REPLACE
    "${FN64_RT64_RASTER_PS_NOISE_SAMPLE_ORIGINAL}"
    "${FN64_RT64_RASTER_PS_NOISE_SAMPLE_PATCHED}"
    FN64_RT64_RASTER_PS_PATCHED
    "${FN64_RT64_RASTER_PS_PATCHED}")
set(FN64_RT64_RASTER_PS_COMBINER_NOISE_ORIGINAL
    "    ccInputs.noise = nextRand(randomSeed);")
set(FN64_RT64_RASTER_PS_COMBINER_NOISE_PATCHED
    "    ccInputs.noise = Fn64RdpFragmentNoiseUnitFloat(fragmentNoise);")
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "${FN64_RT64_RASTER_PS_COMBINER_NOISE_ORIGINAL}"
    FN64_RT64_RASTER_PS_COMBINER_NOISE_OFFSET)
if(FN64_RT64_RASTER_PS_COMBINER_NOISE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS combiner-noise context changed; review the fn64 shared fragment-noise overlay")
endif()
string(REPLACE
    "${FN64_RT64_RASTER_PS_COMBINER_NOISE_ORIGINAL}"
    "${FN64_RT64_RASTER_PS_COMBINER_NOISE_PATCHED}"
    FN64_RT64_RASTER_PS_PATCHED
    "${FN64_RT64_RASTER_PS_PATCHED}")
set(FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_ORIGINAL
    "        if (alphaCompareValue < nextRand(randomSeed)) {")
set(FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_PATCHED
    "        if (alphaCompareValue < Fn64RdpFragmentNoiseUnitFloat(fragmentNoise)) {")
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "${FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_ORIGINAL}"
    FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_OFFSET)
if(FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS alpha-compare-noise context changed; review the fn64 shared fragment-noise overlay")
endif()
string(REPLACE
    "${FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_ORIGINAL}"
    "${FN64_RT64_RASTER_PS_ALPHA_COMPARE_NOISE_PATCHED}"
    FN64_RT64_RASTER_PS_PATCHED
    "${FN64_RT64_RASTER_PS_PATCHED}")
set(FN64_RT64_RASTER_PS_BLEND_ORIGINAL [=[    // Add the blender if it can be replicated with simple emulation.
    Blender::Inputs blInputs;]=])
set(FN64_RT64_RASTER_PS_BLEND_PATCHED [=[    // Alpha compare and coverage intentionally observe the original combiner
    // alpha. Only the combiner input to blending receives this bounded policy;
    // shade, fog, alpha-compare, and coverage alpha remain unchanged.
    combinerColor.a = Fn64RdpAlphaDitherCombinerAlpha(
        otherMode, combinerColor.a, floor(vertexPosition.xy), fragmentNoise);

    // Add the blender if it can be replicated with simple emulation.
    Blender::Inputs blInputs;]=])
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "${FN64_RT64_RASTER_PS_BLEND_ORIGINAL}"
    FN64_RT64_RASTER_PS_BLEND_OFFSET)
if(FN64_RT64_RASTER_PS_BLEND_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS blend context changed; review the fn64 RDP alpha-dither overlay")
endif()
string(REPLACE
    "${FN64_RT64_RASTER_PS_BLEND_ORIGINAL}"
    "${FN64_RT64_RASTER_PS_BLEND_PATCHED}"
    FN64_RT64_RASTER_PS_PATCHED
    "${FN64_RT64_RASTER_PS_PATCHED}")
string(FIND
    "${FN64_RT64_RASTER_PS_PATCHED}"
    "nextRand(randomSeed)"
    FN64_RT64_RASTER_PS_UNSHARED_NOISE_OFFSET)
if(NOT FN64_RT64_RASTER_PS_UNSHARED_NOISE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster PS retains an unshared fragment-noise draw")
endif()
set(FN64_RT64_RASTER_PS_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_raster_ps.hlsl")
file(WRITE
    "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
    "${FN64_RT64_RASTER_PS_PATCHED}")

set(FN64_RT64_RASTER_PS_OUTPUT_DIRECTORY
    "${CMAKE_BINARY_DIR}/src/shaders")
file(MAKE_DIRECTORY "${FN64_RT64_RASTER_PS_OUTPUT_DIRECTORY}")
set(FN64_RT64_RASTER_PS_INCLUDE_OPTIONS
    "-I${FN64_RT64_SOURCE_DIR}/src/shaders"
    "-I${CMAKE_CURRENT_SOURCE_DIR}")

# RT64's public helpers assume shader sources live under PROJECT_SOURCE_DIR.
# This absolute-input wrapper keeps the generated exact-source overlay in the
# build tree while retaining the same compiler options and output symbols.
function(fn64_rt64_build_raster_pixel_shader VARIANT)
    set(FN64_OUTPUT
        "${FN64_RT64_RASTER_PS_OUTPUT_DIRECTORY}/Fn64RdpDither${VARIANT}.hlsl")
    if(WIN32)
        build_shader_dxil_impl(
            rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
            "Fn64RdpDither${VARIANT}" "${FN64_OUTPUT}"
            ${DXC_PS_OPTS} ${ARGN} ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
    endif()
    if(APPLE)
        build_shader_msl_impl(
            rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
            "Fn64RdpDither${VARIANT}" "${FN64_OUTPUT}"
            ${DXC_PS_OPTS} ${ARGN} ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
    endif()
    build_shader_spirv_impl(
        rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
        "Fn64RdpDither${VARIANT}" "${FN64_OUTPUT}"
        ${DXC_PS_OPTS} ${ARGN} ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
endfunction()

function(fn64_rt64_build_raster_spec_shader VARIANT)
    set(FN64_OUTPUT
        "${FN64_RT64_RASTER_PS_OUTPUT_DIRECTORY}/Fn64RdpDither${VARIANT}.hlsl")
    build_shader_spirv_impl(
        rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
        "Fn64RdpDither${VARIANT}" "${FN64_OUTPUT}"
        ${DXC_PS_OPTS} ${ARGN} ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
    if(APPLE)
        build_shader_msl_impl(
            rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
            "Fn64RdpDither${VARIANT}" "${FN64_OUTPUT}"
            ${DXC_PS_OPTS} ${ARGN} ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
    endif()
endfunction()

if(WIN32)
    foreach(FN64_RT64_RASTER_PS_LIBRARY_VARIANT Library LibraryMS)
        set(FN64_RT64_RASTER_PS_LIBRARY_OPTIONS)
        if(FN64_RT64_RASTER_PS_LIBRARY_VARIANT STREQUAL "LibraryMS")
            list(APPEND FN64_RT64_RASTER_PS_LIBRARY_OPTIONS "-D" "MULTISAMPLING")
        endif()
        build_shader_dxil_impl(
            rt64 "${FN64_RT64_RASTER_PS_PATCHED_SOURCE}"
            "Fn64RdpDither${FN64_RT64_RASTER_PS_LIBRARY_VARIANT}"
            "${FN64_RT64_RASTER_PS_OUTPUT_DIRECTORY}/Fn64RdpDither${FN64_RT64_RASTER_PS_LIBRARY_VARIANT}.hlsl"
            ${DXC_LB_OPTS}
            ${FN64_RT64_RASTER_PS_LIBRARY_OPTIONS}
            ${FN64_RT64_RASTER_PS_INCLUDE_OPTIONS})
    endforeach()
endif()
fn64_rt64_build_raster_pixel_shader(Dynamic "-D" "DYNAMIC_RENDER_PARAMS")
fn64_rt64_build_raster_pixel_shader(
    DynamicMS "-D" "DYNAMIC_RENDER_PARAMS" "-D" "MULTISAMPLING")
fn64_rt64_build_raster_spec_shader(
    SpecConstant "-D" "SPEC_CONSTANT_RENDER_PARAMS")
fn64_rt64_build_raster_spec_shader(
    SpecConstantMS "-D" "SPEC_CONSTANT_RENDER_PARAMS" "-D" "MULTISAMPLING")
fn64_rt64_build_raster_spec_shader(
    SpecConstantFlat "-D" "SPEC_CONSTANT_RENDER_PARAMS" "-D" "VERTEX_FLAT_COLOR")
fn64_rt64_build_raster_spec_shader(
    SpecConstantFlatMS
    "-D" "SPEC_CONSTANT_RENDER_PARAMS"
    "-D" "VERTEX_FLAT_COLOR"
    "-D" "MULTISAMPLING")

set(FN64_RT64_RASTER_PS_SHADER_OUTPUTS
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamic.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamicMS.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstant.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantMS.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantFlat.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantFlatMS.hlsl.spirv.c")
if(APPLE)
    list(APPEND FN64_RT64_RASTER_PS_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamic.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamicMS.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstant.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantMS.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantFlat.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherSpecConstantFlatMS.hlsl.metal.c")
elseif(WIN32)
    list(APPEND FN64_RT64_RASTER_PS_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamic.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherDynamicMS.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherLibrary.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RdpDitherLibraryMS.hlsl.dxil.c")
endif()
add_custom_target(
    fn64_rt64_raster_ps_shaders
    DEPENDS ${FN64_RT64_RASTER_PS_SHADER_OUTPUTS})
add_dependencies(rt64 fn64_rt64_raster_ps_shaders)

set(FN64_RT64_ORIGINAL_RASTER_PS_SHADER_OBJECTS
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamic.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamicMS.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstant.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantMS.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantFlat.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantFlatMS.hlsl.spirv.c")
if(APPLE)
    list(APPEND FN64_RT64_ORIGINAL_RASTER_PS_SHADER_OBJECTS
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamic.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamicMS.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstant.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantMS.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantFlat.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSSpecConstantFlatMS.hlsl.metal.c")
elseif(WIN32)
    list(APPEND FN64_RT64_ORIGINAL_RASTER_PS_SHADER_OBJECTS
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamic.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSDynamicMS.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSLibrary.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/RasterPSLibraryMS.hlsl.dxil.c")
endif()
set_source_files_properties(
    ${FN64_RT64_ORIGINAL_RASTER_PS_SHADER_OBJECTS}
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)

# Replace every raster-PS blob selected by the pinned raster shader with the
# corresponding fn64 overlay blob. Both complete source digests and exact
# token checks fail closed if the upstream selection graph changes.
set(FN64_RT64_RASTER_SHADER_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/render/rt64_raster_shader.cpp")
file(SHA256
    "${FN64_RT64_RASTER_SHADER_SOURCE}"
    FN64_RT64_RASTER_SHADER_SHA256)
if(NOT FN64_RT64_RASTER_SHADER_SHA256 STREQUAL
   "5e5de4bbd8a22192e857c277bae5af033b8606a6464aa73863d2efe0fd0a0d4d")
    message(FATAL_ERROR
        "RT64 raster shader changed; review every fn64 raster-PS blob replacement")
endif()
file(READ
    "${FN64_RT64_RASTER_SHADER_SOURCE}"
    FN64_RT64_RASTER_SHADER_PATCHED)
macro(fn64_rt64_replace_raster_shader_token ORIGINAL REPLACEMENT)
    string(FIND
        "${FN64_RT64_RASTER_SHADER_PATCHED}"
        "${ORIGINAL}"
        FN64_RT64_RASTER_SHADER_TOKEN_OFFSET)
    if(FN64_RT64_RASTER_SHADER_TOKEN_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "RT64 raster shader token ${ORIGINAL} changed; review every fn64 raster-PS blob replacement")
    endif()
    string(REPLACE
        "${ORIGINAL}"
        "${REPLACEMENT}"
        FN64_RT64_RASTER_SHADER_PATCHED
        "${FN64_RT64_RASTER_SHADER_PATCHED}")
endmacro()

foreach(FN64_RT64_RASTER_PS_VARIANT
        Dynamic DynamicMS
        SpecConstant SpecConstantMS SpecConstantFlat SpecConstantFlatMS)
    foreach(FN64_RT64_RASTER_PS_FORMAT SPIRV MSL)
        fn64_rt64_replace_raster_shader_token(
            "RasterPS${FN64_RT64_RASTER_PS_VARIANT}Blob${FN64_RT64_RASTER_PS_FORMAT}"
            "Fn64RdpDither${FN64_RT64_RASTER_PS_VARIANT}Blob${FN64_RT64_RASTER_PS_FORMAT}")
    endforeach()
endforeach()
foreach(FN64_RT64_RASTER_PS_VARIANT Dynamic DynamicMS Library LibraryMS)
    fn64_rt64_replace_raster_shader_token(
        "RasterPS${FN64_RT64_RASTER_PS_VARIANT}BlobDXIL"
        "Fn64RdpDither${FN64_RT64_RASTER_PS_VARIANT}BlobDXIL")
endforeach()
foreach(FN64_RT64_RASTER_PS_VARIANT
        Dynamic DynamicMS
        SpecConstant SpecConstantMS SpecConstantFlat SpecConstantFlatMS)
    foreach(FN64_RT64_RASTER_PS_FORMAT spirv metal)
        fn64_rt64_replace_raster_shader_token(
            "shaders/RasterPS${FN64_RT64_RASTER_PS_VARIANT}.hlsl.${FN64_RT64_RASTER_PS_FORMAT}.h"
            "shaders/Fn64RdpDither${FN64_RT64_RASTER_PS_VARIANT}.hlsl.${FN64_RT64_RASTER_PS_FORMAT}.h")
    endforeach()
endforeach()
foreach(FN64_RT64_RASTER_PS_VARIANT Dynamic DynamicMS Library LibraryMS)
    fn64_rt64_replace_raster_shader_token(
        "shaders/RasterPS${FN64_RT64_RASTER_PS_VARIANT}.hlsl.dxil.h"
        "shaders/Fn64RdpDither${FN64_RT64_RASTER_PS_VARIANT}.hlsl.dxil.h")
endforeach()

set(FN64_RT64_ORIGINAL_RASTER_PS_BLOBS
    RasterPSDynamicBlobDXIL RasterPSDynamicMSBlobDXIL
    RasterPSDynamicBlobSPIRV RasterPSDynamicMSBlobSPIRV
    RasterPSDynamicBlobMSL RasterPSDynamicMSBlobMSL
    RasterPSSpecConstantBlobSPIRV RasterPSSpecConstantMSBlobSPIRV
    RasterPSSpecConstantFlatBlobSPIRV RasterPSSpecConstantFlatMSBlobSPIRV
    RasterPSSpecConstantBlobMSL RasterPSSpecConstantMSBlobMSL
    RasterPSSpecConstantFlatBlobMSL RasterPSSpecConstantFlatMSBlobMSL
    RasterPSLibraryBlobDXIL RasterPSLibraryMSBlobDXIL)
foreach(FN64_RT64_ORIGINAL_RASTER_PS_BLOB
        IN LISTS FN64_RT64_ORIGINAL_RASTER_PS_BLOBS)
    string(FIND
        "${FN64_RT64_RASTER_SHADER_PATCHED}"
        "${FN64_RT64_ORIGINAL_RASTER_PS_BLOB}"
        FN64_RT64_ORIGINAL_RASTER_PS_BLOB_OFFSET)
    if(NOT FN64_RT64_ORIGINAL_RASTER_PS_BLOB_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "original raster PS blob remains selected: ${FN64_RT64_ORIGINAL_RASTER_PS_BLOB}")
    endif()
endforeach()

set(FN64_RT64_RASTER_SHADER_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_raster_shader.cpp")
file(WRITE
    "${FN64_RT64_RASTER_SHADER_PATCHED_SOURCE}"
    "${FN64_RT64_RASTER_SHADER_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_RASTER_SHADER_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_RASTER_SHADER_PATCHED_SOURCE}")

# Bind the replacement shader blobs and expanded push constants to the exact
# reviewed pinned MIT sources. A pin update must review this overlay rather
# than silently retaining old shader/library assumptions.
set(FN64_RT64_SHADER_LIBRARY_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/render/rt64_shader_library.cpp")
file(SHA256 "${FN64_RT64_SHADER_LIBRARY_SOURCE}" FN64_RT64_SHADER_LIBRARY_SHA256)
if(NOT FN64_RT64_SHADER_LIBRARY_SHA256 STREQUAL
   "72421375b232d5449f2f90f153c8452420084f43809c3234bc9727eddfde6ffd")
    message(FATAL_ERROR
        "RT64 shader library changed; review the fn64 VI filter overlay")
endif()
file(READ "${FN64_RT64_SHADER_LIBRARY_SOURCE}" FN64_RT64_SHADER_LIBRARY_PATCHED)
string(REPLACE
    "VideoInterfacePSRegular"
    "Fn64VideoInterfacePSRegular"
    FN64_RT64_SHADER_LIBRARY_PATCHED
    "${FN64_RT64_SHADER_LIBRARY_PATCHED}")
string(REPLACE
    "VideoInterfacePSPixel"
    "Fn64VideoInterfacePSPixel"
    FN64_RT64_SHADER_LIBRARY_PATCHED
    "${FN64_RT64_SHADER_LIBRARY_PATCHED}")
string(REPLACE
    "#include \"shared/rt64_video_interface.h\""
    "#include \"fn64_rt64_video_interface.h\""
    FN64_RT64_SHADER_LIBRARY_PATCHED
    "${FN64_RT64_SHADER_LIBRARY_PATCHED}")
set(FN64_RT64_SHADER_LIBRARY_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_shader_library.cpp")
file(WRITE
    "${FN64_RT64_SHADER_LIBRARY_PATCHED_SOURCE}"
    "${FN64_RT64_SHADER_LIBRARY_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_SHADER_LIBRARY_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_SHADER_LIBRARY_PATCHED_SOURCE}")

set(FN64_RT64_VI_RENDERER_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/render/rt64_vi_renderer.cpp")
file(SHA256 "${FN64_RT64_VI_RENDERER_SOURCE}" FN64_RT64_VI_RENDERER_SHA256)
if(NOT FN64_RT64_VI_RENDERER_SHA256 STREQUAL
   "0159b441fd4872ef30a49d7603f4570d8858084004c20dd6646f51039241fa7b")
    message(FATAL_ERROR
        "RT64 VI renderer changed; review the fn64 VI filter overlay")
endif()
file(READ "${FN64_RT64_VI_RENDERER_SOURCE}" FN64_RT64_VI_RENDERER_PATCHED)
string(REPLACE
    "#include \"shared/rt64_video_interface.h\""
    "#include \"fn64_rt64_video_interface.h\"\n\nextern \"C\" void fn64_rt64_vi_filter_constants(void *, interop::VideoInterfaceCB *);"
    FN64_RT64_VI_RENDERER_PATCHED
    "${FN64_RT64_VI_RENDERER_PATCHED}")
set(FN64_RT64_VI_CONSTANTS_ORIGINAL [=[        interop::VideoInterfaceCB pushConstants;
        pushConstants.videoResolution = computeHDSize(hlslpp::float2(p.vi->fbSize()), p.resolutionScale, p.downsamplingScale);
        pushConstants.textureResolution = { float(p.textureWidth), float(p.textureHeight) };
        pushConstants.gamma = p.vi->gamma();]=])
set(FN64_RT64_VI_CONSTANTS_PATCHED [=[        interop::VideoInterfaceCB pushConstants{};
        pushConstants.videoResolution = computeHDSize(hlslpp::float2(p.vi->fbSize()), p.resolutionScale, p.downsamplingScale);
        pushConstants.textureResolution = { float(p.textureWidth), float(p.textureHeight) };
        pushConstants.gamma = p.vi->gamma();
        pushConstants.coverageRange =
            (p.textureFormat == RenderFormat::R16G16B16A16_UNORM) ? 65535U : 255U;
        pushConstants.filtering = uint32_t(p.filtering);
        pushConstants.outputOriginX = uint32_t(std::max(0L, lround(viewport.x)));
        pushConstants.outputOriginY = uint32_t(std::max(0L, lround(viewport.y)));
        pushConstants.outputWidth = uint32_t(std::max(1L, lround(viewport.width)));
        fn64_rt64_vi_filter_constants(p.device, &pushConstants);]=])
string(FIND
    "${FN64_RT64_VI_RENDERER_PATCHED}"
    "${FN64_RT64_VI_CONSTANTS_ORIGINAL}"
    FN64_RT64_VI_CONSTANTS_OFFSET)
if(FN64_RT64_VI_CONSTANTS_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 VI push-constant context changed; review the fn64 VI filter overlay")
endif()
string(REPLACE
    "${FN64_RT64_VI_CONSTANTS_ORIGINAL}"
    "${FN64_RT64_VI_CONSTANTS_PATCHED}"
    FN64_RT64_VI_RENDERER_PATCHED
    "${FN64_RT64_VI_RENDERER_PATCHED}")
set(FN64_RT64_VI_RENDERER_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_vi_renderer.cpp")
file(WRITE
    "${FN64_RT64_VI_RENDERER_PATCHED_SOURCE}"
    "${FN64_RT64_VI_RENDERER_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_VI_RENDERER_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_VI_RENDERER_PATCHED_SOURCE}")

# Every fn64 VI event must enqueue one normal RT64 present even when the raw
# register image and source bytes repeat. This preserves retrace-seeded filter
# cadence without mislabeling the event as PresentEarly or changing VIHistory.
set(FN64_RT64_STATE_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/hle/rt64_state.cpp")
file(SHA256 "${FN64_RT64_STATE_SOURCE}" FN64_RT64_STATE_SHA256)
if(NOT FN64_RT64_STATE_SHA256 STREQUAL
   "07d3a7c1e2fa455aedff045d0ec19d52cea9616e1e4d41e99dad833b61a4cda2")
    message(FATAL_ERROR
        "RT64 State changed; review the fn64 VI retrace overlay")
endif()
file(READ "${FN64_RT64_STATE_SOURCE}" FN64_RT64_STATE_PATCHED)
string(REPLACE
    "#include \"rt64_interpreter.h\""
    "#include \"rt64_interpreter.h\"\n\nextern \"C\" uint32_t fn64_rt64_vi_retrace_event(void *, uint32_t);"
    FN64_RT64_STATE_PATCHED
    "${FN64_RT64_STATE_PATCHED}")
set(FN64_RT64_STATE_CONDITION_ORIGINAL
    "        if (fromEarlyPresent || viDifferent || fbChangesMade || screenChangesMade) {")
set(FN64_RT64_STATE_CONDITION_PATCHED
    "        const bool fn64RetraceEvent = fn64_rt64_vi_retrace_event(ext.device, fromEarlyPresent ? 1U : 0U) != 0U;\n        if (fromEarlyPresent || viDifferent || fbChangesMade || screenChangesMade || fn64RetraceEvent) {")
string(FIND
    "${FN64_RT64_STATE_PATCHED}"
    "${FN64_RT64_STATE_CONDITION_ORIGINAL}"
    FN64_RT64_STATE_CONDITION_OFFSET)
if(FN64_RT64_STATE_CONDITION_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 screen-update condition changed; review the fn64 VI retrace overlay")
endif()
string(REPLACE
    "${FN64_RT64_STATE_CONDITION_ORIGINAL}"
    "${FN64_RT64_STATE_CONDITION_PATCHED}"
    FN64_RT64_STATE_PATCHED
    "${FN64_RT64_STATE_PATCHED}")

# TEMPORARY diagnostic instrumentation (opt-in: -DFN64_RT64_WAIT_TRACE=ON),
# not a permanent overlay. `renderAndSynchronize`'s command-list wait inside
# `State::fullSync()` was found live (`sample` on the windowed shell) to be
# the largest single cost in a busy-scene capture while GPU device
# utilization measured 0% (see docs/plans/rt64-on-the-block-lane.md, 2026-08-13
# addendum, Finding 3). This times exactly that wait so the claim can be
# checked with real numbers instead of inferred from a sampling profiler's
# 1ms granularity. Remove this stanza once that question is answered; it is
# not meant to ship long-term the way the two patches above are.
if(FN64_RT64_WAIT_TRACE)
    set(FN64_RT64_STATE_WAIT_ORIGINAL [=[                ext.framebufferGraphicsWorker->commandList->end();
                framebufferRenderer->waitForUploaders();
                ext.framebufferGraphicsWorker->execute();
                ext.framebufferGraphicsWorker->wait();]=])
    string(FIND
        "${FN64_RT64_STATE_PATCHED}"
        "${FN64_RT64_STATE_WAIT_ORIGINAL}"
        FN64_RT64_STATE_WAIT_OFFSET)
    if(FN64_RT64_STATE_WAIT_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "RT64 renderAndSynchronize command-list wait changed; update or drop FN64_RT64_WAIT_TRACE")
    endif()
    set(FN64_RT64_STATE_WAIT_PATCHED [=[                ext.framebufferGraphicsWorker->commandList->end();
                framebufferRenderer->waitForUploaders();
                {
                    static uint64_t fn64_rs_calls = 0;
                    static double fn64_rs_wait_ms = 0.0;
                    ElapsedTimer fn64_rs_timer;
                    ext.framebufferGraphicsWorker->execute();
                    ext.framebufferGraphicsWorker->wait();
                    fn64_rs_wait_ms += fn64_rs_timer.elapsedMilliseconds();
                    fn64_rs_calls++;
                    if (fn64_rs_calls % 200 == 0) {
                        fprintf(stderr, "[fn64-rt64-trace] renderAndSynchronize execute+wait calls=%llu wait_total_ms=%.2f avg_wait_ms=%.4f\n",
                            (unsigned long long)fn64_rs_calls, fn64_rs_wait_ms, fn64_rs_wait_ms / fn64_rs_calls);
                    }
                }]=])
    string(REPLACE
        "${FN64_RT64_STATE_WAIT_ORIGINAL}"
        "${FN64_RT64_STATE_WAIT_PATCHED}"
        FN64_RT64_STATE_PATCHED
        "${FN64_RT64_STATE_PATCHED}")
    # The plain `#include "rt64_interpreter.h"` anchor consumed by the
    # VI-retrace patch above is no longer present verbatim in
    # FN64_RT64_STATE_PATCHED at this point -- target that patch's own output
    # instead so this replace actually matches.
    set(FN64_RT64_STATE_INCLUDE_PATCHED_ORIGINAL [=[#include "rt64_interpreter.h"

extern "C" uint32_t fn64_rt64_vi_retrace_event(void *, uint32_t);]=])
    set(FN64_RT64_STATE_INCLUDE_PATCHED_NEW [=[#include "rt64_interpreter.h"
#include "common/rt64_elapsed_timer.h"
#include <cstdio>

extern "C" uint32_t fn64_rt64_vi_retrace_event(void *, uint32_t);]=])
    string(FIND
        "${FN64_RT64_STATE_PATCHED}"
        "${FN64_RT64_STATE_INCLUDE_PATCHED_ORIGINAL}"
        FN64_RT64_STATE_INCLUDE_OFFSET)
    if(FN64_RT64_STATE_INCLUDE_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "RT64 State's retrace-patched include block changed; update FN64_RT64_WAIT_TRACE")
    endif()
    string(REPLACE
        "${FN64_RT64_STATE_INCLUDE_PATCHED_ORIGINAL}"
        "${FN64_RT64_STATE_INCLUDE_PATCHED_NEW}"
        FN64_RT64_STATE_PATCHED
        "${FN64_RT64_STATE_PATCHED}")
endif()

set(FN64_RT64_STATE_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_state.cpp")
file(WRITE "${FN64_RT64_STATE_PATCHED_SOURCE}" "${FN64_RT64_STATE_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_STATE_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_STATE_PATCHED_SOURCE}")

# Pinned RT64 normally reuses the previously selected GBI whenever a
# G_LOAD_UCODE keeps the same text/data addresses. That address cache cannot
# distinguish same-address content generations. Route every load through the
# fn64 ordered-plan observer before RT64 flushes or consults that cache, and
# leave only the address bookkeeping/reset behavior in the pinned function.
# The full-file digest and exact replacement contexts make any upstream change
# a mandatory review rather than silently moving this admission boundary.
set(FN64_RT64_INTERPRETER_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/hle/rt64_interpreter.cpp")
file(SHA256
    "${FN64_RT64_INTERPRETER_SOURCE}"
    FN64_RT64_INTERPRETER_SHA256)
if(NOT FN64_RT64_INTERPRETER_SHA256 STREQUAL
   "cd91e76019205296b0ff9a52d4b477d45959243302c218c3c047d9328e66a7d2")
    message(FATAL_ERROR
        "RT64 interpreter source changed; review the fn64 ordered-microcode admission overlay")
endif()
file(READ
    "${FN64_RT64_INTERPRETER_SOURCE}"
    FN64_RT64_INTERPRETER_CONTENTS)
set(FN64_RT64_INTERPRETER_DECLARATION_ORIGINAL [=[#include <cassert>

//#define DUMP_DISPLAY_LISTS]=])
set(FN64_RT64_INTERPRETER_DECLARATION_PATCHED [=[#include <cassert>

extern "C" void *fn64_rt64_ucode_generation_observe(
    void *, uint32_t, uint32_t, uint32_t);
extern "C" void fn64_rt64_ucode_generation_apply(void *, void *);

//#define DUMP_DISPLAY_LISTS]=])
string(FIND
    "${FN64_RT64_INTERPRETER_CONTENTS}"
    "${FN64_RT64_INTERPRETER_DECLARATION_ORIGINAL}"
    FN64_RT64_INTERPRETER_DECLARATION_OFFSET)
if(FN64_RT64_INTERPRETER_DECLARATION_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 interpreter declaration context changed; review the fn64 ordered-microcode admission overlay")
endif()
string(REPLACE
    "${FN64_RT64_INTERPRETER_DECLARATION_ORIGINAL}"
    "${FN64_RT64_INTERPRETER_DECLARATION_PATCHED}"
    FN64_RT64_INTERPRETER_PATCHED
    "${FN64_RT64_INTERPRETER_CONTENTS}")
set(FN64_RT64_INTERPRETER_LOAD_START_ORIGINAL [=[    void Interpreter::loadUCodeGBI(uint32_t textAddress, uint32_t dataAddress, bool resetFromTask) {
        if (!resetFromTask) {]=])
set(FN64_RT64_INTERPRETER_LOAD_START_PATCHED [=[    void Interpreter::loadUCodeGBI(uint32_t textAddress, uint32_t dataAddress, bool resetFromTask) {
        void *fn64AdmittedGBI = fn64_rt64_ucode_generation_observe(
            this, textAddress, dataAddress, resetFromTask ? 1U : 0U);
        if (!resetFromTask) {]=])
string(FIND
    "${FN64_RT64_INTERPRETER_PATCHED}"
    "${FN64_RT64_INTERPRETER_LOAD_START_ORIGINAL}"
    FN64_RT64_INTERPRETER_LOAD_START_OFFSET)
if(FN64_RT64_INTERPRETER_LOAD_START_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 loadUCodeGBI entry changed; review the fn64 ordered-microcode admission overlay")
endif()
string(REPLACE
    "${FN64_RT64_INTERPRETER_LOAD_START_ORIGINAL}"
    "${FN64_RT64_INTERPRETER_LOAD_START_PATCHED}"
    FN64_RT64_INTERPRETER_PATCHED
    "${FN64_RT64_INTERPRETER_PATCHED}")
set(FN64_RT64_INTERPRETER_CACHE_ORIGINAL [=[        if ((UCode.textAddress != maskedTextAddress) || (UCode.dataAddress != maskedDataAddress)) {
            hleGBI = gbiManager.getGBIForUCode(state->RDRAM, maskedTextAddress, maskedDataAddress);
            if (hleGBI != nullptr) {
                state->rsp->setGBI(hleGBI);
            }

            UCode.textAddress = maskedTextAddress;
            UCode.dataAddress = maskedDataAddress;
        }]=])
set(FN64_RT64_INTERPRETER_CACHE_PATCHED [=[        fn64_rt64_ucode_generation_apply(this, fn64AdmittedGBI);
        if ((UCode.textAddress != maskedTextAddress) || (UCode.dataAddress != maskedDataAddress)) {
            UCode.textAddress = maskedTextAddress;
            UCode.dataAddress = maskedDataAddress;
        }]=])
string(FIND
    "${FN64_RT64_INTERPRETER_PATCHED}"
    "${FN64_RT64_INTERPRETER_CACHE_ORIGINAL}"
    FN64_RT64_INTERPRETER_CACHE_OFFSET)
if(FN64_RT64_INTERPRETER_CACHE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 loadUCodeGBI address cache changed; review the fn64 ordered-microcode admission overlay")
endif()
string(REPLACE
    "${FN64_RT64_INTERPRETER_CACHE_ORIGINAL}"
    "${FN64_RT64_INTERPRETER_CACHE_PATCHED}"
    FN64_RT64_INTERPRETER_PATCHED
    "${FN64_RT64_INTERPRETER_PATCHED}")
set(FN64_RT64_INTERPRETER_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_interpreter.cpp")
file(WRITE
    "${FN64_RT64_INTERPRETER_PATCHED_SOURCE}"
    "${FN64_RT64_INTERPRETER_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_INTERPRETER_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_INTERPRETER_PATCHED_SOURCE}")
target_include_directories(rt64 PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src/hle")

# RT64's CompilationThread constructor launches an idle-priority worker before
# initializing its running predicate, and the worker writes that predicate true
# again on entry. A teardown that stores false and notifies before a delayed
# worker enters its loop can therefore have its stop request overwritten; the
# worker then sleeps after the only notification while the destructor joins it.
# Stage an exact-source overlay that publishes the running predicate before
# launch and leaves the destructor as its only post-launch writer. The source
# checks keep the overlay tied visibly to the reviewed pinned MIT source.
set(FN64_RT64_RASTER_SHADER_CACHE_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/render/rt64_raster_shader_cache.cpp")
file(READ
    "${FN64_RT64_RASTER_SHADER_CACHE_SOURCE}"
    FN64_RT64_RASTER_SHADER_CACHE_CONTENTS)
set(FN64_RT64_COMPILATION_THREAD_START_ORIGINAL
    "        this->shaderCache = shaderCache;\n\n        thread = std::make_unique<std::thread>(&CompilationThread::loop, this);\n        threadRunning = false;")
set(FN64_RT64_COMPILATION_THREAD_START_PATCHED
    "        this->shaderCache = shaderCache;\n\n        // The destructor's false store and notification must happen after this\n        // publication: a delayed idle-priority worker must never overwrite the\n        // stop predicate and sleep after the notification while join waits.\n        threadRunning = true;\n        thread = std::make_unique<std::thread>(&CompilationThread::loop, this);")
string(FIND
    "${FN64_RT64_RASTER_SHADER_CACHE_CONTENTS}"
    "${FN64_RT64_COMPILATION_THREAD_START_ORIGINAL}"
    FN64_RT64_COMPILATION_THREAD_START_OFFSET)
if(FN64_RT64_COMPILATION_THREAD_START_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster-shader worker startup source changed; review the fn64 predicate overlay")
endif()
string(REPLACE
    "${FN64_RT64_COMPILATION_THREAD_START_ORIGINAL}"
    "${FN64_RT64_COMPILATION_THREAD_START_PATCHED}"
    FN64_RT64_RASTER_SHADER_CACHE_PATCHED
    "${FN64_RT64_RASTER_SHADER_CACHE_CONTENTS}")
set(FN64_RT64_COMPILATION_THREAD_LOOP_ORIGINAL
    "        threadRunning = true;\n\n        while (threadRunning) {")
set(FN64_RT64_COMPILATION_THREAD_LOOP_PATCHED
    "        while (threadRunning) {")
string(FIND
    "${FN64_RT64_RASTER_SHADER_CACHE_PATCHED}"
    "${FN64_RT64_COMPILATION_THREAD_LOOP_ORIGINAL}"
    FN64_RT64_COMPILATION_THREAD_LOOP_OFFSET)
if(FN64_RT64_COMPILATION_THREAD_LOOP_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 raster-shader worker loop source changed; review the fn64 predicate overlay")
endif()
string(REPLACE
    "${FN64_RT64_COMPILATION_THREAD_LOOP_ORIGINAL}"
    "${FN64_RT64_COMPILATION_THREAD_LOOP_PATCHED}"
    FN64_RT64_RASTER_SHADER_CACHE_PATCHED
    "${FN64_RT64_RASTER_SHADER_CACHE_PATCHED}")
set(FN64_RT64_RASTER_SHADER_CACHE_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_raster_shader_cache.cpp")
file(WRITE
    "${FN64_RT64_RASTER_SHADER_CACHE_PATCHED_SOURCE}"
    "${FN64_RT64_RASTER_SHADER_CACHE_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_RASTER_SHADER_CACHE_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_RASTER_SHADER_CACHE_PATCHED_SOURCE}")
target_include_directories(rt64 PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src/render")

# Pinned RT64 infers a logical workload rate by dividing a hardcoded 60 Hz by
# the stable VI presentation factor. Bind that calculation to the fn64 context's
# IPL-selected 50/60 Hz standard without mutating or vendoring the MIT checkout.
# The full-file digest and exact replacement contexts make upstream drift loud.
set(FN64_RT64_VI_SOURCE
    "${FN64_RT64_SOURCE_DIR}/src/hle/rt64_vi.cpp")
file(SHA256
    "${FN64_RT64_VI_SOURCE}"
    FN64_RT64_VI_SHA256)
if(NOT FN64_RT64_VI_SHA256 STREQUAL
   "9b3cf39bb15fc0c7d52085566197042f4960cc410b241e38457bb817f2501e5b")
    message(FATAL_ERROR
        "RT64 VI source changed; review the fn64 region-rate overlay")
endif()
file(READ
    "${FN64_RT64_VI_SOURCE}"
    FN64_RT64_VI_CONTENTS)
set(FN64_RT64_VI_DECLARATION_ORIGINAL [=[#include "rt64_vi.h"

namespace RT64 {]=])
set(FN64_RT64_VI_DECLARATION_PATCHED [=[#include "rt64_vi.h"

extern "C" uint32_t fn64_rt64_nominal_full_rate(const void *);

namespace RT64 {]=])
string(FIND
    "${FN64_RT64_VI_CONTENTS}"
    "${FN64_RT64_VI_DECLARATION_ORIGINAL}"
    FN64_RT64_VI_DECLARATION_OFFSET)
if(FN64_RT64_VI_DECLARATION_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 VI declaration context changed; review the fn64 region-rate overlay")
endif()
string(REPLACE
    "${FN64_RT64_VI_DECLARATION_ORIGINAL}"
    "${FN64_RT64_VI_DECLARATION_PATCHED}"
    FN64_RT64_VI_PATCHED
    "${FN64_RT64_VI_CONTENTS}")
set(FN64_RT64_VI_RATE_ORIGINAL
    "            const uint32_t FullRate = 60; // TODO: PAL support.")
set(FN64_RT64_VI_RATE_PATCHED
    "            const uint32_t FullRate = fn64_rt64_nominal_full_rate(this);")
string(FIND
    "${FN64_RT64_VI_PATCHED}"
    "${FN64_RT64_VI_RATE_ORIGINAL}"
    FN64_RT64_VI_RATE_OFFSET)
if(FN64_RT64_VI_RATE_OFFSET EQUAL -1)
    message(FATAL_ERROR
        "RT64 VI logical-rate context changed; review the fn64 region-rate overlay")
endif()
string(REPLACE
    "${FN64_RT64_VI_RATE_ORIGINAL}"
    "${FN64_RT64_VI_RATE_PATCHED}"
    FN64_RT64_VI_PATCHED
    "${FN64_RT64_VI_PATCHED}")
set(FN64_RT64_VI_PATCHED_SOURCE
    "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_vi.cpp")
file(WRITE
    "${FN64_RT64_VI_PATCHED_SOURCE}"
    "${FN64_RT64_VI_PATCHED}")
set_source_files_properties(
    "${FN64_RT64_VI_SOURCE}"
    TARGET_DIRECTORY rt64
    PROPERTIES HEADER_FILE_ONLY TRUE)
target_sources(rt64 PRIVATE "${FN64_RT64_VI_PATCHED_SOURCE}")
target_include_directories(rt64 PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src/hle")

if(FN64_RT64_HFR_EVIDENCE)
    # Observe the exact actual-present boundary without changing its control
    # flow. The full-file digest and exact replacement contexts bind this
    # evidence overlay to the reviewed pinned MIT source and fail on drift.
    set(FN64_RT64_PRESENT_QUEUE_SOURCE
        "${FN64_RT64_SOURCE_DIR}/src/hle/rt64_present_queue.cpp")
    file(SHA256
        "${FN64_RT64_PRESENT_QUEUE_SOURCE}"
        FN64_RT64_PRESENT_QUEUE_SHA256)
    if(NOT FN64_RT64_PRESENT_QUEUE_SHA256 STREQUAL
       "023dc3f4bc74bf6863e37ad1781fb32eaa543a7713bd8b9a3757b2262a1a1e52")
        message(FATAL_ERROR
            "RT64 present-queue source changed; review the HFR pacing overlay")
    endif()
    file(READ
        "${FN64_RT64_PRESENT_QUEUE_SOURCE}"
        FN64_RT64_PRESENT_QUEUE_CONTENTS)
    set(FN64_RT64_PRESENT_QUEUE_DECLARATION_ORIGINAL [=[#include "rt64_workload_queue.h"

namespace RT64 {]=])
    set(FN64_RT64_PRESENT_QUEUE_DECLARATION_PATCHED [=[#include "rt64_workload_queue.h"

extern "C" void fn64_rt64_hfr_present_call_observe(void *, uint64_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t);

namespace RT64 {]=])
    string(FIND
        "${FN64_RT64_PRESENT_QUEUE_CONTENTS}"
        "${FN64_RT64_PRESENT_QUEUE_DECLARATION_ORIGINAL}"
        FN64_RT64_PRESENT_QUEUE_DECLARATION_OFFSET)
    if(FN64_RT64_PRESENT_QUEUE_DECLARATION_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "RT64 present-queue declaration context changed; review the HFR pacing overlay")
    endif()
    string(REPLACE
        "${FN64_RT64_PRESENT_QUEUE_DECLARATION_ORIGINAL}"
        "${FN64_RT64_PRESENT_QUEUE_DECLARATION_PATCHED}"
        FN64_RT64_PRESENT_QUEUE_PATCHED
        "${FN64_RT64_PRESENT_QUEUE_CONTENTS}")
    set(FN64_RT64_PRESENT_QUEUE_CALL_ORIGINAL [=[                presentTimestamp = Timer::current();
                swapChainValid = ext.swapChain->present(swapChainIndex, &waitSemaphore, 1);
                presentProfiler.logAndRestart();]=])
    set(FN64_RT64_PRESENT_QUEUE_CALL_PATCHED [=[                presentTimestamp = Timer::current();
                fn64_rt64_hfr_present_call_observe(ext.device, present.presentId, uint32_t(i), uint32_t(framesToPresent), viOriginalRate, targetRate, 0, 1);
                swapChainValid = ext.swapChain->present(swapChainIndex, &waitSemaphore, 1);
                fn64_rt64_hfr_present_call_observe(ext.device, present.presentId, uint32_t(i), uint32_t(framesToPresent), viOriginalRate, targetRate, 1, swapChainValid ? 1U : 0U);
                presentProfiler.logAndRestart();]=])
    string(FIND
        "${FN64_RT64_PRESENT_QUEUE_PATCHED}"
        "${FN64_RT64_PRESENT_QUEUE_CALL_ORIGINAL}"
        FN64_RT64_PRESENT_QUEUE_CALL_OFFSET)
    if(FN64_RT64_PRESENT_QUEUE_CALL_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "RT64 present-call context changed; review the HFR pacing overlay")
    endif()
    string(REPLACE
        "${FN64_RT64_PRESENT_QUEUE_CALL_ORIGINAL}"
        "${FN64_RT64_PRESENT_QUEUE_CALL_PATCHED}"
        FN64_RT64_PRESENT_QUEUE_PATCHED
        "${FN64_RT64_PRESENT_QUEUE_PATCHED}")
    set(FN64_RT64_PRESENT_QUEUE_PATCHED_SOURCE
        "${CMAKE_CURRENT_BINARY_DIR}/fn64_rt64_present_queue.cpp")
    file(WRITE
        "${FN64_RT64_PRESENT_QUEUE_PATCHED_SOURCE}"
        "${FN64_RT64_PRESENT_QUEUE_PATCHED}")
    set_source_files_properties(
        "${FN64_RT64_PRESENT_QUEUE_SOURCE}"
        TARGET_DIRECTORY rt64
        PROPERTIES HEADER_FILE_ONLY TRUE)
    target_sources(rt64 PRIVATE "${FN64_RT64_PRESENT_QUEUE_PATCHED_SOURCE}")
    target_include_directories(rt64 PRIVATE
        "${FN64_RT64_SOURCE_DIR}/src/hle")
endif()

# plume acquires several autoreleased convenience-factory results, then manually
# releases them from commit/end/destructor paths. When RT64's workload thread
# exits, its implicit autorelease pool releases the already-deallocated AGX blit
# context and crashes in objc_release. Retain at acquisition so plume's existing
# releases balance. Cover both persistent encoder variants because they share
# the same ownership shape even though framebuffer readback currently exercises
# the blit encoder.
# Keep this exact-source check loud: a changed upstream ownership path must be
# reviewed rather than receiving a stale textual patch silently.
if(APPLE)
    set(FN64_PLUME_METAL_SOURCE
        "${FN64_RT64_SOURCE_DIR}/src/contrib/plume/plume_metal.cpp")
    file(READ "${FN64_PLUME_METAL_SOURCE}" FN64_PLUME_METAL_CONTENTS)
    set(FN64_PLUME_COMMAND_BUFFER_ACQUIRE
        "        mtl = queue->mtl->commandBufferWithUnretainedReferences();\n        mtl->setLabel(MTLSTR(\"RT64 Command List\"));")
    set(FN64_PLUME_COMMAND_BUFFER_RETAIN
        "        mtl = queue->mtl->commandBufferWithUnretainedReferences();\n        mtl->retain();\n        mtl->setLabel(MTLSTR(\"RT64 Command List\"));")
    string(FIND
        "${FN64_PLUME_METAL_CONTENTS}"
        "${FN64_PLUME_COMMAND_BUFFER_ACQUIRE}"
        FN64_PLUME_PATCH_OFFSET)
    if(FN64_PLUME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal command-buffer ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_COMMAND_BUFFER_ACQUIRE}"
        "${FN64_PLUME_COMMAND_BUFFER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_CONTENTS}")
    set(FN64_PLUME_BLIT_ENCODER_ACQUIRE
        "            activeBlitEncoder = mtl->blitCommandEncoder(device->sharedBlitDescriptor);\n            activeBlitEncoder->setLabel(MTLSTR(\"Copy Blit Encoder\"));")
    set(FN64_PLUME_BLIT_ENCODER_RETAIN
        "            activeBlitEncoder = mtl->blitCommandEncoder(device->sharedBlitDescriptor);\n            activeBlitEncoder->retain();\n            activeBlitEncoder->setLabel(MTLSTR(\"Copy Blit Encoder\"));")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_BLIT_ENCODER_ACQUIRE}"
        FN64_PLUME_BLIT_PATCH_OFFSET)
    if(FN64_PLUME_BLIT_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal blit-encoder ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_BLIT_ENCODER_ACQUIRE}"
        "${FN64_PLUME_BLIT_ENCODER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_RESOLVE_ENCODER_ACQUIRE
        "            activeResolveComputeEncoder = mtl->computeCommandEncoder(MTL::DispatchTypeConcurrent);\n            activeResolveComputeEncoder->setLabel(MTLSTR(\"Resolve Texture Encoder\"));")
    set(FN64_PLUME_RESOLVE_ENCODER_RETAIN
        "            activeResolveComputeEncoder = mtl->computeCommandEncoder(MTL::DispatchTypeConcurrent);\n            activeResolveComputeEncoder->retain();\n            activeResolveComputeEncoder->setLabel(MTLSTR(\"Resolve Texture Encoder\"));")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_RESOLVE_ENCODER_ACQUIRE}"
        FN64_PLUME_RESOLVE_PATCH_OFFSET)
    if(FN64_PLUME_RESOLVE_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal resolve-encoder ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_RESOLVE_ENCODER_ACQUIRE}"
        "${FN64_PLUME_RESOLVE_ENCODER_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE
        "        MTL::TextureDescriptor *descriptor = MTL::TextureDescriptor::textureBufferDescriptor(pixelFormat, width, options, usage);\n        this->texture = buffer->mtl->newTexture(descriptor, 0, bytesPerRow);")
    set(FN64_PLUME_TEXTURE_DESCRIPTOR_RETAIN
        "        MTL::TextureDescriptor *descriptor = MTL::TextureDescriptor::textureBufferDescriptor(pixelFormat, width, options, usage);\n        descriptor->retain();\n        this->texture = buffer->mtl->newTexture(descriptor, 0, bytesPerRow);")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE}"
        FN64_PLUME_TEXTURE_DESCRIPTOR_PATCH_OFFSET)
    if(FN64_PLUME_TEXTURE_DESCRIPTOR_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal texture-descriptor ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_ACQUIRE}"
        "${FN64_PLUME_TEXTURE_DESCRIPTOR_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_FUNCTION_NAME_ACQUIRE
        "        this->functionName = (entryPointName != nullptr) ? NS::String::string(entryPointName, NS::UTF8StringEncoding) : MTLSTR(\"\");")
    set(FN64_PLUME_FUNCTION_NAME_RETAIN
        "        this->functionName = (entryPointName != nullptr) ? NS::String::string(entryPointName, NS::UTF8StringEncoding) : MTLSTR(\"\");\n        this->functionName->retain();")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_FUNCTION_NAME_ACQUIRE}"
        FN64_PLUME_FUNCTION_NAME_PATCH_OFFSET)
    if(FN64_PLUME_FUNCTION_NAME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal shader-name ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_FUNCTION_NAME_ACQUIRE}"
        "${FN64_PLUME_FUNCTION_NAME_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_DEBUG_NAME_ACQUIRE
        "        debugName = NS::String::string(name.c_str(), NS::UTF8StringEncoding);\n        library->setLabel(debugName);")
    set(FN64_PLUME_DEBUG_NAME_RETAIN
        "        debugName = NS::String::string(name.c_str(), NS::UTF8StringEncoding);\n        debugName->retain();\n        library->setLabel(debugName);")
    string(FIND
        "${FN64_PLUME_METAL_PATCHED}"
        "${FN64_PLUME_DEBUG_NAME_ACQUIRE}"
        FN64_PLUME_DEBUG_NAME_PATCH_OFFSET)
    if(FN64_PLUME_DEBUG_NAME_PATCH_OFFSET EQUAL -1)
        message(FATAL_ERROR
            "plume Metal shader-debug-name ownership source changed; review the fn64 retain patch")
    endif()
    string(REPLACE
        "${FN64_PLUME_DEBUG_NAME_ACQUIRE}"
        "${FN64_PLUME_DEBUG_NAME_RETAIN}"
        FN64_PLUME_METAL_PATCHED
        "${FN64_PLUME_METAL_PATCHED}")
    set(FN64_PLUME_METAL_PATCHED_SOURCE
        "${CMAKE_CURRENT_BINARY_DIR}/fn64_plume_metal.cpp")
    file(WRITE
        "${FN64_PLUME_METAL_PATCHED_SOURCE}"
        "${FN64_PLUME_METAL_PATCHED}")

    # plume declared the source relative to its own source directory. Mark that
    # entry non-compiling in plume's directory scope before adding the patched
    # translation unit; mutating the target's SOURCES list leaves both object
    # files in Xcode/Makefile generators.
    set_source_files_properties(
        "${FN64_PLUME_METAL_SOURCE}"
        TARGET_DIRECTORY plume
        PROPERTIES HEADER_FILE_ONLY TRUE)
    target_sources(plume PRIVATE "${FN64_PLUME_METAL_PATCHED_SOURCE}")
endif()

# AppleClang 21 no longer makes the C `labs` declaration transitively visible
# to RT64's bundled hlslpp scalar backend. Inject the standard declaration at
# compile time without modifying the sibling checkout. The warning suppressions
# mirror RT64's own Clang >=20 branch, which CMake does not select when the
# compiler id is spelled AppleClang.
if(APPLE AND CMAKE_CXX_COMPILER_ID STREQUAL "AppleClang")
    target_compile_options(rt64 PRIVATE
        "$<$<COMPILE_LANGUAGE:CXX>:-include>"
        "$<$<COMPILE_LANGUAGE:CXX>:stdlib.h>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-nontrivial-memcall>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-deprecated-literal-operator>"
    )
endif()

add_library(fn64_rt64_shim STATIC fn64_rt64_shim.cpp)
target_compile_definitions(fn64_rt64_shim PRIVATE
    HLSL_CPU
    FFX_GCC
    IMGUI_IMPL_VULKAN_NO_PROTOTYPES
)
if(FN64_RT64_HFR_EVIDENCE)
    target_compile_definitions(fn64_rt64_shim PRIVATE FN64_RT64_HFR_EVIDENCE)
endif()
if(FN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE)
    target_compile_definitions(fn64_rt64_shim PRIVATE
        FN64_RT64_SYNTHETIC_F3DEX2_EVIDENCE)
endif()
if(FN64_RT64_SYNTHETIC_S2DEX_EVIDENCE)
    target_compile_definitions(fn64_rt64_shim PRIVATE
        FN64_RT64_SYNTHETIC_S2DEX_EVIDENCE)
endif()
target_include_directories(fn64_rt64_shim PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src"
    "${FN64_RT64_SOURCE_DIR}/src/contrib"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume/contrib/metal-cpp"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/hlslpp/include"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/imgui"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/nativefiledialog-extended/src/include"
    "${CMAKE_CURRENT_BINARY_DIR}/rt64-build/src"
)
target_link_libraries(fn64_rt64_shim PRIVATE rt64)
