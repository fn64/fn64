// Minimal C smoke test standing in for a RecompiledFuncs/*.c translation
// unit: declares the same extern "C" signature fn64-abi exports and calls
// it, exactly the way N64Recomp-generated code would call a `_recomp`
// shim -- (uint8_t *rdram, recomp_context *ctx), per
// aki-recomp/runtime/ABI-SURFACE.md section (b)/(a).
//
// This only exercises osCreateMesgQueue_recomp (the non-panicking path);
// pause_self/osSendMesg's loud-stub paths are covered by Rust-side
// #[should_panic] tests in src/lib.rs, not here, since a smoke test's job
// is proving the link+call shape works, not exercising every behavior.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct RecompContext {
    uint64_t r0, r1, r2, r3, r4, r5, r6, r7;
} RecompContext;

extern void osCreateMesgQueue_recomp(uint8_t *rdram, RecompContext *ctx);

int main(void) {
    uint8_t rdram[64] = {0};
    RecompContext ctx = {0};

    // a0 = mq address (sign-extended KSEG0 form, as recomp_context's gpr
    // fields carry it per ABI-SURFACE.md section (b)), a1 = msg buffer
    // (unused by this shim's smoke path), a2 = count.
    ctx.r4 = 0xFFFFFFFF80057228ULL;
    ctx.r5 = 0;
    ctx.r6 = 4;

    osCreateMesgQueue_recomp(rdram, &ctx);

    printf("fn64-abi C smoke test: osCreateMesgQueue_recomp linked and returned OK\n");
    return 0;
}
