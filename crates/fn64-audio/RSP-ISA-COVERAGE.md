# RSP ISA and control-flow coverage

This is the completeness ledger for `src/rsp/`. The normative source is the
public [SGI Nintendo 64 RSP Programmer's Guide](https://ultra64.ca/files/documentation/silicon-graphics/SGI_Nintendo_64_RSP_Programmers_Guide.pdf),
revision 1.0. Encoding and generated-control-flow structure were also checked
against the MIT-licensed [RSPRecomp](https://github.com/Mr-Wiseguy/N64Recomp/tree/main/RSPRecomp).
CEN64 and MAME were used only as independent algorithm-structure checks for
divider, clip, and unusual vector-memory cases; no GPL runtime was read.

## Coverage matrix

| Surface | Manual instructions/registers | Status | Evidence |
|---|---|---|---|
| Scalar unit | ADD, ADDI, ADDIU, ADDU, AND, ANDI, BEQ, BGEZ, BGEZAL, BGTZ, BLEZ, BLTZ, BLTZAL, BNE, BREAK, J, JAL, JALR, JR, LB, LBU, LH, LHU, LW, LUI, MFC0, MTC0, NOP, NOR, OR, ORI, SB, SH, SLL, SLLV, SLT, SLTI, SLTIU, SLTU, SRA, SRAV, SRL, SRLV, SUB, SUBU, SW, XOR, XORI | Complete (48/48) | `all_48_manual_scalar_unit_mnemonics_decode`; excluded MOVZ/MOVN, multiply/divide, HI/LO, likely branches, traps, and 64-bit operations are tested as unknown. See Guide pp. 26-27 and scalar appendix. |
| Vector loads | LBV, LSV, LLV, LDV, LQV, LRV, LPV, LUV, LHV, LFV, LTV | Complete (11/11) | Exhaustive sub-op decode plus fixed-width, quad/rest, packed/half/fourth, and transpose behavioral tests. Guide Table 3-1 and pp. 48-55, 178-195. LWC2 sub-op 10 is reserved in the Guide; it is not treated as an architectural LWV. |
| Vector stores | SBV, SSV, SLV, SDV, SQV, SRV, SPV, SUV, SHV, SFV, SWV, STV | Complete (12/12) | Exhaustive sub-op decode and the paired behavioral tests above, including row wrap and transpose. Guide Table 3-1 and pp. 205-231. |
| COP2 compute | VMULF, VMULU, VRNDP, VMULQ, VMUDL, VMUDM, VMUDN, VMUDH, VMACF, VMACU, VRNDN, VMACQ, VMADL, VMADM, VMADN, VMADH, VADD, VSUB, VABS, VADDC, VSUBC, VSAR, VLT, VEQ, VNE, VGE, VCL, VCH, VCR, VMRG, VAND, VNAND, VOR, VNOR, VXOR, VNXOR, VRCP, VRCPL, VRCPH, VMOV, VRSQ, VRSQL, VRSQH, VNOP | Complete (44/44 non-reserved function codes) | `all_44_non_reserved_vu_function_codes_decode` and `every_canonical_vu_op_is_dispatched`; every function code reaches a real body. Guide Tables 3-2 and 3-5 through 3-8. The previously quoted counts of 46 or 47 included non-instructions/reserved slots; the Guide has 44 canonical compute opcodes. |
| COP2 transfers/control | MFC2, MTC2, CFC2, CTC2; VCO, VCC, VCE | Complete | Byte-element-15 wrap and all three control registers tested. Reserved control numbers trap. Guide pp. 198, 200 and VU control-register section. |
| COP0 | SP_MEM_ADDR, SP_DRAM_ADDR, SP_RD_LEN, SP_WR_LEN, SP_STATUS, SP_DMA_FULL, SP_DMA_BUSY, SP_SEMAPHORE, DP_START, DP_END, DP_CURRENT, DP_STATUS, DP_CLOCK, DP_BUSY, DP_PIPE_BUSY, DP_TMEM_BUSY | Complete at instruction boundaries | Reads/writes, semaphore read-set/write-clear, status command pairs, BREAK's HALT+BROKE, address/length alignment, DMA count/skip, and IMEM overlay exits are tested. Guide Table 4-1 and pp. 81-96. DMA and the absent RDP execute synchronously, so busy/full/counters are boundary-idle rather than cycle-accurate. |

## Four disputed vector operations

- `VMULQ`: `ACC=(signed_product<<16)+(product<0 ? 31<<16 : 0)`;
  destination is signed-clamped `ACC[32:17]` with its low nibble cleared.
  Source: Guide pp. 285-286.
- `VMACQ`: when ACC bits 47..21 are nonzero and bit 21 is clear, add
  `32<<16` for negative ACC or subtract it for positive ACC; destination uses
  the same `ACC[32:17]` signed clamp and nibble clear. Source: pp. 260-261.
- `VCH`: the implementation follows the sign-split sum/difference algorithm,
  including VCO sign/not-equal, both VCC halves, and the `sum == -1` VCE
  extension. Source: pp. 240-242.
- `VCL`: the carry path recomputes the low compare from wrapping-zero,
  unsigned carry, and VCE (`vce ? zero||no_carry : zero&&no_carry`), preserves
  VCC when VCO.not-equal is set, then clears VCO/VCE. Source: pp. 243-245.

## General control flow

- Branches execute one delay instruction on both paths and fall through to
  `PC+8`; BGEZAL/BLTZAL write the 12-bit link unconditionally.
- J/JAL/JR/JALR normalize the hardware's 12-bit PC into emitted IMEM labels.
  Computed jump-table entries work in either bare-PC or 0x1000-window form.
- An IMEM DMA returns `SwapOverlay` with the post-instruction or already
  resolved post-delay control target saved. Re-entry consumes that target; a
  two-image test replaces the image and resumes in the second overlay.
- BREAK sets SP_STATUS.HALT and SP_STATUS.BROKE and returns `Broke`.

The instruction-level model is therefore manual-complete. It is not a
cycle-accurate RSP/RDP simulator: DMA completes synchronously, the RDP command
engine is represented as immediately idle, and pipeline timing is collapsed to
architectural instruction boundaries. A hardware/reference-runtime event-trace
diff remains required before claiming independently differential-proven
bit-exactness for every unusual element/alignment encoding.
