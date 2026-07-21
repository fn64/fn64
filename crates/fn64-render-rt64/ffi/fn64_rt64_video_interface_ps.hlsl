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

uint CoverageCode(float alpha) {
    return uint(round(saturate(alpha) * float(gConstants.coverageRange))) & 7u;
}

bool HasFullCoverage(float alpha) {
    return CoverageCode(alpha) == 7u;
}

bool HasQualifiedPartialCoverage(float alpha) {
    // Pinned RT64 writes its estimated sample count directly for codes 1..6
    // and clamps eight samples to seven. Code zero is ambiguous across
    // modulo-wrap and zero-destination save/unqualified paths, so it cannot
    // identify one sample here.
    uint code = CoverageCode(alpha);
    return (code >= 1u) && (code <= 6u);
}

bool ViFilterEnabled(uint flag) {
    return (gConstants.viFilterFlags & flag) != 0u;
}

float Median(float left, float center, float right) {
    return max(min(left, center), min(max(left, center), right));
}

uint3 Rgba16Components(float3 rgb) {
    return uint3(round(saturate(rgb) * 255.0f)) >> 3u;
}

uint3 SourceComponents(float3 rgb) {
    uint3 value = uint3(round(saturate(rgb) * 255.0f));
    if (!ViFilterEnabled(ViFilterRgba16)) {
        return value;
    }
    uint3 fiveBit = value >> 3u;
    return (fiveBit << 3u) | (fiveBit >> 2u);
}

void AdmitFullCoverageNeighbor(
    float4 neighbor,
    inout uint admitted,
    inout uint3 firstMinimum,
    inout uint3 secondMinimum,
    inout uint3 firstMaximum,
    inout uint3 secondMaximum) {
    if (!HasFullCoverage(neighbor.a)) {
        return;
    }
    uint3 value = SourceComponents(neighbor.rgb);
    uint3 priorMinimum = firstMinimum;
    uint3 priorMaximum = firstMaximum;
    firstMinimum = min(firstMinimum, value);
    secondMinimum = min(secondMinimum, max(priorMinimum, value));
    firstMaximum = max(firstMaximum, value);
    secondMaximum = max(secondMaximum, min(priorMaximum, value));
    admitted++;
}

// US 5,742,277 Figure 11 and Equation 4 define the preferred six-neighbor
// silhouette filter. RT64's managed target supplies only its own modulo-eight
// coverage estimate. The topology is public; 5-to-8 expansion, saturation,
// and round-to-nearest are named bounded integer policies.
float4 FinishCoverageAa(
    float4 raw,
    uint admitted,
    uint3 secondMinimum,
    uint3 secondMaximum) {
    uint3 foreground = SourceComponents(raw.rgb);
    if (admitted < 3u) {
        // The preferred penultimate interval is undefined. Preserve the
        // source-format foreground through the named bounded fallback.
        raw.rgb = float3(foreground) / 255.0f;
        return raw;
    }
    uint3 low = min(foreground, secondMinimum);
    uint3 high = max(foreground, secondMaximum);
    int3 backgroundSigned = clamp(
        int3(low) + int3(high) - int3(foreground),
        int3(0, 0, 0),
        int3(255, 255, 255));
    uint coverage = CoverageCode(raw.a);
    uint3 filtered =
        (coverage * foreground + (8u - coverage) * uint3(backgroundSigned) + 4u) >> 3u;
    raw.rgb = float3(filtered) / 255.0f;
    return raw;
}

void AdmitTexelNeighbor(
    int2 coordinate,
    int2 extent,
    inout uint admitted,
    inout uint3 firstMinimum,
    inout uint3 secondMinimum,
    inout uint3 firstMaximum,
    inout uint3 secondMaximum) {
    if (any(coordinate < int2(0, 0)) || any(coordinate >= extent)) {
        return;
    }
    AdmitFullCoverageNeighbor(
        gInput.Load(int3(coordinate, 0)),
        admitted,
        firstMinimum,
        secondMinimum,
        firstMaximum,
        secondMaximum);
}

float4 CoverageAaTexel(float4 raw, int2 coordinate, int2 extent) {
    uint admitted = 0u;
    uint3 firstMinimum = uint3(256u, 256u, 256u);
    uint3 secondMinimum = firstMinimum;
    uint3 firstMaximum = uint3(0u, 0u, 0u);
    uint3 secondMaximum = firstMaximum;
    int rowStride = ViFilterEnabled(ViFilterSerratedRows) ? 2 : 1;
    AdmitTexelNeighbor(coordinate + int2(-1, -rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitTexelNeighbor(coordinate + int2(1, -rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitTexelNeighbor(coordinate + int2(-2, 0), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitTexelNeighbor(coordinate + int2(2, 0), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitTexelNeighbor(coordinate + int2(-1, rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitTexelNeighbor(coordinate + int2(1, rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    return FinishCoverageAa(raw, admitted, secondMinimum, secondMaximum);
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
float4 FilteredTexel(int2 coordinate) {
    int2 extent = int2(SourceExtent());
    coordinate = clamp(coordinate, int2(0, 0), extent - int2(1, 1));
    float4 raw = gInput.Load(int3(coordinate, 0));
    if (ViFilterEnabled(ViFilterSilhouetteAa) && HasQualifiedPartialCoverage(raw.a)) {
        return CoverageAaTexel(raw, coordinate, extent);
    }
    if (!ViFilterEnabled(ViFilterDitherRestoration) || !HasFullCoverage(raw.a)) {
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

void AdmitNearestNeighbor(
    float2 uv,
    int2 coordinate,
    int2 extent,
    inout uint admitted,
    inout uint3 firstMinimum,
    inout uint3 secondMinimum,
    inout uint3 firstMaximum,
    inout uint3 secondMaximum) {
    if (any(coordinate < int2(0, 0)) || any(coordinate >= extent)) {
        return;
    }
    AdmitFullCoverageNeighbor(
        gInput.SampleLevel(gSampler, uv, 0),
        admitted,
        firstMinimum,
        secondMinimum,
        firstMaximum,
        secondMaximum);
}

float4 CoverageAaNearest(float4 raw, float2 uv, uint2 coordinate) {
    uint admitted = 0u;
    uint3 firstMinimum = uint3(256u, 256u, 256u);
    uint3 secondMinimum = firstMinimum;
    uint3 firstMaximum = uint3(0u, 0u, 0u);
    uint3 secondMaximum = firstMaximum;
    int rowStride = ViFilterEnabled(ViFilterSerratedRows) ? 2 : 1;
    int2 center = int2(coordinate);
    int2 extent = int2(SourceExtent());
    float2 texel = 1.0f / gConstants.textureResolution;
    AdmitNearestNeighbor(uv + float2(-1, -rowStride) * texel,
        center + int2(-1, -rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitNearestNeighbor(uv + float2(1, -rowStride) * texel,
        center + int2(1, -rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitNearestNeighbor(uv + float2(-2, 0) * texel,
        center + int2(-2, 0), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitNearestNeighbor(uv + float2(2, 0) * texel,
        center + int2(2, 0), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitNearestNeighbor(uv + float2(-1, rowStride) * texel,
        center + int2(-1, rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    AdmitNearestNeighbor(uv + float2(1, rowStride) * texel,
        center + int2(1, rowStride), extent, admitted,
        firstMinimum, secondMinimum, firstMaximum, secondMaximum);
    return FinishCoverageAa(raw, admitted, secondMinimum, secondMaximum);
}

float4 FilteredNearest(float2 uv, uint2 coordinate) {
    float4 raw = gInput.SampleLevel(gSampler, uv, 0);
    if (ViFilterEnabled(ViFilterSilhouetteAa) && HasQualifiedPartialCoverage(raw.a)) {
        return CoverageAaNearest(raw, uv, coordinate);
    }
    if (!ViFilterEnabled(ViFilterDitherRestoration) || !HasFullCoverage(raw.a)) {
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
    float4 center = FilteredTexel(coordinate);
    if ((coordinate.x == 0) || (coordinate.x == maximum.x)) {
        return center;
    }

    float4 left = FilteredTexel(coordinate + int2(-1, 0));
    float4 right = FilteredTexel(coordinate + int2(1, 0));
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
    float4 center = FilteredNearest(uv, sourceCoordinate);
    uint sourceX = sourceCoordinate.x;
    if ((sourceX == 0u) || (sourceX + 1u == SourceExtent().x)) {
        return center;
    }

    float2 horizontal = float2(1.0f, 0.0f) / gConstants.textureResolution;
    float4 left = FilteredNearest(
        uv - horizontal, sourceCoordinate - uint2(1u, 0u));
    float4 right = FilteredNearest(
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

float4 SampleFiltered(float2 uv) {
    const float2 LowerRight = gConstants.videoResolution / gConstants.textureResolution;
    const float2 HalfPixel = float2(0.5f, 0.5f) / gConstants.textureResolution;
    float2 boundedUv = clamp(uv, HalfPixel, LowerRight - HalfPixel);

    if (gConstants.filtering == 0u) {
        return FilteredNearest(boundedUv, NearestSourceCoordinate(boundedUv));
    }

    float2 texelPosition = boundedUv * gConstants.textureResolution - 0.5f;
    int2 lower = int2(floor(texelPosition));
    float2 fraction = frac(texelPosition);
    float4 upperLeft = FilteredTexel(lower);
    float4 upperRight = FilteredTexel(lower + int2(1, 0));
    float4 lowerLeft = FilteredTexel(lower + int2(0, 1));
    float4 lowerRight = FilteredTexel(lower + int2(1, 1));
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
    else if ((gConstants.viFilterFlags &
              (ViFilterDitherRestoration | ViFilterSilhouetteAa)) != 0u) {
        sampledColor = SampleFiltered(uv);
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
