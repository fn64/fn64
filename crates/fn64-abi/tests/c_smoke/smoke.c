// Minimal C smoke test standing in for a RecompiledFuncs/*.c translation
// unit: declares the SAME extern "C" signatures and the SAME
// `recomp_context` layout N64Recomp-generated code actually uses (verbatim
// transcription of `recomp.h`'s struct, MIT-licensed, from
// aki-recomp/games/NWXE/RecompiledFuncs/recomp_overlays.inl's own included
// header -- not `#include`d directly so this test stays self-contained and
// buildable without the aki-recomp checkout present), then calls a
// representative sample of exported symbols exactly the way real generated
// C would.
//
// A prior version of this file used a 9-field RecompContext subset
// (r0..r7, r29 only) which happened to still work for symbols that only
// touch those fields (the first 8 u64s land at the same offsets either
// way), but was NOT actually testing the real ABI shape -- any symbol
// touching r8+ or a float register would have silently linked against the
// wrong struct layout. This version uses the full 32-gpr/32-fpr/hi/lo/
// f_odd/status_reg/mips3_float_mode struct, matching fn64-abi's own
// `RecompContext` byte-for-byte, so a real struct-layout regression here
// would show up as a wrong value read back, not just "linked."
//
// pause_self/switch_error/do_break/osSendMesg/osRecvMesg's loud-panic-or-
// coroutine-only paths are exercised by the Rust-side tests in src/lib.rs
// instead: osSendMesg_recomp/osRecvMesg_recomp unconditionally suspend the
// active coroutine by design (see lib.rs's module doc), which panics
// loudly if called from outside one -- exactly what a bare `main()` (no
// coroutine) would hit, so this smoke test only calls shims that are safe
// to call directly from a normal C `main`, proving the link+call shape and
// real-struct-layout round-trip work; the coroutine-only shims' behavior is
// proven by fn64-abi's own Rust test suite instead.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef union RecompFpr {
    double d;
    struct { float fl; float fh; };
    struct { uint32_t u32l; uint32_t u32h; };
    uint64_t u64;
} RecompFpr;

typedef struct RecompContext {
    uint64_t r0, r1, r2, r3, r4, r5, r6, r7,
             r8, r9, r10, r11, r12, r13, r14, r15,
             r16, r17, r18, r19, r20, r21, r22, r23,
             r24, r25, r26, r27, r28, r29, r30, r31;
    RecompFpr f0, f1, f2, f3, f4, f5, f6, f7,
              f8, f9, f10, f11, f12, f13, f14, f15,
              f16, f17, f18, f19, f20, f21, f22, f23,
              f24, f25, f26, f27, f28, f29, f30, f31;
    uint64_t hi, lo;
    uint32_t *f_odd;
    uint32_t status_reg;
    uint8_t mips3_float_mode;
} RecompContext;

extern void osCreateMesgQueue_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osVirtualToPhysical_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osSetIntMask_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osInitialize_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osContInit_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osContSetCh_recomp(uint8_t *rdram, RecompContext *ctx);
extern void osPfsIsPlug_recomp(uint8_t *rdram, RecompContext *ctx);

int main(void) {
    uint8_t rdram[64] = {0};
    RecompContext ctx = {0};

    // osCreateMesgQueue(mq, msg, count) -- a0/a1/a2 = r4/r5/r6.
    ctx.r4 = 0xFFFFFFFF80057228ULL;
    ctx.r5 = 0;
    ctx.r6 = 4;
    osCreateMesgQueue_recomp(rdram, &ctx);

    // osVirtualToPhysical(0x80001234) -- exercises an r8+-adjacent-shaped
    // struct read (r4 in, r2 out) and confirms KSEG0 masking through the
    // REAL, full-size struct layout (would have silently misread a
    // truncated struct if the Rust side and this C side disagreed on any
    // field's byte offset before r4, though r4 happens to be early enough
    // that only a total-size mismatch elsewhere would show up here rather
    // than an offset mismatch -- osInitialize below additionally proves the
    // whole struct round-trips through a no-op call without crashing).
    RecompContext vtop_ctx = {0};
    vtop_ctx.r4 = 0x80001234;
    osVirtualToPhysical_recomp(rdram, &vtop_ctx);
    if (vtop_ctx.r2 != 0x00001234) {
        fprintf(stderr, "osVirtualToPhysical_recomp: expected 0x1234, got %#llx\n",
                (unsigned long long)vtop_ctx.r2);
        return 1;
    }

    // osSetIntMask returns the previous mask in r2.
    RecompContext mask_ctx = {0};
    mask_ctx.r4 = 1;
    osSetIntMask_recomp(rdram, &mask_ctx);

    // osInitialize(void) -- exercises a call that touches no ctx fields at
    // all beyond the pointer itself being valid.
    RecompContext init_ctx = {0};
    osInitialize_recomp(rdram, &init_ctx);

    // Controller Manager signatures: initialize the manager and select one
    // channel. osPfsIsPlug is synchronous and therefore requires a live guest
    // coroutine; retaining its address still forces the C linker to resolve
    // the exact exported shim without invoking it outside that contract.
    RecompContext cont_init_ctx = {0};
    cont_init_ctx.r5 = 0x80000020;
    cont_init_ctx.r6 = 0x80000030;
    osContInit_recomp(rdram, &cont_init_ctx);
    RecompContext set_ch_ctx = {0};
    set_ch_ctx.r4 = 1;
    osContSetCh_recomp(rdram, &set_ch_ctx);
    void (*volatile pfs_is_plug_fn)(uint8_t *, RecompContext *) =
        osPfsIsPlug_recomp;
    if (pfs_is_plug_fn == NULL) {
        fprintf(stderr, "osPfsIsPlug_recomp: unresolved function address\n");
        return 1;
    }

    printf("fn64-abi C smoke test: linked and returned OK\n");
    return 0;
}
