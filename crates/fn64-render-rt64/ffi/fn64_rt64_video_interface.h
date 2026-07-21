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
        uint reserved;
    };
#ifdef HLSL_CPU
};
#endif
