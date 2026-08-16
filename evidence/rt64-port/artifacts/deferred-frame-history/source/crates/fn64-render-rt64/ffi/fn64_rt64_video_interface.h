// fn64-owned extension of pinned RT64's VI push constants.
#pragma once

#include "shared/rt64_hlsl.h"

#ifdef HLSL_CPU
namespace interop {
#endif
    struct VideoInterfaceCB {
        float2 videoResolution;
        float2 textureResolution;
        float gamma;
        uint gammaDither;
        uint outputOriginX;
        uint outputOriginY;
        uint outputWidth;
        uint noiseSeedLow;
        uint noiseSeedHigh;
        uint policyVersion;
        uint divot;
        uint coverageRange;
        uint filtering;
        uint viFilterFlags;
    };
    static const uint ViFilterDitherRestoration = 1u << 0u;
    static const uint ViFilterSilhouetteAa = 1u << 1u;
    static const uint ViFilterRgba16 = 1u << 2u;
    static const uint ViFilterSerratedRows = 1u << 3u;
#ifdef HLSL_CPU
};
#endif
