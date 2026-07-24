// sm64-boot: linkable stubs for the 9 libultra/internal functions that
// SM64's RecompiledFuncs corpus REFERENCES with a direct C call but does not
// itself DEFINE, and for which fn64-abi provides no host adapter either.
//
// These are the low-level libultra internals N64Recomp declined to emit
// (chiefly because their bodies use VR4300 instructions the recompiler cannot
// translate -- TLB ops, cop0 reads, 64-bit shifts), while other recompiled
// functions still emit a direct `symbol_recomp(rdram, ctx)` call to them.
// Their guest VAs (from SM64U/syms/dump.toml, all inside the always-resident
// `main` segment 0x80246000..0x8032D560):
//
//   __ll_lshift        0x80324194   send_mesg          0x80327b98
//   string_to_u32      0x8032a890   __osEnqueueThread  0x80327d10
//   __osDispatchThread 0x80327d68   __osGetCause       0x8032b1f0
//   osMapTLB           0x803223e0   osUnmapTLBAll      0x803224a0
//   send               0x8032a9a8
//
// Bring-up policy: each is a LOUD panic stub. If SM64's boot actually reaches
// one, the process aborts with the exact symbol name -- a real, precise
// finding (either fn64-abi needs a host shim for that libultra internal, or
// SM64's corpus must be regenerated to emit its body) rather than a silent
// wrong-answer. This lets the harness LINK and boot thread 0, and reports the
// first blocker exactly, per the task's "report precisely where it stops."
//
// This file contains ZERO game content -- only symbol names (which are
// libultra/decomp public identifiers, not copyrighted game data) and a panic
// message. It lives in the harness, not in aki-recomp.

#include <stdio.h>
#include <stdlib.h>

#include "recomp.h"

// The generated corpus declares every `*_recomp` symbol inside `extern "C"`
// (via funcs.h), so these definitions must have C linkage too -- the bridge is
// compiled as C++, which would otherwise name-mangle them and leave the
// callers' references unresolved.
#ifdef __cplusplus
extern "C" {
#endif

#define FN64_MISSING_STUB(sym, va)                                             \
    void sym##_recomp(uint8_t* rdram, recomp_context* ctx) {                   \
        (void)rdram;                                                           \
        (void)ctx;                                                            \
        fprintf(stderr,                                                        \
                "[sm64-boot] FATAL: SM64 boot reached the un-emitted libultra "\
                "internal `" #sym "` (guest VA " #va ") -- the recompiler did "\
                "not emit its body and fn64-abi has no host adapter for it. "  \
                "This is the first real boot blocker: fn64 needs a host shim "  \
                "for " #sym ", or SM64's corpus must be regenerated to emit "   \
                "it. See examples/sm64-boot/bridge/sm64_missing_stubs.c.\n");  \
        abort();                                                              \
    }

FN64_MISSING_STUB(__ll_lshift, 0x80324194)
FN64_MISSING_STUB(string_to_u32, 0x8032a890)
FN64_MISSING_STUB(__osDispatchThread, 0x80327d68)
FN64_MISSING_STUB(__osEnqueueThread, 0x80327d10)
FN64_MISSING_STUB(__osGetCause, 0x8032b1f0)
FN64_MISSING_STUB(osMapTLB, 0x803223e0)
FN64_MISSING_STUB(osUnmapTLBAll, 0x803224a0)
FN64_MISSING_STUB(send_mesg, 0x80327b98)
FN64_MISSING_STUB(send, 0x8032a9a8)

#ifdef __cplusplus
} // extern "C"
#endif
