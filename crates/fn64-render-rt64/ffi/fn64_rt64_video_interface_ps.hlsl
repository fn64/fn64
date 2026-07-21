// Pinned RT64 VI pass plus fn64's bounded post-gamma dither policy.

#include "fn64_rt64_video_interface.h"

[[vk::push_constant]] ConstantBuffer<VideoInterfaceCB> gConstants : register(b0);
Texture2D<float4> gInput : register(t1);
SamplerState gSampler : register(s2);

// uint2 is little-endian (low, high). RT64's Metal shader target predates
// MSL 2.2, so the exact modulo-2^64 policy is expressed without native u64.
uint2 Add64(uint2 left, uint2 right) {
    uint low = left.x + right.x;
    return uint2(low, left.y + right.y + uint(low < left.x));
}

uint2 ShiftRight64(uint2 value, uint amount) {
    return uint2(
        (value.x >> amount) | (value.y << (32u - amount)),
        value.y >> amount);
}

uint2 Mul32Wide(uint left, uint right) {
    uint leftLow = left & 0xffffu;
    uint leftHigh = left >> 16u;
    uint rightLow = right & 0xffffu;
    uint rightHigh = right >> 16u;
    uint lowProduct = leftLow * rightLow;
    uint leftCross = leftLow * rightHigh;
    uint rightCross = leftHigh * rightLow;
    uint middle = (lowProduct >> 16u) +
        (leftCross & 0xffffu) + (rightCross & 0xffffu);
    return uint2(
        (lowProduct & 0xffffu) | (middle << 16u),
        leftHigh * rightHigh + (leftCross >> 16u) +
            (rightCross >> 16u) + (middle >> 16u));
}

uint2 Mul64(uint2 left, uint2 right) {
    uint2 lowProduct = Mul32Wide(left.x, right.x);
    return uint2(
        lowProduct.x,
        lowProduct.y + left.x * right.y + left.y * right.x);
}

uint ReferenceNoiseBitV1(uint2 retraceCycle, uint2 outputPixel, uint channel) {
    const uint2 Golden = uint2(0x7f4a7c15u, 0x9e3779b9u);
    const uint2 Mix1 = uint2(0x1ce4e5b9u, 0xbf58476du);
    const uint2 Mix2 = uint2(0x133111ebu, 0x94d049bbu);
    uint2 key = retraceCycle ^ Mul64(outputPixel, Golden) ^
        Mul64(uint2(channel, 0u), Mix1);
    uint2 mixed = Add64(key, Golden);
    mixed = Mul64(mixed ^ ShiftRight64(mixed, 30u), Mix1);
    mixed = Mul64(mixed ^ ShiftRight64(mixed, 27u), Mix2);
    return (mixed ^ ShiftRight64(mixed, 31u)).x & 1u;
}

float GammaDitherChannelBoundedV1(float channel, uint randomBit) {
    uint value = uint(round(saturate(channel) * 255.0f));
    uint quantized = (min(value + randomBit, 255u) >> 1);
    uint expanded = (quantized << 1) | (quantized >> 6);
    return float(expanded) / 255.0f;
}

float4 ApplyGammaDither(float4 color, float4 position) {
    if (gConstants.gammaDither == 0u) {
        return color;
    }
    // A mismatched policy is a loud visible failure, never an approximation.
    if (gConstants.policyVersion != 1u) {
        return float4(1.0f, 0.0f, 1.0f, 1.0f);
    }
    uint x = uint(position.x) - gConstants.outputOriginX;
    uint y = uint(position.y) - gConstants.outputOriginY;
    uint2 pixel = Add64(
        Mul64(uint2(y, 0u), uint2(gConstants.outputWidth, 0u)),
        uint2(x, 0u));
    uint2 seed = uint2(gConstants.noiseSeedLow, gConstants.noiseSeedHigh);
    color.r = GammaDitherChannelBoundedV1(color.r, ReferenceNoiseBitV1(seed, pixel, 0u));
    color.g = GammaDitherChannelBoundedV1(color.g, ReferenceNoiseBitV1(seed, pixel, 1u));
    color.b = GammaDitherChannelBoundedV1(color.b, ReferenceNoiseBitV1(seed, pixel, 2u));
    return color;
}

uint2 SourceExtent() {
    return max(
        min(uint2(round(gConstants.videoResolution)),
            uint2(round(gConstants.textureResolution))),
        uint2(1u, 1u));
}

bool HasFullCoverage(float alpha) {
    uint encoded = uint(round(saturate(alpha) * float(gConstants.coverageRange)));
    return (encoded & 7u) == 7u;
}

float Median(float left, float center, float right) {
    return max(min(left, center), min(max(left, center), right));
}

uint3 Rgba16Components(float3 rgb) {
    return uint3(round(saturate(rgb) * 255.0f)) >> 3u;
}

int CompareFiveBit(uint neighbor, uint center) {
    return (neighbor > center) ? 1 : ((neighbor < center) ? -1 : 0);
}

void AccumulateRestoration(inout int3 restored, uint3 center, uint3 neighbor) {
    restored.x += CompareFiveBit(neighbor.x, center.x);
    restored.y += CompareFiveBit(neighbor.y, center.y);
    restored.z += CompareFiveBit(neighbor.z, center.z);
}

// US 5,699,079 specifies the signed 3x3-neighbor comparisons used to
// reconstruct full-coverage RGBA16 components. RT64's managed target is not
// an authoritative N64 storage image, so source reconstruction and lattice
// identity remain explicitly bounded even though this integer kernel is exact.
float4 RestoredTexel(int2 coordinate) {
    int2 extent = int2(SourceExtent());
    coordinate = clamp(coordinate, int2(0, 0), extent - int2(1, 1));
    float4 raw = gInput.Load(int3(coordinate, 0));
    if ((gConstants.ditherFilter == 0u) || !HasFullCoverage(raw.a)) {
        return raw;
    }

    uint3 center = Rgba16Components(raw.rgb);
    int3 restored = int3(center << 3u);
    for (int deltaY = -1; deltaY <= 1; deltaY++) {
        for (int deltaX = -1; deltaX <= 1; deltaX++) {
            if ((deltaX == 0) && (deltaY == 0)) {
                continue;
            }
            int2 neighborCoordinate = coordinate + int2(deltaX, deltaY);
            if (any(neighborCoordinate < int2(0, 0)) ||
                any(neighborCoordinate >= extent)) {
                continue;
            }
            uint3 neighbor = Rgba16Components(
                gInput.Load(int3(neighborCoordinate, 0)).rgb);
            AccumulateRestoration(restored, center, neighbor);
        }
    }
    raw.rgb = float3(restored) / 255.0f;
    return raw;
}

uint2 NearestSourceCoordinate(float2 uv) {
    return min(
        uint2(floor(uv * gConstants.textureResolution)),
        SourceExtent() - uint2(1u, 1u));
}

float4 RestoredNearest(float2 uv, uint2 coordinate) {
    float4 raw = gInput.SampleLevel(gSampler, uv, 0);
    if ((gConstants.ditherFilter == 0u) || !HasFullCoverage(raw.a)) {
        return raw;
    }

    uint3 center = Rgba16Components(raw.rgb);
    int3 restored = int3(center << 3u);
    int2 extent = int2(SourceExtent());
    int2 centerCoordinate = int2(coordinate);
    for (int deltaY = -1; deltaY <= 1; deltaY++) {
        for (int deltaX = -1; deltaX <= 1; deltaX++) {
            if ((deltaX == 0) && (deltaY == 0)) {
                continue;
            }
            int2 neighborCoordinate = centerCoordinate + int2(deltaX, deltaY);
            if (any(neighborCoordinate < int2(0, 0)) ||
                any(neighborCoordinate >= extent)) {
                continue;
            }
            float2 neighborUv = uv +
                float2(deltaX, deltaY) / gConstants.textureResolution;
            uint3 neighbor = Rgba16Components(
                gInput.SampleLevel(gSampler, neighborUv, 0).rgb);
            AccumulateRestoration(restored, center, neighbor);
        }
    }
    raw.rgb = float3(restored) / 255.0f;
    return raw;
}

// US 6,166,748 specifies a componentwise horizontal median on or adjacent to
// a silhouette edge. RT64's color-target alpha is its modulo-eight coverage
// estimate; code seven denotes full coverage. This stage intentionally runs
// on the resolved texture lattice before VI resampling and gamma.
float4 DivotTexel(int2 coordinate) {
    int2 maximum = int2(SourceExtent()) - int2(1, 1);
    coordinate = clamp(coordinate, int2(0, 0), maximum);
    float4 center = RestoredTexel(coordinate);
    if ((coordinate.x == 0) || (coordinate.x == maximum.x)) {
        return center;
    }

    float4 left = RestoredTexel(coordinate + int2(-1, 0));
    float4 right = RestoredTexel(coordinate + int2(1, 0));
    if (HasFullCoverage(left.a) && HasFullCoverage(center.a) &&
        HasFullCoverage(right.a)) {
        return center;
    }

    center.r = Median(left.r, center.r, right.r);
    center.g = Median(left.g, center.g, right.g);
    center.b = Median(left.b, center.b, right.b);
    return center;
}

float4 DivotNearest(float2 uv) {
    uint2 sourceCoordinate = NearestSourceCoordinate(uv);
    float4 center = RestoredNearest(uv, sourceCoordinate);
    uint sourceX = sourceCoordinate.x;
    if ((sourceX == 0u) || (sourceX + 1u == SourceExtent().x)) {
        return center;
    }

    float2 horizontal = float2(1.0f, 0.0f) / gConstants.textureResolution;
    float4 left = RestoredNearest(
        uv - horizontal, sourceCoordinate - uint2(1u, 0u));
    float4 right = RestoredNearest(
        uv + horizontal, sourceCoordinate + uint2(1u, 0u));
    if (HasFullCoverage(left.a) && HasFullCoverage(center.a) &&
        HasFullCoverage(right.a)) {
        return center;
    }

    center.r = Median(left.r, center.r, right.r);
    center.g = Median(left.g, center.g, right.g);
    center.b = Median(left.b, center.b, right.b);
    return center;
}

float4 SampleRestored(float2 uv) {
    const float2 LowerRight = gConstants.videoResolution / gConstants.textureResolution;
    const float2 HalfPixel = float2(0.5f, 0.5f) / gConstants.textureResolution;
    float2 boundedUv = clamp(uv, HalfPixel, LowerRight - HalfPixel);

    if (gConstants.filtering == 0u) {
        return RestoredNearest(boundedUv, NearestSourceCoordinate(boundedUv));
    }

    float2 texelPosition = boundedUv * gConstants.textureResolution - 0.5f;
    int2 lower = int2(floor(texelPosition));
    float2 fraction = frac(texelPosition);
    float4 upperLeft = RestoredTexel(lower);
    float4 upperRight = RestoredTexel(lower + int2(1, 0));
    float4 lowerLeft = RestoredTexel(lower + int2(0, 1));
    float4 lowerRight = RestoredTexel(lower + int2(1, 1));
    return lerp(
        lerp(upperLeft, upperRight, fraction.x),
        lerp(lowerLeft, lowerRight, fraction.x),
        fraction.y);
}

float4 SampleDivot(float2 uv) {
    const float2 LowerRight = gConstants.videoResolution / gConstants.textureResolution;
    const float2 HalfPixel = float2(0.5f, 0.5f) / gConstants.textureResolution;
    float2 boundedUv = clamp(uv, HalfPixel, LowerRight - HalfPixel);

    if (gConstants.filtering == 0u) {
        // Preserve the graphics API's exact half-texel tie decision. Integer
        // reconstruction can select a different row at a sampler boundary.
        return DivotNearest(boundedUv);
    }

    float2 texelPosition = boundedUv * gConstants.textureResolution - 0.5f;
    int2 lower = int2(floor(texelPosition));
    float2 fraction = frac(texelPosition);
    float4 upperLeft = DivotTexel(lower);
    float4 upperRight = DivotTexel(lower + int2(1, 0));
    float4 lowerLeft = DivotTexel(lower + int2(0, 1));
    float4 lowerRight = DivotTexel(lower + int2(1, 1));
    return lerp(
        lerp(upperLeft, upperRight, fraction.x),
        lerp(lowerLeft, lowerRight, fraction.x),
        fraction.y);
}

// Limit texture sampling to the area the VI can sample of the texture.
float4 SampleInput(float2 uv) {
    const float2 LowerRight = gConstants.videoResolution / gConstants.textureResolution;
    const float2 HalfPixel = float2(0.5f, 0.5f) / gConstants.textureResolution;
    float2 outsideBorder = step(LowerRight, uv);
    float4 sampledColor;
    if (gConstants.divot != 0u) {
        sampledColor = SampleDivot(uv);
    }
    else if (gConstants.ditherFilter != 0u) {
        sampledColor = SampleRestored(uv);
    }
    else {
        sampledColor = gInput.SampleLevel(
            gSampler, clamp(uv, HalfPixel, LowerRight - HalfPixel), 0);
    }
    float4 gammaCorrectedColor = pow(sampledColor, gConstants.gamma);
    gammaCorrectedColor.rgb *= max(1.0f - outsideBorder.x - outsideBorder.y, 0.0f);
    gammaCorrectedColor.a = 1.0f;
    return gammaCorrectedColor;
}

// Sourced by pinned MIT RT64 from https://www.shadertoy.com/view/csX3RH.
float4 PixelAntialiasing(float2 uv) {
    float2 uvTexspace = uv * gConstants.videoResolution;
    float2 seam = floor(uvTexspace + 0.5f);
    uvTexspace = (uvTexspace - seam) / fwidth(uvTexspace) + seam;
    uvTexspace = clamp(uvTexspace, seam - 0.5f, seam + 0.5f);
    return SampleInput(uvTexspace / gConstants.textureResolution);
}

float4 PSMain(in float4 pos : SV_Position, in float2 uv : TEXCOORD0) : SV_TARGET {
#ifdef PIXEL_ANTIALIASING
    float4 color = PixelAntialiasing(uv);
#else
    float4 color = SampleInput((uv / gConstants.textureResolution) * gConstants.videoResolution);
#endif
    return ApplyGammaDither(color, pos);
}
