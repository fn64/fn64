# MIPS III / VR4300 ISA coverage

This is the completeness audit for `fn64-recomp-rs`. It distinguishes
decoding an encoding from faithfully executing it; a named panic is useful and
intentional, but it is not reported as a complete CPU implementation.

Primary sources:

- *MIPS IV Instruction Set*, revision 3.2, tables A-39 (MIPS III CPU encoding,
  pp. A-179–A-181) and B-25 (MIPS III FPU encoding, pp. B-73–B-75):
  <https://www.cs.cmu.edu/afs/cs/academic/class/15740-f97/public/doc/mips-isa.pdf>
- *NEC VR4300 User's Manual*, chapter 3 (32-bit virtual-address translation,
  PageMask, EntryHi/EntryLo, and TLB exceptions), chapter 6 (exception
  processing, vectors, Context/BadVAddr, Cause/EPC/BD, and exception codes),
  sections 6.3.2.1–6.3.2.3 (FCR0/FCR31 and rounding), chapter 16 SC
  (pp. 486–488) and SCD (pp. 488–490), and appendix D.2 (division by zero):
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
implement one address-and-width reservation and one-shot clear. The manual's
SC/SCD pseudocode consults only LLbit and declares a different address from the
linked instruction undefined; mismatched failure is fn64's bounded
deterministic choice, not a physical-alias or silicon-granule parity claim.
DMA/external writes and the architecture's permitted implementation-dependent
invalidations are outside this single-context model.

Executable-write invalidation is a separate project-owned runner contract, not
a broader hardware-cache claim. The notification-only `WriteObserver` remains
unchanged. When a live owner can prove that a committed store overlaps its
active executable region, its typed `GuestWriteBoundaryObserver` returns
`ExecutableChanged`. Generated AOT and interpreted runners then stop after the
one straight instruction or after the complete indivisible branch/delay pair.
Typed executable-write exits retain the source bank and defer resume, call
classification, or exception-vector resolution until the replacement
generation is published. A non-overlapping store continues normally; a
faulting store, failed conditional store, or annulled likely delay slot does
not fabricate the boundary. This does not model cache tags, `Status.CH`, or
silicon self-modifying-code synchronization, so `CACHE` remains **P**.

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
dispatch rather than splitting the pair or exceeding the budget. Canonical
32-bit mapped fetch now keeps that architectural virtual PC separate from
`InstructionWordIdentity { BankId, physical_address }`. KSEG0/KSEG1 translate
directly; KUSEG/KSSEG/KSEG3 use the recorded PageMask/ASID/global/V state.
Physical code admission is indexed by the resulting PA, and a branch's slot VA
is translated independently before either instruction executes, including at
a page boundary whose two PFNs are nonadjacent. Generated AOT uses one bound
straight or branch/slot unit so it cannot fall through to an unvalidated word;
the interpreter builds the equivalent execution-local virtual view from the
same PA-fetched words, including the canonical `0xffff_fffc -> 0` slot wrap
without weakening ordinary non-wrapping code-span geometry. Refill and invalid
faults retain the faulting VA plus
precise EPC/BD and select the existing first-level/common exception vectors.
This is the main `BlockProgram::run`/`dispatch` path: registering a physical
generation makes its remaining entries use the mapped interpreter, while an
exact registered `MappedAotBlock` replaces one entry under the same fetch seam.
Canonical block-program evidence binds every physical span/word plus each
mapped entry's exact `BankId`/PA sequence, preflight-expected words, and
generated artifact identity; it does not hash native pointers or depend on
registration order.
Mapped-interpreter destination observations deliberately retain no generated
runner artifact. They are operational and differential evidence only, and
remain ineligible for fixed-cycle release evidence under report schema v23;
artifact-identified mapped AOT observations carry their real artifact identity
and are eligible; compatibility AOT without one is not.

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
all decode. Index (0), Random (1), EntryLo0/1 (2/3), Context (4), PageMask (5), Wired (6),
BadVAddr (8), Count (9), EntryHi (10), Compare (11), Status (12), Cause (13), EPC (14),
WatchLo/Hi (18/19), XContext (20), and ErrorEPC (30) have typed 32-bit reads; writable
registers other than BadVAddr/Random have modeled typed writes. Cause accepts
its two writable software-pending bits while preserving hardware-pending lines.
The arbitrary-PC lanes also implement DMFC0 for BadVAddr, EntryHi, and XContext
and DMTC0 for EntryHi and XContext, retaining the full 64-bit address fields.
Indexed TLB
write/read/probe and random-indexed write share one 32-entry raw management
array in both arbitrary-PC generated and interpreted lanes. The public manual supplies
Random's inclusive 31-through-Wired range and Wired-write reset to 31. Fn64's
bounded clock policy advances once per charged block-runner instruction unit,
including the existing second unit for an annulled likely slot; TLBWR samples
before its own unit advances. This is not silicon cycle-timing evidence. Wired
values outside the physical 32-entry TLB trap rather than being masked. Probe
applies each entry's PageMask plus the paired Global-bit/ASID rule and traps the
architecturally undefined multiple-match case. A failed probe sets `Index.P`
and deterministically clears the architecturally unpredictable low Index field;
this bounded choice is not a silicon-exact claim. The historical whole-function
lane keeps Random reads and TLBWR loud because its callable ABI has no
instruction clock. Canonical 32-bit KUSEG/KSSEG/KSEG3 data loads and stores in
the arbitrary-PC generated and interpreted lanes now use those same entries.
The shared translator applies each legal VR4300 PageMask, selects EntryLo0/1,
assembles PFN plus page offset, and enforces V and store-D before any memory
side effect. No match, V=0, and store with D=0 return distinct typed refill,
invalid, and modified exceptions with precise EPC/BD/BadVAddr. Exception entry
updates Context.BadVPN2 and EntryHi.VPN2 while preserving PTEBase/ASID, then
selects the 32-bit refill vector only for a first-level miss; invalid, modified,
and nested misses use the common vector. KSEG0/KSEG1 remain direct. The same
translator now applies Status.KSU plus UX/SX/KX to the documented 64-bit
XUSEG/XKSSEG/XKUSEG, XSSEG, XKPHYS, and XKSEG ranges. Mapped entries compare
EntryHi.Region plus the 40-bit VPN2; XKPHYS validates VA[58:32] before using
PA[31:0] directly. Privilege or width violations return typed AdEL/AdES before
any TLB lookup or memory side effect. Extended refill entry preserves the full
BadVAddr, updates Context, XContext.Region/BadVPN2 and EntryHi.Region/VPN2 while
retaining both PTEBase fields and ASID, and selects the first-level XTLB vector;
nested misses still use the common vector. Multiple matches and unsupported
PageMask encodings trap loudly. Full 64-bit instruction-PC admission remains
open because GuestPc, ExecutionKey, and code-catalog identity are still u32;
generated-lane translated physical device routing is also open. Other
doubleword COP0 register moves remain host-boundary traps. In the arbitrary-PC
bank lane, ERET selects ErrorEPC/ERL or EPC/EXL, clears the LL reservation, and
returns a typed resolved transfer; the historical whole-function lane still
traps because its callable ABI cannot return a transfer. BC0 uses a typed
condition input, but CACHE does not synthesize Status.CH. Privileged-
instruction admission is shared and exhaustive across the two arbitrary-PC
lanes: every decoded MFC0/DMFC0/MTC0/DMTC0, BC0*, TLB management, and ERET
shape checks kernel-or-Status.CU0 before register-shape validation, condition
sampling, Random advancement, TLB mutation, LLbit clearing, or a delay-slot
effect. Unauthorized User/Supervisor execution returns Coprocessor Unusable
(ExcCode 11) with Cause.CE=0 and precise PC/EPC/BD/accounting; kernel execution
remains authorized with CU0 clear, while User/Supervisor execution is
authorized with CU0 set. Authorized but unsupported register shapes remain
loud. Reserved Instruction behavior is not yet a complete VR4300 model, so
COP0 is encoding-complete but not a complete privileged CPU/MMU model. These
boundaries follow the VR4300 User's Manual chapters 3 and 6 already cited
above; no runtime implementation is an authority for them.

The live block owner synchronizes Count/Compare with the executor at every
checkpoint. Count retains its once-per-two-cycle phase across split advances;
the exact retained phase is imported into both arbitrary-PC lanes, so an
interior MFC0 Count includes every preceding charged instruction without
mutating or double-counting the executor-owned clock. Compare/interrupt
sampling inside an indivisible emitted unit remains checkpoint-granular.
Wrap-safe Compare equality latches IP7, and every MTC0 Compare write (including
a same-value write) clears both the block-local Cause bit and executor latch.

## COP1 fmt and funct

| fmt | operation family | decode | runtime |
|---:|---|:---:|:---:|
| 00/01 | MFC1 / DMFC1 | yes | C (AOT + interpreter) |
| 02 | CFC1 FCR0/FCR31 | yes | C (AOT + interpreter) |
| 04/05 | MTC1 / DMTC1 | yes | C (AOT + interpreter) |
| 06 | CTC1 FCR31 | yes | C (AOT + interpreter) |
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
Cause and sticky Flag bits, and trap loudly when enabled.

The physical FGR file follows VR4300 User's Manual sections 5.2 and 5.3:
FR=0 exposes the low word of each of 32 physical FGRs and joins adjacent
even/odd low words for an even-indexed doubleword FPR; FR=1 exposes 32
independent 64-bit FPRs, including odd doubleword operands. Changing Status.FR
changes only the view and preserves every latent upper word. FR=0
double/64-bit access to an odd FPR traps loudly as an invalid encoding.
`PhysicalFgrState` is the view-independent import/export currency for ABI
bridges, coroutine ownership, and deterministic state snapshots. This models
the documented register organization; it is not a silicon-parity claim.

S/D ADD, SUB, MUL, DIV, and SQRT use the host-independent soft-float shim,
honor all four FCSR rounding modes, canonicalize legacy-encoded NaNs, and
produce the modeled IEEE Cause/Flag conditions. ABS and NEG preserve raw
non-NaN bits while applying their sign operation and canonicalize NaNs. A
denormal operand or result raises the unmaskable Unimplemented Operation cause.
Enabled IEEE conditions and Unimplemented Operation return typed
FloatingPoint/ExcCode 15 before destination commit in both arbitrary-PC lanes.
MOVF/MOVT and MOVZ/MOVN copy raw S/D bits only when their condition is true.

CFC1 reads are admitted only for FCR0 and FCR31, and CTC1 writes only FCR31.
CTC1 first commits the masked FCSR fields, then returns typed
FloatingPoint/ExcCode 15 when Cause.E is set or an IEEE Cause intersects its
Enable. Reserved FCR accesses remain named loud traps. The typed ABI owns one
FCSR per emulated thread; initialization installs the libultra initial value,
while the reset thread starts from zero.

All sixteen S/D comparison predicates classify raw legacy NaNs and implement
their quiet/signaling Invalid rules. S/D-to-W/L conversions implement every
fixed and FCSR-selected rounding form, integer-indefinite Invalid results, and
typed E/V/I destination suppression. W/L-to-S/D uses an integer-only encoder
for all four rounding modes, including signed-56 Cause.E admission and enabled
Inexact suppression. CVT.D.S/CVT.S.D also use raw-bit integer conversion for
all rounding modes, legacy NaN and denormal precedence, atomic O+I/U+I, FS
flush rules, and after-rounding tininess. These paths share the same precise
straight/delay-slot exception boundary in AOT and interpreter execution and
retain the physical FR=0/FR=1 register view.

COP1 remains **partial as a silicon model**. The implemented arithmetic,
comparison, conversion, transfer, and FCSR semantics do not establish exact
instruction latency, pipeline timing, or every undocumented VR4300 NaN payload
and exception-priority quirk. Those claims require hardware differential
evidence rather than further host-language replacement.

The arbitrary-PC lanes classify every COP1 move, memory, arithmetic, compare,
conversion, and branch instruction in the decoder and check Status.CU1 before
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
  exact conversion precision, directed rounding, enabled-exception
  suppression, NaN/denormal, signed-56, FR=0/FR=1 odd-register, branch-likely
  nullification, and JALR ordering.
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
  The same compiled gate differentially executes TLB-backed load/store plus
  refill, invalid, modified, and delay-slot refill cases in the emitted and
  interpreted lanes. It also compares 64-bit XUSEG translation, XKPHYS direct
  access, user privilege address faults, and EntryHi/XContext doubleword moves.
  Unit tests separately assert full BadVAddr/EntryHi/XContext exception state
  and normal, BEV, and nested XTLB vector selection.
- `tests/bank_runner.rs::contiguous_and_sparse_runners_stop_after_executable_store`
  proves a committed overlapping straight store returns after one instruction,
  suppresses the stale sentinel in both emitter shapes, and leaves non-overlap
  in the ordinary turn.
- `tests/bank_runner.rs::emitted_and_interpreted_delay_slot_store_choose_the_same_boundary`
  proves AOT/interpreter identity for the full branch/delay pair and selected
  resume. The interpreter's conditional/fault/annul and the execution module's
  source-lineage/deferred-continuation tests cover the typed negative and
  unresolved exits; the ABI live stale-sentinel gate proves generation A is
  retired before its next instruction and B owns the resume PC.
- `tests/interp_differential.rs`: compiles generated AOT bank runners and
  compares them in-process with the dynamic interpreter. Its snapshot binds
  GPR/HI/LO, all 32 physical 64-bit FGRs, FCR31, selected COP0
  Count/Compare/Random/condition state, complete RDRAM, `BlockExit`, and
  instruction count. The FR-transition vector
  seeds latent upper words and repeatedly switches FR=0/FR=1 so either lane
  losing physical register state is observable. This differential does not
  claim the interpreter exception families which remain listed outside its
  implemented surface.

Bottom line: encoding coverage is complete for the documented MIPS III CPU
table, with COP2 decoded as architecturally unusable. Execution is complete
for the ordinary integer, control-flow, aligned/unaligned memory, shift, and
HI/LO paths OoT can execute. It is not a complete VR4300 CPU model until the
remaining P items—especially silicon-certified FPU timing and edge behavior,
64-bit instruction PC/catalog identity, translated physical device routing,
COP0/COP2 exception behavior, and the whole-function lane's exception
boundary—are implemented or deliberately moved behind a documented host ABI
boundary.
