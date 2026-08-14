cmake_minimum_required(VERSION 3.20)
project(fn64_rmlui_shim LANGUAGES C CXX)

if(NOT DEFINED FN64_RMLUI_SOURCE_DIR)
    message(FATAL_ERROR "FN64_RMLUI_SOURCE_DIR must name the sibling MIT RmlUi checkout")
endif()
if(NOT DEFINED FN64_RENDER_RT64_FFI_DIR)
    message(FATAL_ERROR "FN64_RENDER_RT64_FFI_DIR must name fn64-render-rt64's ffi/ directory")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# This crate deliberately does NOT build RT64 -- the RmlUi<->plume
# render-interface bridge that used to live here (and required its own
# full RT64 CMake tree to compile) has moved to fn64-render-rt64 itself
# (see that crate's ffi/fn64_rt64_rmlui_bridge.{h,cpp} and its own
# CMakeLists.txt's `rmlui` feature block). This crate now only needs
# fn64_rt64_shim.h's and fn64_rt64_rmlui_bridge.h's plain C declarations
# (no RT64/plume headers, no RT64 source) to call into that
# already-built bridge -- linking against fn64-render-rt64's own compiled
# archive happens at Cargo's final link stage (each crate's build.rs emits
# its own `cargo:rustc-link-lib` directives; both land in the one final
# binary), not here. A prior version of this file built its own
# independent copy of RT64 (including its own copy of RT64's internal
# RenderHookInit/Draw/Deinit global storage) specifically so it could
# construct that render interface itself -- besides being wasteful (RT64
# compiling twice per full workspace build), that duplication meant this
# crate's copy of RT64's hook-storage globals was a SEPARATE C++ static
# from fn64-render-rt64's own copy, so the single shared overlay-draw
# hook slot the whole settings-menu mechanism depends on was only ever
# guaranteed to work by link-order luck, not by design. Not linking RT64
# here at all removes that hazard structurally.
set(RMLUI_SAMPLES OFF CACHE BOOL "Do not build RmlUi's sample viewer" FORCE)
set(RMLUI_LUA_BINDINGS OFF CACHE BOOL "fn64-rmlui does not use RmlUi's Lua scripting plugin" FORCE)
add_subdirectory("${FN64_RMLUI_SOURCE_DIR}" rmlui-build EXCLUDE_FROM_ALL)

add_library(fn64_rmlui_shim STATIC fn64_rmlui_shim.cpp)
target_include_directories(fn64_rmlui_shim PRIVATE
    # fn64-render-rt64's own shim headers, for the Fn64Rt64Context opaque
    # type, the overlay-draw registration calls, and the RmlUi
    # render-interface bridge -- this crate calls into fn64-render-rt64's
    # already-built code, it does not redefine or duplicate any of it.
    "${FN64_RENDER_RT64_FFI_DIR}"
)
target_link_libraries(fn64_rmlui_shim PRIVATE RmlUi::Core)
