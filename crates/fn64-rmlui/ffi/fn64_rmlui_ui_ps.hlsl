// fn64-rmlui UI pixel shader. Samples one bound texture and multiplies by
// the interpolated (already premultiplied-alpha) vertex color. Untextured
// RmlUi geometry (RenderGeometry() called with texture == 0) is handled by
// always binding some texture -- the render interface binds a 1x1 opaque
// white texture in that case -- rather than branching to a second
// shader/pipeline variant, so this shader never needs to know whether the
// draw is "really" textured.

Texture2D<float4> gTexture : register(t1);
SamplerState gSampler : register(s2);

struct PS_INPUT {
    float4 position : SV_Position;
    float4 color     : COLOR;
    float2 texCoord  : TEXCOORD0;
};

float4 PSMain(PS_INPUT input) : SV_TARGET {
    float4 sampled = gTexture.Sample(gSampler, input.texCoord);
    return sampled * input.color;
}
