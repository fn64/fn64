# MIPS III / VR4300 ISA coverage

This is the completeness audit for `fn64-recomp-rs`. It distinguishes
decoding an encoding from faithfully executing it; a named panic is useful and
intentional, but it is not reported as a complete CPU implementation.

Primary sources:

- *MIPS IV Instruction Set*, revision 3.2, tables A-39 (MIPS III CPU encoding,
  pp. A-179–A-181) and B-25 (MIPS III FPU encoding, pp. B-73–B-75):
  <https://www.cs.cmu.edu/afs/cs/academic/class/15740-f97/public/doc/mips-isa.pdf>
- *NEC VR4300 User's Manual*, chapter 6 (exception processing, Cause/EPC/BD,
  BadVAddr, and the AdEL/AdES exception codes), sections 6.3.2.1–6.3.2.3
  (FCR0/FCR31 and rounding), 10.8/11.8 (LL/SC), and appendix D.2 (division by
  zero):
  <https://bitsavers.org/components/nec/mips/1995_NEC_VR4300_MIPS_RISC_Microprocessor_Users_Manual.pdf>
- The host RDRAM representation is the project ABI: native-endian word access,
  halfword address `^ 2`, byte address `^ 3`. The MIT N64Recomp `recomp.h`
  macros were used only as an allowed differential oracle for that ABI.

Legend: **C** = decoded, emitted, and correct for the typed user-mode runtime;
**P** = decoded and emitted, but uses a loud host exception or lacks some CPU
state/effect; **T** = decoded to the architecturally appropriate unusable-
coprocessor trap; **R** = reserved in MIPS III.

## Primary opcode, bits 31:26

| op | instruction | status | op | instruction | status |
|---:|---|:---:|---:|---|:---:|
| 00 | SPECIAL | P | 20 | LB | C |
| 01 | REGIMM | P | 21 | LH | C |
| 02 | J | C | 22 | LWL | C |
| 03 | JAL | C | 23 | LW | C |
| 04 | BEQ | C | 24 | LBU | C |
| 05 | BNE | C | 25 | LHU | C |
| 06 | BLEZ | C | 26 | LWR | C |
| 07 | BGTZ | C | 27 | LWU | C |
| 08 | ADDI | P | 28 | SB | C |
| 09 | ADDIU | C | 29 | SH | C |
| 0A | SLTI | C | 2A | SWL | C |
| 0B | SLTIU | C | 2B | SW | C |
| 0C | ANDI | C | 2C | SDL | C |
| 0D | ORI | C | 2D | SDR | C |
| 0E | XORI | C | 2E | SWR | C |
| 0F | LUI | C | 2F | CACHE | P |
| 10 | COP0 | P | 30 | LL | P |
| 11 | COP1 | P | 31 | LWC1 | C |
| 12 | COP2 | T | 32 | LWC2 | T |
| 13 | reserved | R | 33 | reserved | R |
| 14 | BEQL | C | 34 | LLD | P |
| 15 | BNEL | C | 35 | LDC1 | C |
| 16 | BLEZL | C | 36 | LDC2 | T |
| 17 | BGTZL | C | 37 | LD | C |
| 18 | DADDI | P | 38 | SC | P |
| 19 | DADDIU | C | 39 | SWC1 | C |
| 1A | LDL | C | 3A | SWC2 | T |
| 1B | LDR | C | 3B | reserved | R |
| 1C | reserved | R | 3C | SCD | P |
| 1D | reserved | R | 3D | SDC1 | C |
| 1E | reserved | R | 3E | SDC2 | T |
| 1F | reserved | R | 3F | SD | C |

The P ratings have specific meanings. In the arbitrary-PC lane, ADDI/DADDI and
trapping SPECIAL adds construct precise Cause/EPC/BD state and vector through
the installed guest handler; the historical whole-function callable lane still
panics because its ABI cannot return a control transfer. CACHE preserves host
coherence but does not model cache tags or Status.CH. LL/LLD and SC/SCD
implement one address-and-width reservation, one-shot clear, and mismatched
failure; DMA/external writes and the architecture's permitted
implementation-dependent invalidations are outside this single-context model.

The arbitrary-PC lane checks every naturally aligned integer, LL/SC, and COP1
load/store before executing it. Misaligned loads produce AdEL (ExcCode 4),
misaligned stores produce AdES (ExcCode 5), and exception entry records the
low 32-bit effective address in BadVAddr. A delay-slot fault records the branch
PC in EPC and sets Cause.BD. Byte and left/right merge instructions retain
their architecturally unaligned behavior. The whole-function lane still uses
the RDRAM accessors' loud alignment assertions because it has no exception
return ABI.

Instruction fetch uses the same architectural boundary. A misaligned initial
PC or computed branch target produces AdEL with EPC and BadVAddr equal to the
requested target and Cause.BD clear: the fault is on the next fetch, after any
branch delay instruction has retired. A fetch attempt consumes one
deterministic dispatcher unit. When a branch/delay pair exactly exhausts its
budget, execution checkpoints at the bad target and raises AdEL on the next
dispatch rather than splitting the pair or exceeding the budget.

## SPECIAL funct, bits 5:0

| funct | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 |
|---:|---|---|---|---|---|---|---|---|
| 00–07 | SLL C | R | SRL C | SRA C | SLLV C | R | SRLV C | SRAV C |
| 08–0F | JR C | JALR C | R | R | SYSCALL P | BREAK P | R | SYNC C |
| 10–17 | MFHI C | MTHI C | MFLO C | MTLO C | DSLLV C | R | DSRLV C | DSRAV C |
| 18–1F | MULT C | MULTU C | DIV C | DIVU C | DMULT C | DMULTU C | DDIV P | DDIVU P |
| 20–27 | ADD P | ADDU C | SUB P | SUBU C | AND C | OR C | XOR C | NOR C |
| 28–2F | R | R | SLT C | SLTU C | DADD P | DADDU C | DSUB P | DSUBU C |
| 30–37 | TGE P | TGEU P | TLT P | TLTU P | TEQ P | R | TNE P | R |
| 38–3F | DSLL C | R | DSRL C | DSRA C | DSLL32 C | R | DSRL32 C | DSRA32 C |

The VR4300 appendix D.2 explicitly gives word-sized DIV/DIVU zero-divisor
results; those are implemented and boundary-tested. It does not print
doubleword DDIV/DDIVU zero-divisor constants. Those cases deliberately panic
with the uncertainty instead of extrapolating a value. In the arbitrary-PC
lane, conditional traps, SYSCALL, BREAK, and signed overflow build the
architectural exception frame and enter the installed handler. The historical
whole-function lane retains its named panic boundary.

## REGIMM rt, bits 20:16

| rt | instruction | status | rt | instruction | status |
|---:|---|:---:|---:|---|:---:|
| 00 | BLTZ | C | 10 | BLTZAL | C |
| 01 | BGEZ | C | 11 | BGEZAL | C |
| 02 | BLTZL | C | 12 | BLTZALL | C |
| 03 | BGEZL | C | 13 | BGEZALL | C |
| 04–07 | reserved | R | 14–1F | reserved | R |
| 08 | TGEI | P | 09 | TGEIU | P |
| 0A | TLTI | P | 0B | TLTIU | P |
| 0C | TEQI | P | 0D | reserved | R |
| 0E | TNEI | P | 0F | reserved | R |

All likely forms place the delay instruction only in the taken arm. The link
forms write `$ra = PC+8` as specified; JALR snapshots `GPR[rs]` before the link
write and honors an encoded `rd=0`.

## COP0

MFC0, DMFC0, MTC0, DMTC0, BC0F/T/FL/TL, TLBR, TLBWI, TLBWR, TLBP, and ERET
all decode. BadVAddr (8), Count (9), Compare (11), Status (12), Cause (13), EPC
(14), and ErrorEPC (30) have typed 32-bit reads; Count, Compare, Status, EPC,
and ErrorEPC have typed writes. Cause accepts its two writable software-pending
bits while preserving hardware-pending lines. The other registers, doubleword
moves, and TLB operations remain loud host-boundary traps. In the arbitrary-PC
bank lane, ERET selects ErrorEPC/ERL or EPC/EXL, clears the LL reservation, and
returns a typed resolved transfer; the historical whole-function lane still
traps because its callable ABI cannot return a transfer. BC0 uses a typed
condition input, but CACHE does not synthesize Status.CH. Therefore COP0 is
encoding-complete but not a complete privileged CPU/MMU model.

The live block owner synchronizes Count/Compare with the executor at every
checkpoint. Count retains its once-per-two-cycle phase across split advances;
wrap-safe Compare equality latches IP7, and every MTC0 Compare write (including
a same-value write) clears both the block-local Cause bit and executor latch.
Visibility inside an indivisible emitted block remains checkpoint-granular.

## COP1 fmt and funct

| fmt | operation family | decode | runtime |
|---:|---|:---:|:---:|
| 00/01/02 | MFC1 / DMFC1 / CFC1 | yes | C |
| 04/05/06 | MTC1 / DMTC1 / CTC1 | yes | C |
| 08 | BC1F/T/FL/TL | yes | C |
| 10 | S format | all MIPS III functs | P |
| 11 | D format | all MIPS III functs | P |
| 14 | W source: CVT.S.W, CVT.D.W | yes | C |
| 15 | L source: CVT.S.L, CVT.D.L | yes | C |

For both S and D, funct 00–07 (ADD, SUB, MUL, DIV, SQRT, ABS, MOV, NEG),
08–0B (ROUND.L, TRUNC.L, CEIL.L, FLOOR.L), 0C–0F (ROUND.W, TRUNC.W,
CEIL.W, FLOOR.W), applicable CVT.S/CVT.D, CVT.W (24), CVT.L (25), and every
compare funct 30–3F decode. The compare low nibble implements all sixteen
F/UN/EQ/UEQ/OLT/ULT/OLE/ULE/SF/NGLE/SEQ/NGL/LT/NGE/LE/NGT predicates,
including unordered/signaling invalid behavior.

FCR0 reports VR4300 implementation `0x0B`; FCR31 preserves FS, condition,
Cause, Enable, Flag, and RM fields. CVT.W/L and all fixed rounding instructions
obey RM/fixed direction, return the integer-indefinite value on invalid, set
Cause and sticky Flag bits, and trap loudly when enabled. FR=0 odd singles
alias the high word of the preceding even FGR; double/64-bit access to an odd
FGR traps as a reserved encoding.

COP1 remains **partial** because host `f32`/`f64` ADD/SUB/MUL/DIV/SQRT and
float-to-float/int-to-float conversion do not honor non-nearest FCSR.RM, FS
flush-to-zero, or reproduce every VR4300 IEEE exception/NaN payload bit. This
is the principal user-mode blocker to a truthful “complete CPU” claim.

The arbitrary-PC lane classifies every COP1 move, memory, arithmetic, compare,
conversion, and branch instruction in the decoder and checks Status.CU1 before
any visible effect. A disabled COP1 produces Coprocessor Unusable (ExcCode 11)
with Cause.CE=1. COP1 branches fault before their delay instruction; a COP1
instruction in another branch's delay slot records the branch EPC and sets BD.
The CU1 check precedes COP1 memory alignment checks, while CU1-enabled code
continues into the normal FPU or AdEL/AdES path. The historical whole-function
lane still assumes its caller has established the libultra FPU context.

## COP2 and reserved space

COP2 is absent on the N64. MFC2/DMFC2/CFC2/MTC2/DMTC2/CTC2, COP2 branch/op
space, and LWC2/LDC2/SWC2/SDC2 decode to named loud unusable-coprocessor
traps. All primary, SPECIAL, and REGIMM slots marked R remain `Unknown` and
therefore become compile-time errors, never silent no-ops.

## Validation map

- `tests/isa_completeness.rs`: missing-slot decoder words; all rounding and
  compare functs; all byte offsets for LWL/LWR/SWL/SWR and LDL/LDR/SDL/SDR;
  LL/SC reservation behavior; DIV boundaries; FCSR modes/flags/predicates;
  FR=0; branch-likely nullification; JALR ordering.
- OoT `func_80B3C964`: its LWL/LWR/SWL/SWR register/offset shape is constructed
  from public I-format fields and decoder-checked; the same pair semantics are
  exhaustively byte-checked. No game ROM bytes or generated-game output are
  committed.
- OoT `truncf` at `0x800CD930`: existing emitted-Rust golden and C-semantics
  oracle validate TRUNC.W.S/CVT.S.W.
- OoT `__osGetFpcCsr`: its CFC1/FCR31 shape is decoder/emitter-checked and the
  returned writable FCSR fields are state-checked.
- Existing `dword.rs`, `fpu_oracle.rs`, `cop0.rs`, and `oracle.rs` retain the
  synthetic boundary sweeps and real-function oracle checks.
- `tests/bank_runner.rs`: compile-and-run proof for normal AdEL and delay-slot
  AdES, including no destination/store side effect, precise EPC/BD/BadVAddr,
  installed-vector dispatch, handler execution, and ERET resumption. The same
  gate covers initial and computed-target fetch AdEL, branch-pair budget
  checkpointing, handler-selected aligned resumption, COP1 unusable straight
  and branch paths, delay-slot BD/EPC, CU1-enabled execution, exception
  priority over COP1 address alignment, Cause.CE, and ERET.

Bottom line: encoding coverage is complete for the documented MIPS III CPU
table, with COP2 decoded as architecturally unusable. Execution is complete
for the ordinary integer, control-flow, aligned/unaligned memory, shift, and
HI/LO paths OoT can execute. It is not a complete VR4300 CPU model until the
remaining P items—especially full FPU environment behavior, TLB/translation,
COP0/COP2 exception behavior, and the whole-function lane's exception
boundary—are implemented or deliberately moved behind a documented host ABI
boundary.
