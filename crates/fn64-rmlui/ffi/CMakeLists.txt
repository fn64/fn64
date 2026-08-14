cmake_minimum_required(VERSION 3.20)
project(fn64_rmlui_shim LANGUAGES C CXX)

if(NOT DEFINED FN64_RT64_SOURCE_DIR)
    message(FATAL_ERROR "FN64_RT64_SOURCE_DIR must name the sibling MIT RT64 checkout")
endif()
if(NOT DEFINED FN64_RMLUI_SOURCE_DIR)
    message(FATAL_ERROR "FN64_RMLUI_SOURCE_DIR must name the sibling MIT RmlUi checkout")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Matches fn64-render-rt64/ffi/CMakeLists.txt's own RT64 configuration
# exactly, so the two crates' independently-built `rt64` static targets stay
# structurally identical (same static/no-shared-libs policy, same disabled
# tool/test/example subtargets). This crate does NOT reuse
# fn64-render-rt64's already-built RT64 archive -- Cargo build scripts each
# get their own OUT_DIR and there is no supported way to share one CMake
# build tree across two separate crates' build.rs invocations, so this is a
# second, independent build of RT64 from the same source, not a second copy
# of RT64 itself. Wasteful (RT64 compiles twice across a full workspace
# build with both crates enabled) but correctness-safe: both builds compile
# the identical vendored source with the identical flags.
set(RT64_STATIC ON CACHE BOOL "Build only RT64's static render/HLE library" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "Keep RT64 and its dependencies static" FORCE)
set(NFD_BUILD_TESTS OFF CACHE BOOL "Do not build nativefiledialog tests" FORCE)
set(NFD_INSTALL OFF CACHE BOOL "Do not generate nativefiledialog install target" FORCE)
set(ZSTD_BUILD_PROGRAMS OFF CACHE BOOL "Do not build zstd tools" FORCE)
set(ZSTD_BUILD_TESTS OFF CACHE BOOL "Do not build zstd tests" FORCE)
set(PLUME_BUILD_EXAMPLES OFF CACHE BOOL "Do not build plume examples" FORCE)
add_subdirectory("${FN64_RT64_SOURCE_DIR}" rt64-build EXCLUDE_FROM_ALL)

# AppleClang 21 no longer makes the C `labs` declaration transitively
# visible to RT64's bundled hlslpp scalar backend. Matches
# fn64-render-rt64/ffi/CMakeLists.txt's own fix for this exact issue
# exactly -- required here too, since this crate builds its own
# independent `rt64` CMake target rather than reusing
# fn64-render-rt64's already-patched one (see the comment above on why
# RT64 is built twice rather than shared).
if(APPLE AND CMAKE_CXX_COMPILER_ID STREQUAL "AppleClang")
    target_compile_options(rt64 PRIVATE
        "$<$<COMPILE_LANGUAGE:CXX>:-include>"
        "$<$<COMPILE_LANGUAGE:CXX>:stdlib.h>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-nontrivial-memcall>"
        "$<$<COMPILE_LANGUAGE:CXX>:-Wno-deprecated-literal-operator>"
    )
endif()

# RmlUi's own default build (Freetype only, no SVG/Lottie/Lua/samples) --
# verified elsewhere this session to configure cleanly and to carry zero
# dependency overlap with RT64's own dependency set (no shared library
# name between RmlUi's {Freetype} and RT64's {zstd, nfd, re-spirv, plume,
# SDL2}, so no version-pin collision is possible building both in one tree).
set(RMLUI_SAMPLES OFF CACHE BOOL "Do not build RmlUi's sample viewer" FORCE)
set(RMLUI_LUA_BINDINGS OFF CACHE BOOL "fn64-rmlui does not use RmlUi's Lua scripting plugin" FORCE)
add_subdirectory("${FN64_RMLUI_SOURCE_DIR}" rmlui-build EXCLUDE_FROM_ALL)

# Compile fn64-rmlui's UI vertex/pixel shaders. Same directory-scope
# re-import dance fn64-render-rt64/ffi/CMakeLists.txt already uses (see its
# comment above its own build_shader calls): build_vertex_shader/
# build_pixel_shader are CMake functions defined inside RT64's own
# CMakeLists.txt, so DXC/its option variables must be re-imported from that
# child directory's scope before calling them here. build_vertex_shader/
# build_pixel_shader are used directly (rather than build_shader with a
# manually-supplied OPTIONS list) since this crate has exactly one VS/PS
# entry point pair each named VSMain/PSMain -- the exact case those two
# convenience wrappers exist for.
foreach(FN64_RMLUI_SHADER_VARIABLE
        DXC DXC_VS_OPTS DXC_PS_OPTS DXC_SPV_OPTS SPIRVCROSS)
    get_directory_property(
        FN64_RMLUI_IMPORTED_${FN64_RMLUI_SHADER_VARIABLE}
        DIRECTORY "${FN64_RT64_SOURCE_DIR}"
        DEFINITION ${FN64_RMLUI_SHADER_VARIABLE})
    set(${FN64_RMLUI_SHADER_VARIABLE} "${FN64_RMLUI_IMPORTED_${FN64_RMLUI_SHADER_VARIABLE}}")
endforeach()
target_include_directories(rt64 PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
build_vertex_shader(
    rt64
    "fn64_rmlui_ui_vs.hlsl"
    "src/shaders/Fn64RmluiUiVS.hlsl"
    "-I${CMAKE_CURRENT_SOURCE_DIR}")
build_pixel_shader(
    rt64
    "fn64_rmlui_ui_ps.hlsl"
    "src/shaders/Fn64RmluiUiPS.hlsl"
    "-I${CMAKE_CURRENT_SOURCE_DIR}")
set(FN64_RMLUI_UI_SHADER_OUTPUTS
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiVS.hlsl.spirv.c"
    "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiPS.hlsl.spirv.c")
if(APPLE)
    list(APPEND FN64_RMLUI_UI_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiVS.hlsl.metal.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiPS.hlsl.metal.c")
elseif(WIN32)
    list(APPEND FN64_RMLUI_UI_SHADER_OUTPUTS
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiVS.hlsl.dxil.c"
        "${CMAKE_BINARY_DIR}/src/shaders/Fn64RmluiUiPS.hlsl.dxil.c")
endif()
# CMake custom-command visibility is directory-scoped while `rt64` belongs to
# the child directory; this explicit parent target publishes the generated
# shader objects before fn64_rmlui_shim compiles the C++ that includes their
# generated headers.
add_custom_target(fn64_rmlui_ui_shaders DEPENDS ${FN64_RMLUI_UI_SHADER_OUTPUTS})
add_dependencies(rt64 fn64_rmlui_ui_shaders)

add_library(fn64_rmlui_shim STATIC
    fn64_rmlui_shim.cpp
    fn64_rmlui_render_interface.cpp
)
add_dependencies(fn64_rmlui_shim fn64_rmlui_ui_shaders)
target_compile_definitions(fn64_rmlui_shim PRIVATE
    HLSL_CPU
    FFX_GCC
)
target_include_directories(fn64_rmlui_shim PRIVATE
    "${FN64_RT64_SOURCE_DIR}/src"
    "${FN64_RT64_SOURCE_DIR}/src/contrib"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/plume/contrib/metal-cpp"
    "${FN64_RT64_SOURCE_DIR}/src/contrib/hlslpp/include"
    "${CMAKE_CURRENT_BINARY_DIR}/rt64-build/src"
    # Generated shader blob headers (Fn64RmluiUiVS.hlsl.spirv.h etc.), built
    # by build_vertex_shader/build_pixel_shader above into
    # ${CMAKE_BINARY_DIR}/src/shaders. rt64 itself gets this path via its
    # own PRIVATE target_include_directories in RT64's top-level
    # CMakeLists.txt, which is not visible to this separate target.
    "${CMAKE_BINARY_DIR}/src"
    # fn64-render-rt64's own shim header, for the Fn64Rt64Context opaque
    # type and fn64_rt64_register_overlay_draw/unregister_overlay_draw --
    # this crate calls into that shim, it does not redefine or duplicate
    # its types.
    "${FN64_RENDER_RT64_FFI_DIR}"
)
target_link_libraries(fn64_rmlui_shim PRIVATE rt64 RmlUi::Core)
