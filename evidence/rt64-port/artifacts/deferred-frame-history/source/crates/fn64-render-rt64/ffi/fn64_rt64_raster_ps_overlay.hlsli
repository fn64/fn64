// fn64's bounded native RDP noise/dither policy.
//
// The selector topology and threshold values come from the pinned MIT RT64
// OtherMode and Formats helpers. The typed sample makes fn64's bounded shared
// fragment-routing policy explicit and prevents independent consumer draws.

#pragma once

struct Fn64RdpFragmentNoiseSample {
    uint raw;
};

Fn64RdpFragmentNoiseSample Fn64RdpTakeFragmentNoiseSample(
    inout uint fragmentRandomState)
{
    nextRandUint(fragmentRandomState);
    Fn64RdpFragmentNoiseSample sample;
    sample.raw = fragmentRandomState;
    return sample;
}

float Fn64RdpFragmentNoiseUnitFloat(Fn64RdpFragmentNoiseSample sample)
{
    return float(sample.raw & 0x00FFFFFFU) / float(0x01000000U);
}

uint Fn64RdpFragmentNoiseLowThreeBits(Fn64RdpFragmentNoiseSample sample)
{
    return sample.raw & 7U;
}

float Fn64RdpAlphaDitherCombinerAlpha(
    OtherMode otherMode,
    float combinerAlpha,
    uint2 fragmentCoordinate,
    Fn64RdpFragmentNoiseSample fragmentNoise)
{
    if (otherMode.alphaDither() == G_AD_DISABLE) {
        return combinerAlpha;
    }

    const uint rgbDither =
        (otherMode.rgbDither() >> G_MDSFT_RGBDITHER) & 0x3U;
    const uint alphaDither =
        (otherMode.alphaDither() >> G_MDSFT_ALPHADITHER) & 0x3U;
    const uint threshold = AlphaDitherValue(
        rgbDither,
        alphaDither,
        fragmentCoordinate,
        Fn64RdpFragmentNoiseLowThreeBits(fragmentNoise));
    const uint alpha8 = uint(round(clamp(combinerAlpha, 0.0f, 1.0f) * 255.0f));
    const uint rounded5 = min(
        (alpha8 >> 3U) + (((alpha8 & 7U) > threshold) ? 1U : 0U),
        31U);
    const uint expanded8 = (rounded5 << 3U) | (rounded5 >> 2U);
    return float(expanded8) / 255.0f;
}
