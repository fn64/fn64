// Push-constant layout shared between fn64_rmlui_render_interface.cpp and
// fn64_rmlui_ui_vs.hlsl/fn64_rmlui_ui_ps.hlsl. Plain float members only (no
// hlslpp/interop wrapper types): a two-float translation vector has
// identical layout under both a C++ compiler and DXC, so the heavier
// shared/rt64_hlsl.h interop machinery (built for hlslpp-backed matrix/vector
// constant buffers elsewhere in RT64) is not needed for a struct this small.
#pragma once

#ifdef __cplusplus
namespace fn64_rmlui_interop {
#endif

// RmlUi's RenderGeometry() hands the render interface a translation vector,
// not a projection matrix (see RenderInterface.h) -- the vertex shader still
// needs the viewport size to map screen-space pixels to clip space, so that
// travels alongside the translation in the same push-constant range rather
// than as a second range (it changes once per frame, on resize, not once
// per draw call, but a second range buys nothing here: both are tiny and
// pushed together is simpler than tracking two ranges for one pipeline).
struct Fn64RmluiTranslationCB {
    float translationX;
    float translationY;
    float viewportWidth;
    float viewportHeight;
};

#ifdef __cplusplus
} // namespace fn64_rmlui_interop
#endif
