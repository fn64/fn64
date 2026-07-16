# RSP VU differential verification

This is the evidence record for `tests/rsp_vu_differential.rs`. It is a
clean-room, instruction-boundary differential oracle: fn64's production
typed-Rust dispatcher and vector-memory runtime execute the same deterministic
inputs as a separate test-only semantic model, and the test compares every
architecturally visible result after each instruction.

## Sources and independence

The semantic model directly evaluates the equations in the public
[SGI Nintendo 64 RSP Programmer's Guide](https://ultra64.ca/files/documentation/silicon-graphics/SGI_Nintendo_64_RSP_Programmers_Guide.pdf),
revision 1.0. Relevant locations are Chapter 3 pp. 48–79 and the instruction
appendix pp. 178–195, 205–231, 240–245, 259–270, 284–288, and 300–314.

Algorithm structure and boundary behavior were independently checked against
the BSD-3-Clause CEN64 source pinned at commit
[`e0641c8452a3ae8edcd2bf4e46794bb4eaafc076`](https://github.com/n64dev/cen64/tree/e0641c8452a3ae8edcd2bf4e46794bb4eaafc076),
especially its [clamp helpers](https://github.com/n64dev/cen64/blob/e0641c8452a3ae8edcd2bf4e46794bb4eaafc076/arch/x86_64/rsp/clamp.h),
[clip operations](https://github.com/n64dev/cen64/blob/e0641c8452a3ae8edcd2bf4e46794bb4eaafc076/arch/x86_64/rsp/vcl.h),
[divider](https://github.com/n64dev/cen64/blob/e0641c8452a3ae8edcd2bf4e46794bb4eaafc076/arch/x86_64/rsp/vrcpsq.c),
and [reciprocal ROM](https://github.com/n64dev/cen64/blob/e0641c8452a3ae8edcd2bf4e46794bb4eaafc076/common/reciprocal.c).
Vector-memory addressing was also checked against the BSD-3-Clause MAME RSP
interpreter pinned at commit
[`24b318ed57ed4deb5638f9b8347cd8bf8a772a7b`](https://github.com/mamedev/mame/blob/24b318ed57ed4deb5638f9b8347cd8bf8a772a7b/src/devices/cpu/rsp/rsp.cpp).
No GPL runtime implementation was read.

The reference side owns its register file, signed 48-bit accumulator lanes,
flags, divider latches, logical DMEM, element-selection table, clamp equations,
integer-square-root routine, and generated ROM. It does not call fn64's clamp,
element-selection, ROM, operation, or vector-memory helpers. Only public
instruction and state types form the comparison seam.

## Differential matrix

- All 44 non-reserved vector compute operations run from three boundary states
  spanning sign extrema, saturation edges, accumulator carry/sign-extension
  boundaries, and every VCO/VCC/VCE bit pattern. Vector operations exercise all
  16 element encodings, including all scalar divider/move aliases.
- Reciprocal and inverse-square-root run explicit signed 16-bit extrema and
  zero, plus paired high/low inputs at signed 32-bit extrema, one's-complement
  transition boundaries, zero, and normalization boundaries.
- Stateful streams compare the full state after every instruction for
  `VSUBC→VLT→VCH→VCL→VCR→VMRG`,
  `VMULF→VMACF→VMADH→VMACQ→VSAR`, and the high/low reciprocal and
  inverse-square-root latch sequences.
- Every result compares all 32 vector registers, all eight signed 48-bit ACC
  lanes, VCO, VCC, VCE, and the divider input-valid/input/output latches.
- All 11 vector loads and 12 vector stores run at six address/alignment
  boundaries, including DMEM wrap, using the valid boundary element encodings.
  LTV/STV also run where fewer than eight consecutive registers remain. The
  comparison covers the entire vector register file and all 4096 logical DMEM
  bytes.

## Delay-slot oracle parity

The recompiler round-trip tests and `rsp_trace` use a decoded-instruction
interpreter over the same typed `RspMachine` that emitted Rust calls. This is a
control-flow parity oracle rather than the independent VU semantic model above:
for each branch or jump, it must execute the same delay-slot operation the
emitter inlines before transferring control.

The delay-slot match is exhaustive over `Instr`. It executes scalar ALU,
shifts, loads and stores; MFC0/MTC0; MFC2/MTC2/CFC2/CTC2; vector loads and
stores; VU compute; and BREAK. MTC0 preserves the resolved post-branch resume
address if an IMEM DMA swaps overlays. Unknown words trap with their instruction
address, and a nested control transfer in a delay slot panics during both
interpretation and code generation instead of becoming a no-op.

Before the delay-slot correctness fix, the round-trip interpreter handled only
ALU-immediate, ALU-register, LUI, scalar load, and MFC0. It silently omitted
scalar stores, fixed and variable shifts, MTC0, all four CP2 scalar-transfer
forms, vector loads/stores, VU compute, BREAK, unknown words, and nested control
transfers. (`Nop` was also unmatched, but omitting it is its correct effect.)
`delay_slot_store_executes_and_matches_emitter` places SW in a taken BEQ's
delay slot, verifies the interpreter's DMEM write, and verifies that the emitter
inlines the matching typed `store_w` call. Against the old handler its DMEM
assertion reads zero and fails.

## Disagreements resolved

1. **fn64 bug — inverse-square-root ROM/index parity.** The generated RSQ ROM
   placed the two mantissa octaves in the wrong order and the instruction used
   bit 23 rather than bit 22 for its 9-bit index. This disagreed with Guide
   pp. 310–314 and CEN64's divider/ROM bytes. The generator now interleaves the
   parity octaves correctly and the operation forms `((normalized >> 22) &
   0x1fe) | exponent_parity`.
2. **fn64 bug — unsigned multiply clamps.** `VMULU`/`VMACU` incorrectly used a
   numeric `0..=0xffff` threshold. The Guide's multiply result selection and
   CEN64's HI/MD sign test saturate a positive MD with bit 15 set to `0xffff`.
   The helper now clamps at the signed-MD boundary.
3. **fn64 bug — VMADN/VMADL low-slice clamps.** fn64 treated every negative
   `ACC[47..16]` as underflow. Guide pp. 265–270 instead return LO when HI is
   the sign extension of MD; only an HI/MD mismatch saturates. The helper and
   its focused tests now encode that invariant.
4. **fn64 bug — LTV/STV transpose addressing.** The old implementation rounded
   both operations to a register group and treated the element as a simple
   rotation. Guide pp. 54–55, LTV p. 191, and STV p. 225 specify consecutive
   registers, rotated byte elements, LTV's `effective+8` row selection, and
   STV's in-row byte wrap. Both runtime paths now follow those mappings.
5. **test/spec bugs.** Existing RSQ anchor tests encoded the old octave order;
   transpose tests encoded the old grouped-register mapping; and VMADN tests
   incorrectly expected every sign-extended negative accumulator to clamp to
   zero. Those expectations were corrected to the cited behavior.

`VMULQ`, `VMACQ`, `VCH`, `VCL`, and `VCR` produced no differential
disagreement after the oracle exercised their branch and flag boundaries.

## Claim boundary

The VU is differentially verified against the pinned independent semantic
references for the deterministic instruction-boundary matrix above. This is
not an exhaustive proof over every possible 128-bit input, nor a physical-RSP
capture or cycle/pipeline timing comparison. Scalar-core timing, DMA timing,
and RDP behavior remain outside this harness.

## Validation

- `cargo clippy -p fn64-audio --all-targets -- -D warnings`: clean.
- `cargo test -p fn64-audio`: 10 consecutive clean runs, each with 165 unit
  tests and four differential integration tests passing, with zero warnings.
