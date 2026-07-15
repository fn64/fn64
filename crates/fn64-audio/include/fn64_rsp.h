/*
 * fn64_rsp.h — the C ABI the RSPRecomp-generated audio-ucode C compiles
 * against, replacing BOTH GPL headers the generated file #includes
 * (`librecomp/rsp.hpp` + `librecomp/rsp_vu_impl.hpp`, per RSPRecomp's
 * rsp_recomp.cpp lines 1179-1180). Clean-room, spec-derived (see
 * ../RSP-VU-ISA.md); no GPL header was read.
 *
 * This header declares ONLY the surface the generated code references — the
 * types, the `RSP_MEM_*` / scalar-helper macros, the DMA-action macros, and
 * the `RSP` VU object with one method per emitted CP2 op. The bodies live in
 * the Rust crate (fn64-audio); this header is the linkage seam if/when the
 * generated C is compiled and bound to that crate over FFI. It is provided so
 * the generated file has something to include that matches the exact call
 * shapes the recompiler emits.
 *
 * Correspondence to rsp_recomp.cpp's generated code:
 *   - RspExitReason enum + RspContext struct (lines 907-908, 1016-1023).
 *   - RSP_MEM_{B,BU,H,HU,W}_{LOAD,STORE} sub-word accessors (lines 457-481);
 *     the ^2/^3 byte-lane XOR lives in the Rust Dmem, so these macros forward
 *     to the FFI accessors rather than dereferencing rdram directly.
 *   - RSP_ADD32/RSP_SUB32/S32/U32/RSP_SIGNED scalar helpers (lines 381-453).
 *   - SET_DMA_DRAM/SET_DMA_MEM/DO_DMA_READ/DO_DMA_WRITE (lines 149-155).
 *   - The `RSP rsp` object with VMULF<e>(...), VMOV<e>(...), VSAR<e>(...),
 *     VRNDN<e>(...), VMACQ(...), VNOP() etc. (call shapes, lines 316-371).
 *
 * NOTE: RSPRecomp emits C++ (templates `OP<e>(...)`, so the generated file is
 * compiled as C++). This header therefore uses C++ constructs guarded for C++
 * only; the op methods are templates on the element field `e` exactly as the
 * generated calls expect.
 */
#ifndef FN64_RSP_H
#define FN64_RSP_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Why the RSP returned. Matches the generated `return RspExitReason::X`
 * sites; the Rust `RspExitReason` enum mirrors these variants 1:1.
 */
typedef enum RspExitReason {
    RspExitReason_Broke = 0,
    RspExitReason_Unsupported,
    RspExitReason_ImemOverrun,
    RspExitReason_UnhandledJumpTarget,
    RspExitReason_SwapOverlay,
    RspExitReason_UnhandledResumeTarget,
} RspExitReason;

/* Forward decl of the VU object; opaque to the generated C, defined below for
 * C++ so the emitted `rsp.VOP<e>(...)` calls type-check. In C++ `struct RSP`
 * is the tag; in C we also provide the `RSP` typedef alias. */
#ifdef __cplusplus
struct RSP;
#else
typedef struct RSP RSP;
#endif

/*
 * The scalar-side context threaded through the generated ucode. Field names
 * match the generated save/restore code (rsp_recomp.cpp lines 598-606,
 * 1017-1023). r[0] is $zero and is never written by well-formed code.
 */
typedef struct RspContext {
    uint32_t r[32];            /* r1..r31 used; r0 hardwired zero */
    uint32_t dma_dram_address; /* SET_DMA_DRAM */
    uint32_t dma_mem_address;  /* SET_DMA_MEM; &0x1000 => IMEM overlay load */
    uint32_t jump_target;      /* pending jr/jalr target */
    uint32_t resume_address;   /* IMEM addr to resume at after overlay swap */
    uint32_t resume_delay;     /* nonzero if resume point was in a delay slot */
    /* The VU state (`ctx->rsp`, line 1023). Opaque here; the Rust side owns
     * its layout. Held by pointer so this struct stays ABI-stable. */
    RSP *rsp;
} RspContext;

/* --- Scalar helpers the generated core-op code uses (lines 381-453) --- */

/* Signed/unsigned reinterpretation and 32-bit wraparound arithmetic. */
#define S32(x)          ((int32_t)(x))
#define U32(x)          ((uint32_t)(x))
#define RSP_SIGNED(x)   ((int32_t)(x))
#define RSP_ADD32(a, b) ((uint32_t)((uint32_t)(a) + (uint32_t)(b)))
#define RSP_SUB32(a, b) ((uint32_t)((uint32_t)(a) - (uint32_t)(b)))

/* --- DMEM sub-word accessors (lines 457-481) ---
 *
 * The ^2/^3 byte-lane swizzle is implemented in the Rust Dmem; these macros
 * forward to the FFI accessors below so the generated C never re-derives the
 * lane XOR. `base`+`offset` is the RSP data address; the accessors mask it
 * into the 0x1000 DMEM window. */
int32_t  fn64_rsp_mem_w_load(uint32_t addr);
void     fn64_rsp_mem_w_store(uint32_t addr, int32_t value);
int16_t  fn64_rsp_mem_h_load(uint32_t addr);
void     fn64_rsp_mem_h_store(uint32_t addr, int16_t value);
uint16_t fn64_rsp_mem_hu_load(uint32_t addr);
int8_t   fn64_rsp_mem_b_load(uint32_t addr);
void     fn64_rsp_mem_b_store(uint32_t addr, int8_t value);
uint8_t  fn64_rsp_mem_bu_load(uint32_t addr);
void     fn64_rsp_mem_bu_store(uint32_t addr, uint8_t value);

#define RSP_MEM_W_LOAD(offset, base)      fn64_rsp_mem_w_load((base) + (offset))
#define RSP_MEM_W_STORE(offset, base, v)  fn64_rsp_mem_w_store((base) + (offset), (v))
#define RSP_MEM_H_LOAD(offset, base)      fn64_rsp_mem_h_load((base) + (offset))
#define RSP_MEM_H_STORE(offset, base, v)  fn64_rsp_mem_h_store((base) + (offset), (v))
#define RSP_MEM_HU_LOAD(offset, base)     fn64_rsp_mem_hu_load((base) + (offset))
/* The codegen writes RSP_MEM_B(...) as both an rvalue (lb) and an lvalue-ish
 * store `RSP_MEM_B(...) = rt` (line 480); expose read/write helpers and let
 * the generated store site call the store form. */
#define RSP_MEM_B(offset, base)           fn64_rsp_mem_b_load((base) + (offset))
#define RSP_MEM_BU(offset, base)          fn64_rsp_mem_bu_load((base) + (offset))

/* --- DMA action macros (lines 149-155) --- */
void fn64_rsp_set_dma_dram(RspContext *ctx, uint32_t v);
void fn64_rsp_set_dma_mem(RspContext *ctx, uint32_t v);
void fn64_rsp_do_dma_read(RspContext *ctx, uint32_t len);
void fn64_rsp_do_dma_write(RspContext *ctx, uint32_t len);
#define SET_DMA_DRAM(v)  fn64_rsp_set_dma_dram(ctx, (v))
#define SET_DMA_MEM(v)   fn64_rsp_set_dma_mem(ctx, (v))
#define DO_DMA_READ(v)   fn64_rsp_do_dma_read(ctx, (v))
#define DO_DMA_WRITE(v)  fn64_rsp_do_dma_write(ctx, (v))

#ifdef __cplusplus
} /* extern "C" */

/*
 * The VU object the generated C++ calls its CP2 ops on. Each op is a template
 * on the compile-time element field `e` (0..15), matching the emitted
 * `rsp.VMULF<e>(rsp.vpu.r[vd], rsp.vpu.r[vs], rsp.vpu.r[vt])` shape. A vector
 * register is 8 lanes of int16_t; `vpu.r[n]` is the register file. The method
 * bodies are NOT defined here — they forward (over the C ABI, elsewhere) to
 * the fn64-audio Rust op implementations. This class exists so the generated
 * translation unit type-checks against the exact call shapes.
 */
typedef int16_t RspVec8[8];

struct RspVpu {
    RspVec8 r[32];
};

struct RSP {
    RspVpu vpu;

    /* Vd, Vs, Vt group (lines 62-93). */
    template <int e> void VMULF(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMULU(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMULQ(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMUDH(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMUDM(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMUDN(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMUDL(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMACF(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMACU(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMADH(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMADM(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMADN(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMADL(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VADD(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VADDC(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VSUB(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VSUBC(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VABS(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VAND(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VNAND(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VOR(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VNOR(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VXOR(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VNXOR(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VLT(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VEQ(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VNE(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VGE(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VMRG(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VCH(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VCL(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);
    template <int e> void VCR(RspVec8 &vd, const RspVec8 &vs, const RspVec8 &vt);

    /* Vd, Vs (Vt=None): VSAR (line 94). */
    template <int e> void VSAR(RspVec8 &vd, const RspVec8 &vs);

    /* Vd only; e ignored: VMACQ (line 95). */
    void VMACQ(RspVec8 &vd);

    /* Vd, VsIndex, Vt: VRNDN/VRNDP (lines 97-98). */
    template <int e> void VRNDN(RspVec8 &vd, int vs_index, const RspVec8 &vt);
    template <int e> void VRNDP(RspVec8 &vd, int vs_index, const RspVec8 &vt);

    /* Vd, De, Vt: VMOV and the VRCP/VRSQ family (lines 101-107). */
    template <int e> void VMOV(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRCP(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRCPL(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRCPH(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRSQ(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRSQL(RspVec8 &vd, int de, const RspVec8 &vt);
    template <int e> void VRSQH(RspVec8 &vd, int de, const RspVec8 &vt);

    /* No-op; e ignored (line 114). */
    void VNOP();
};
#endif /* __cplusplus */

#endif /* FN64_RSP_H */
