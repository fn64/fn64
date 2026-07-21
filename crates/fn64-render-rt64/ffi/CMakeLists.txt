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

# Compile fn64's bounded post-gamma VI shader as a source-identity overlay.
# RT64's shader helper functions are defined in the child directory; import
# their directory-local compiler variables into this caller before invoking
# them against the fn64-owned source staged beside this file.
foreach(FN64_SHADER_VARIABLE
        DXC DXC_DXIL_OPTS DXC_SPV_OPTS DXC_PS_OPTS SPIRVCROSS)
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
