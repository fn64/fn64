// fn64's bounded native RDP alpha-dither policy.
//
// The selector topology and threshold values come from the pinned MIT RT64
// OtherMode and Formats helpers. This overlay deliberately consumes the
// fragment's current random state directly: it must not advance the stream a
// second time after combiner NOISE or G_AC_DITHER has consumed it.

#pragma once

float Fn64RdpAlphaDitherCombinerAlpha(
    OtherMode otherMode,
    float combinerAlpha,
    uint2 fragmentCoordinate,
    uint fragmentRandomState)
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
        fragmentRandomState);
    const uint alpha8 = uint(round(clamp(combinerAlpha, 0.0f, 1.0f) * 255.0f));
    const uint rounded5 = min(
        (alpha8 >> 3U) + (((alpha8 & 7U) > threshold) ? 1U : 0U),
        31U);
    const uint expanded8 = (rounded5 << 3U) | (rounded5 >> 2U);
    return float(expanded8) / 255.0f;
}
