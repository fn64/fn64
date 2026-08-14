// fn64-rmlui UI vertex shader. Consumes RmlUi's own Vertex layout directly
// (screen-space pixel position, premultiplied-alpha vertex color, texture
// UV -- see RmlUi/Core/Vertex.h) and maps it to clip space using the
// per-draw translation RmlUi supplies through RenderGeometry() plus the
// current viewport size, both carried in one push constant.

#include "fn64_rt64_rmlui_ui.h"

[[vk::push_constant]] ConstantBuffer<Fn64RmluiTranslationCB> gConstants : register(b0);

struct VS_INPUT {
    float2 position : POSITION;
    float4 color     : COLOR;
    float2 texCoord  : TEXCOORD0;
};

struct VS_OUTPUT {
    float4 position : SV_Position;
    float4 color     : COLOR;
    float2 texCoord  : TEXCOORD0;
};

VS_OUTPUT VSMain(VS_INPUT input) {
    VS_OUTPUT output;
    float2 screenPosition = input.position + float2(gConstants.translationX, gConstants.translationY);
    float2 ndc = (screenPosition / float2(gConstants.viewportWidth, gConstants.viewportHeight)) * 2.0f - 1.0f;
    output.position = float4(ndc.x, -ndc.y, 0.0f, 1.0f);
    output.color = input.color;
    output.texCoord = input.texCoord;
    return output;
}
