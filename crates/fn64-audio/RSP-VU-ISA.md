# RSP Vector Unit (VU) ISA — Behavioral Spec

> Clean-room RSP VU ISA behavioral spec from public N64/RSP hardware documentation;
> no GPL implementation code read or copied.

## Provenance & scope

This document describes the **behavior** of the Nintendo 64 RSP (Reality Signal
Processor) Vector Unit (a.k.a. CP2 / VU) instruction set, derived from public
documentation, chiefly the public SGI *Nintendo 64 RSP Programmer's Guide*,
Chapter 3 and the instruction appendix (especially pp. 240-245, 260-263,
272, 285-288, and 301-314). CEN64 and MAME were consulted only to
cross-check algorithm structure at ambiguous boundaries. Op *semantics* — what each op
computes, how the 48-bit accumulator works, the VCO/VCC/VCE flag registers,
element selection/broadcast, and the clamp/saturation rules — are hardware
facts, not copyrightable expression, and are restated here in our own words.

The **only** thing taken from MIT's `RSPRecomp` codegen
(`RSPRecomp/src/rsp_recomp.cpp`) is *which* ops the recompiler emits and the
*shape of the generated call*, so this spec matches the target it must satisfy.
No GPL librecomp implementation (`rsp_vu_impl.hpp`) was read or transcribed.

**Implementation target:** `fn64-audio`, a portable Rust reimplementation using
scalar / `[i16; 8]`-style lane arrays. The reference recompiler emits calls that
its C++ runtime implements with `__m128i` SSE intrinsics; we implement the
**same math** with portable per-lane scalar operations. Nothing here assumes
x86, SSE, or a particular lane layout beyond "8 signed-16-bit lanes per vector".

### Generated call shape (the contract we must satisfy)

The recompiler emits, per vector instruction, a call on an `rsp` object:

```
rsp.VMULF<e>(rsp.vpu.r[vd], rsp.vpu.r[vs], rsp.vpu.r[vt]);   // Vd, Vs, Vt ops
rsp.VMOV<e>(rsp.vpu.r[vd], de, rsp.vpu.r[vt]);               // Vd, De, Vt ops
rsp.VRCP<e>(rsp.vpu.r[vd], de, rsp.vpu.r[vt]);
rsp.VSAR<e>(rsp.vpu.r[vd], rsp.vpu.r[vs]);                   // Vd, Vs (Vt=None)
rsp.VRNDN<e>(rsp.vpu.r[vd], vs /*index*/, rsp.vpu.r[vt]);    // Vd, VsIndex, Vt
rsp.VMACQ(rsp.vpu.r[vd]);                                    // Vd only; e ignored
rsp.VNOP();                                                  // e ignored
```

- `e` is a compile-time constant 0..15, the **element modifier** (see §5). It is
  a template/const parameter, applied to `Vt` (and, for VRCP/VRSQ/VMOV, it also
  selects which source lane of `Vt` is read; `de` selects the destination lane).
- `VMACQ` and `VNOP` ignore `e` (recompiler calls them without the `<e>`).
- For `VSAR`, the element field encodes *which accumulator slice* to read
  (see §2, §6). For `VRNDN`/`VRNDP`, the `vs` field is repurposed as an
  immediate index (0 or 1) rather than a register (see §6).
- `De` (`de & 7`) is the destination-element index for VMOV/VRCP/VRCPH/VRCPL/
  VRSQ/VRSQL/VRSQH.

> Load/store ops (LQV, SQV, LPV, …) and the scalar MIPS core ops (ADD, LW, BNE,
> …) are handled elsewhere in the recompiler and are **out of scope** for this
> VU-math spec. This document covers only the CP2 compute ops listed in §6.

---

## 1. The vector register file

- **32 vector registers**, `V0..V31`, each **128 bits wide**.
- Each register is **8 lanes of signed 16-bit** (`i16`). Lane 0 is the
  most-significant 16 bits of the 128-bit register, lane 7 the least-significant
  (big-endian lane order — element index 0 is the "first"/high halfword).
- Recommended Rust representation: `[i16; 8]` (or `[u16; 8]` reinterpreted per
  op). Portable scalar per-lane math; no SIMD required.
- All arithmetic ops operate **lane-wise**: lane *i* of the result is computed
  from lane *i* of `vs` and the (element-selected) lane of `vt`.

Byte/halfword addressing note (relevant only if you also implement loads):
element index *e* in the shuffle table refers to **16-bit lanes**, indexed 0..7.

---

## 2. The 48-bit accumulator (ACC)

The VU has a **wide accumulator with one 48-bit signed slot per lane** (8 lanes).
It is addressed as three 16-bit slices per lane:

| Slice | Name       | Bit range within the 48-bit lane |
|-------|------------|----------------------------------|
| HI    | `acc_hi`   | bits 47..32 (signed high word)   |
| MD    | `acc_mid`  | bits 31..16 (middle word)        |
| LO    | `acc_lo`   | bits 15..0  (low word)           |

The full per-lane accumulator value is the signed 48-bit integer
`(acc_hi << 32) | (acc_mid << 16) | acc_lo`, treated as **two's-complement
signed 48-bit**. Model it in Rust as an `i64` per lane, keeping only the low 48
bits meaningful (sign-extend bit 47 when you need the signed value, e.g. for
clamping). Each of the eight lanes has an independent accumulator.

**Who writes ACC:** the multiply family (`VMUDx`, `VMACx`, `VMULx`) and the
add/sub family and compares. As a rule:

- **`VMULx` / `VMUDx`** *set* the accumulator (overwrite it) from the product.
- **`VMACx` / `VMADx`** *accumulate* (add the product into the existing ACC).
- **`VADD/VSUB/VADDC/VSUBC/VABS/...`** and the logical/compare ops write
  `acc_lo` (the result of the operation) as a side effect; higher slices are
  left as the hardware leaves them (documented per-op in §6; for the simple
  ALU/compare ops the meaningful, testable behavior is the written result lane —
  do not rely on acc_hi/acc_mid after those ops).

**Reading ACC:** only `VSAR` reads the accumulator into a vector register (§6).
The `e` field of `VSAR` selects the slice: `e=8` → HI, `e=9` → MD, `e=10` → LO.
(On real hardware other `e` values return 0 / undefined; for microcode fidelity,
implement 8/9/10, and treat other values as returning 0.)

**Signed vs unsigned 48-bit:** the accumulator is a signed 48-bit quantity. The
distinction between the multiply variants (F/U/H/M/N/L/Q) is (a) how the source
halfwords are interpreted (signed×signed, signed×unsigned, unsigned×unsigned),
(b) what shift is applied before adding into ACC, and (c) how the result lane is
extracted and clamped from ACC. See §6 for each.

---

## 3. Control registers: VCO, VCC, VCE

Three special flag registers, each holding **8 bits (one per lane)** in its
low byte(s). They are read/written by `MFC2`/`CFC2`/`CTC2` from the scalar side
(out of scope here) and consumed/produced by the compute ops.

### VCO — "carry / not-equal" (16 flag bits: 2 × 8 lanes)

- **Low 8 bits = carry / borrow** (one per lane). Written by `VADDC` (carry-out
  of the unsigned add) and `VSUBC` (borrow-out). Read by `VADD`/`VSUB` as the
  carry-in and by `VMRG`/`VCH`/`VCL` as documented.
- **High 8 bits = "not-equal" (`ne`)** (one per lane). Written by `VSUBC` (set
  when the two operands were not equal) and by the compare ops that produce a
  not-equal condition (`VNE`, and `VCH`/`VCL`/`VCR` use it internally). Read by
  `VEQ`/`VNE`.

Bit `i` of each half corresponds to lane `i`. Represent as two `u8` (or one
`u16` with low byte = carry, high byte = ne).

### VCC — "clip / compare result" (16 flag bits: 2 × 8 lanes)

- **Low 8 bits** = the primary compare/clip result per lane (the "less-than"/
  "clip low" outcome).
- **High 8 bits** = the secondary compare/clip result per lane ("greater-equal"/
  "clip high" outcome).
- Written by the compares `VLT/VEQ/VNE/VGE` (low byte) and by the clip ops
  `VCL/VCH/VCR` (both bytes). Read by `VMRG` (selects `vs` vs `vt[e]` per lane
  from the low byte) and by `VCL` (which consumes the previous VCC/VCO/VCE state
  set by a preceding `VCH`).

### VCE — "compare extension" (8 flag bits: 1 × 8 lanes)

- **8 bits, one per lane.** Written by `VCH` (records the special "sum == -1"
  extension condition needed to make the two-instruction `VCH`+`VCL` clip
  sequence exact). Read by `VCL`.
- `VCE` is only meaningful across the `VCH → VCL` pair; standalone it is set by
  `VCH` and consumed by `VCL`.

> Implementation tip: store the three registers as a small struct
> `{ vco: u16, vcc: u16, vce: u8 }`. Compares/clips read and write specific
> bits; the exact bit each op touches is in §6.

---

## 4. Clamp / saturation modes

Three distinct behaviors appear when extracting a result lane from a product or
the accumulator. Implement all three as helpers:

1. **Signed clamp (`clamp_signed`)** — clamp a wide signed value to
   `i16` range `[-32768, 32767]`. Used by the "F" and "H"/"M" result
   extractions (`VMULF`, `VMACF`, `VMADH`, `VMADM`, `VMUDH`, `VMUDM`, `VADD`,
   `VSUB`, `VABS`, ...). Values above `0x7FFF` → `0x7FFF`; below `-0x8000` →
   `0x8000`.

2. **Unsigned clamp (`clamp_unsigned`)** — clamp to unsigned 16-bit range but
   with the RSP's specific rule: if the signed accumulator value is negative,
   the result is `0x0000`; if it exceeds `0xFFFF`, the result is `0xFFFF`;
   otherwise the low 16 bits. Used by the "U" variants (`VMULU`, `VMACU`) and by
   the "M"/"L" *low-part* extractions where the hardware produces an unsigned
   fraction. The exact RSP rule: take `acc[47..16]` (the top 32 bits after
   dropping acc_lo); if that signed 32-bit value is `< 0` → `0`, if
   `> 0xFFFF` → `0xFFFF`, else the low 16 of acc_mid. (See per-op notes.)

3. **No clamp (`truncate`)** — just take the low 16 bits of the relevant
   accumulator slice, no saturation. Used by `VMUDN`, `VMUDL`, `VMADN`,
   `VMADL`, `VADDC`, `VSUBC`, and the logical ops.

> The subtle part of every multiply op is precisely *which* slice of ACC the
> result lane comes from and *which* of these three modes applies. §6 states it
> per op. **Get the clamp mode and the slice right and the op is correct.**

---

## 5. Element selection modifier `e` (broadcast / rotate)

Every `Vt`-consuming op reads `vt` through a **4-bit element field `e` (0..15)**
that produces a "shuffled" view `vt[e]`: for each destination lane `i`, it
selects some source lane `src(e, i)` of `vt`. This is the whole-vector broadcast/
rotate mechanism.

The shuffle table (destination lane `i` → source lane), by `e`:

| `e`   | Mnemonic | Behavior (source lane picked for dest lane `i`) |
|-------|----------|--------------------------------------------------|
| 0     | (none)   | identity: `src = i` (each lane uses its own lane) |
| 1     | (none)   | identity: `src = i` (same as 0 on hardware)       |
| 2     | `0q`     | quarter-broadcast: pairs `(0,0,2,2,4,4,6,6)`      |
| 3     | `1q`     | quarter-broadcast: pairs `(1,1,3,3,5,5,7,7)`      |
| 4     | `0h`     | half-broadcast: `(0,0,0,0,4,4,4,4)`               |
| 5     | `1h`     | half-broadcast: `(1,1,1,1,5,5,5,5)`               |
| 6     | `2h`     | half-broadcast: `(2,2,2,2,6,6,6,6)`               |
| 7     | `3h`     | half-broadcast: `(3,3,3,3,7,7,7,7)`               |
| 8     | `0`      | full-broadcast lane 0: all dest lanes ← lane 0    |
| 9     | `1`      | full-broadcast lane 1: all dest lanes ← lane 1    |
| 10    | `2`      | full-broadcast lane 2: all dest lanes ← lane 2    |
| 11    | `3`      | full-broadcast lane 3: all dest lanes ← lane 3    |
| 12    | `4`      | full-broadcast lane 4: all dest lanes ← lane 4    |
| 13    | `5`      | full-broadcast lane 5: all dest lanes ← lane 5    |
| 14    | `6`      | full-broadcast lane 6: all dest lanes ← lane 6    |
| 15    | `7`      | full-broadcast lane 7: all dest lanes ← lane 7    |

Precise definition (implement as a lookup, this reproduces the table above):

```
fn vt_element(vt: [i16;8], e: usize, i: usize) -> i16 {
    let src = match e {
        0 | 1 => i,                              // identity
        2..=3 => (i & !1) | (e & 1),             // quarter: keep pair, pick lo/hi bit
        4..=7 => (i & !3) | (e & 3),             // half:    keep group of 4
        8..=15 => e - 8,                         // whole:   broadcast one lane
        _ => i,
    };
    vt[src]
}
```

- `e = 0` (and `1`) means "no modifier" — the natural per-lane pairing.
- For `VRCP`/`VRSQ`/`VMOV`, `e` (masked to 0..7) instead selects the **single
  source lane of `vt`** to operate on, and `de` selects the destination lane.
  (These are scalar ops that touch one lane; see §6.)

Represent this as one function used by every `Vt`-consuming op.

---

## 6. Per-op reference

Notation:
- `vs[i]`, `vt_e[i] = vt_element(vt, e, i)` — signed 16-bit source lanes.
- `ACC[i]` — the signed 48-bit accumulator lane, slices `hi/mid/lo`.
- `vd[i]` — result lane written.
- "sign-extend to 48" means treat a 16- or 32-bit product as signed and place it
  in the 48-bit lane, sign-extended.
- Unless noted, an op writes **all 8 lanes** and does **not** touch VCO/VCC/VCE.

Group the implementation into families; each family shares the extraction/clamp
skeleton and differs only in operand signedness and shift.

### 6.1 Multiply — set accumulator (`VMULx`, `VMUDx`)

These **overwrite** ACC with the product, then extract `vd`.

**`VMULF` — signed fractional multiply, round, signed-clamp.**
- Product `p = vs[i] * vt_e[i]` (signed × signed, 32-bit).
- ACC ← `(p << 1) + 0x8000` (i.e. `2*p` rounded at bit 15). Store as 48-bit
  signed. (The `<<1` is the fixed-point ".15 × .15 → .15" fractional scale; the
  `+0x8000` is round-to-nearest.)
- `vd[i] = clamp_signed(ACC[i] >> 16)` — i.e. clamp acc bits [47..16] to i16.
- Special case: `vs=vt=0x8000` (−1.0 × −1.0) saturates to `0x7FFF`, which the
  round-and-clamp naturally produces.
- Flags: none.

**`VMULU` — unsigned variant of VMULF.**
- Same ACC as VMULF (`(p<<1)+0x8000`, signed product).
- `vd[i] = clamp_unsigned(ACC[i] >> 16)` (mode 2 in §4): negative → `0x0000`,
  overflow → `0xFFFF`.
- Flags: none.

**`VMULQ` — "multiply by Q" oddball.**
- Product `p = vs[i] * vt_e[i]` (signed).
- `ACC = (p << 16) + (p < 0 ? (31 << 16) : 0)`.
- `vd[i] = clamp_signed(ACC[i] >> 17) & 0xFFF0`.
- Flags: none. This is the exact Programmer's Guide pp. 285-286 operation;
  the negative bias occurs in accumulator bit position 16.

**`VMUDH` — signed × signed, high (integer) part.**
- Product `p = vs[i] * vt_e[i]` (signed 32-bit).
- ACC ← `p << 16` (product placed in acc_mid:acc_hi; acc_lo = 0).
- `vd[i] = clamp_signed(ACC[i] >> 16)` = clamp_signed of the signed 32-bit
  product. Equivalent to signed-clamp of `p`.
- Flags: none.

**`VMUDM` — signed(vs) × unsigned(vt), middle.**
- `p = (vs[i] as i16) * (vt_e[i] as u16)` — **vs signed, vt unsigned**.
- ACC ← sign-extend(`p`) into the 48-bit lane (product occupies bits [31..0],
  sign-extended above).
- `vd[i] = clamp_signed(ACC[i] >> 16)` (the acc_mid word, signed-clamped =
  the high half of the signed×unsigned product).
- Flags: none.

**`VMUDN` — unsigned(vs) × signed(vt), low, NO clamp.**
- `p = (vs[i] as u16) * (vt_e[i] as i16)` — **vs unsigned, vt signed**.
- ACC ← sign-extend(`p`) into 48-bit lane.
- `vd[i] = truncate(ACC[i] & 0xFFFF)` = **acc_lo, no clamp** (the low 16 bits).
- Flags: none.

**`VMUDL` — unsigned × unsigned, low fractional, NO clamp.**
- `p = (vs[i] as u16) * (vt_e[i] as u16)` (unsigned × unsigned, 32-bit).
- ACC ← `p >> 16` (only the high 16 bits of the unsigned product survive into
  acc_lo; acc_mid/acc_hi = 0). Equivalently acc_lo = `(u32 product) >> 16`.
- `vd[i] = truncate(acc_lo)` = those 16 bits, **no clamp**.
- Flags: none.

### 6.2 Multiply — accumulate (`VMACx`, `VMADx`)

Identical products/shifts to the `VMULx`/`VMUDx` counterpart, but **add into**
the existing ACC instead of overwriting, then extract `vd`.

- **`VMACF`** — like VMULF: `ACC += (p << 1)` (no `+0x8000` round on the
  accumulate step — the rounding bias is a VMULF-only feature; VMACF just adds
  `2*p`). `vd[i] = clamp_signed(ACC[i] >> 16)`. Flags: none.
- **`VMACU`** — like VMACF accumulate, `vd[i] = clamp_unsigned(ACC[i] >> 16)`.
- **`VMACQ`** — accumulate step of VMULQ; **takes no vs/vt** (recompiler calls
  `VMACQ(vd)` only). If ACC bits 47..21 are nonzero and bit 21 is clear, add
  `32 << 16` for a negative ACC or subtract `32 << 16` for a positive ACC;
  otherwise leave ACC unchanged. Then write `vd[i] =
  clamp_signed(ACC[i] >> 17) & 0xFFF0`. `e` is ignored (Programmer's Guide
  pp. 260-261).
- **`VMADH`** — like VMUDH: `ACC += (p << 16)` (signed product into mid:hi).
  `vd[i] = clamp_signed(ACC[i] >> 16)`. Flags: none.
- **`VMADM`** — like VMUDM (signed×unsigned): `ACC += sign_ext(p)`.
  `vd[i] = clamp_signed(ACC[i] >> 16)`. Flags: none.
- **`VMADN`** — like VMUDN (unsigned×signed): `ACC += sign_ext(p)`.
  `vd[i] = clamp_unsigned_low(ACC[i])` — **the low-part variant clamps here!**
  VMADN/VMUDN differ: `VMUDN` truncates acc_lo (no clamp), but `VMADN`, because
  the accumulate may overflow acc_lo into acc_mid, extracts the low 16 with the
  **unsigned-low clamp** (mode 2, §4) based on the sign of `acc[47..16]`.
  Concretely: if signed `acc[47..16] < 0` → `0x0000`, if `> 0xFFFF` → `0xFFFF`,
  else `acc_lo`. **(See §7 — subtle: MUDN truncates, MADN clamps.)**
- **`VMADL`** — like VMUDL (unsigned×unsigned, `>>16`): `ACC += (p >> 16)`.
  `vd[i] = clamp_unsigned_low(ACC[i])` (same unsigned-low clamp as VMADN).
  Flags: none.

### 6.3 Add / subtract with carry (`VADD`, `VADDC`, `VSUB`, `VSUBC`)

**`VADD` — signed add with carry-in, signed-clamp.**
- `sum = vs[i] + vt_e[i] + carry_in` where `carry_in = VCO.carry[i]` (0/1).
- `acc_lo[i] = (sum as i16 low 16, wrapping)` (the raw 16-bit sum, no clamp,
  written to acc_lo).
- `vd[i] = clamp_signed(sum)` (the *clamped* sum is the result lane).
- Flags: **clears both bytes of VCO** (carry and ne) for these lanes.
- Note the split: acc_lo gets the wrapped sum; vd gets the clamped sum.

**`VADDC` — unsigned add, produce carry, NO clamp.**
- `sum = (vs[i] as u16) + (vt_e[i] as u16)` (17-bit).
- `vd[i] = acc_lo[i] = (sum & 0xFFFF)` (truncated, no clamp).
- Flags: `VCO.carry[i] = (sum >> 16) & 1` (carry-out); `VCO.ne[i] = 0`.

**`VSUB` — signed subtract with borrow-in, signed-clamp.**
- `diff = vs[i] - vt_e[i] - carry_in` where `carry_in = VCO.carry[i]`.
- `acc_lo[i] = (diff wrapped to 16 bits)`.
- `vd[i] = clamp_signed(diff)`.
- Flags: clears both VCO bytes for these lanes.

**`VSUBC` — unsigned subtract, produce borrow + ne, NO clamp.**
- `diff = (vs[i] as u16) - (vt_e[i] as u16)` (with borrow).
- `vd[i] = acc_lo[i] = (diff & 0xFFFF)`.
- Flags: `VCO.carry[i] = borrow` (1 if `vs < vt` unsigned, else 0);
  `VCO.ne[i] = (diff != 0) ? 1 : 0` (set when operands differ). This ne bit is
  what `VCH`/`VCL`/`VEQ`/`VNE` sequences depend on.

### 6.4 VABS — signed "sign-apply"

**`VABS`** — apply the sign of `vs` to `vt_e`:
- If `vs[i] > 0` → `vd[i] = vt_e[i]`.
- If `vs[i] < 0` → `vd[i] = -vt_e[i]` **with the −0x8000 quirk**: negating
  `0x8000` overflows; hardware yields `0x7FFF` for the *result lane* but writes
  `0x8000` (the un-clamped negation, i.e. `0x8000`) into acc_lo. So
  `vd[i] = clamp_signed(-vt_e[i])` while `acc_lo[i] = (-vt_e[i]) & 0xFFFF`.
- If `vs[i] == 0` → `vd[i] = 0`, `acc_lo[i] = 0`.
- `acc_lo` receives the (unclamped, wrapped) chosen/negated value; `vd` the
  clamped one. Flags: none. **(See §7 — the 0x8000 clamp/acc split is subtle.)**

### 6.5 VZERO / logical ops

**`VZERO`** (pseudo; the recompiler comments it out as unused, but define it for
completeness): `vd[i] = vs[i] + vt_e[i]` with the result **truncated** (it's the
"add and discard clamp" pseudo), or, in the common convention, `vd = 0`. Because
the recompiler never emits it, treat it as unreachable; if forced, implement as
`acc_lo = vs + vt_e; vd = acc_lo` (no clamp). Flags: none.

Bitwise ops — pure per-lane, write acc_lo = result, no flags:
- **`VAND`**  `vd[i] = vs[i] & vt_e[i]`
- **`VNAND`** `vd[i] = ~(vs[i] & vt_e[i])`
- **`VOR`**   `vd[i] = vs[i] | vt_e[i]`
- **`VNOR`**  `vd[i] = ~(vs[i] | vt_e[i])`
- **`VXOR`**  `vd[i] = vs[i] ^ vt_e[i]`
- **`VNXOR`** `vd[i] = ~(vs[i] ^ vt_e[i])`

All six also set `acc_lo[i] = vd[i]`. No clamp, no flag changes.

**`VNOP`** — no operation. Touches nothing (no ACC, no flags, no vd). `e`
ignored.

### 6.6 Compares (`VLT`, `VEQ`, `VNE`, `VGE`) and select (`VMRG`)

These set **VCC.low** per lane and select `vs[i]` or `vt_e[i]` into `vd`/acc_lo
based on the comparison. They consume VCO (carry+ne) as tie-breakers.

For each lane, let `eq = (vs[i] == vt_e[i])`, and let `ne_flag = VCO.ne[i]`,
`carry = VCO.carry[i]`.

**`VLT` — set-if-less-than (signed).**
- Condition: `vs[i] < vt_e[i]` OR (`eq` AND `ne_flag` AND `carry`) — the equal
  case is included when the preceding VCO flags say the lanes were "equal but
  flagged" (this reproduces the RSP's exact `VLT` behavior used after `VSUBC`).
  In practice: `cond = (vs < vt) || (eq && ne_flag && carry)`.
- `VCC.low[i] = cond`. `VCC.high[i] = 0`.
- `vd[i] = acc_lo[i] = cond ? vs[i] : vt_e[i]`.
- Clears VCO (both bytes) for these lanes afterward.

**`VEQ` — set-if-equal.**
- `cond = eq && !ne_flag`. `VCC.low[i] = cond`, `VCC.high[i] = 0`.
- `vd[i] = acc_lo[i] = cond ? vs[i] : vt_e[i]` (on equal, picks vt_e which equals
  vs). Clears VCO.

**`VNE` — set-if-not-equal.**
- `cond = !eq || ne_flag`. `VCC.low[i] = cond`, `VCC.high[i]=0`.
- `vd[i] = acc_lo[i] = cond ? vs[i] : vt_e[i]`. Clears VCO.

**`VGE` — set-if-greater-or-equal (signed).**
- `cond = (vs > vt) || (eq && !(ne_flag && carry))`. `VCC.low[i] = cond`,
  `VCC.high[i]=0`.
- `vd[i] = acc_lo[i] = cond ? vs[i] : vt_e[i]`. Clears VCO.

**`VMRG` — merge/select by VCC.low.**
- `vd[i] = acc_lo[i] = VCC.low[i] ? vs[i] : vt_e[i]`.
- Flags: none changed. This is the "select" used after a compare.

### 6.7 Clip ops (`VCH`, `VCL`, `VCR`)

The exact clip pair used for clip-space culling. `VCH` computes the full compare
state; `VCL` completes it; `VCR` is a one-shot signed-range clip. These are the
**hardest** to get bit-exact.

**`VCH` — clip, high half. Sets VCO, VCC, VCE all at once.**
Per lane, let `s = vs[i]`, `t = vt_e[i]`, `sign = (s ^ t) < 0` (operands have
opposite signs):
- If `sign` (opposite signs):
  - `VCO.carry[i] = 1`.
  - `vce = (s == -t - 1)` → `VCE[i] = vce`.
  - `ne = !(s == -t || s == -t - 1)` → `VCO.ne[i] = ne`.
  - `VCC.low[i]  = (s + t <= 0)` ("clip to −t").
  - `VCC.high[i] = (t < 0)`.
  - `vd[i] = acc_lo[i] = VCC.low[i] ? -t : s`.
- Else (same signs):
  - `VCO.carry[i] = 0`, `VCE[i] = 0`.
  - `ne = !(s == t)` → `VCO.ne[i] = ne` (well: `ne = (s - t != 0)`).
  - `VCC.high[i] = (s - t >= 0)` ("clip to t").
  - `VCC.low[i]  = (t < 0)`.
  - `vd[i] = acc_lo[i] = VCC.high[i] ? t : s`.
- **(See §7 — the flagship subtle op: three flag registers, sign-dependent
  branches, the `-t-1` extension. Test each branch separately.)**

**`VCL` — clip, low half. Consumes VCO/VCC/VCE from a prior VCH; refines VCC.**
Per lane, using the flags left by `VCH` (`carry=VCO.carry[i]`, `ne=VCO.ne[i]`,
`vce=VCE[i]`, and the two VCC bits):
- If `carry` (was opposite-sign in VCH):
  - If `!ne`:
    - Let `sum = (u16)s + (u16)t`, `zero = ((sum & 0xFFFF) == 0)`, and
      `no_carry = (sum <= 0xFFFF)`.
    - If `vce`: `VCC.low[i] = zero || no_carry`.
    - Else: `VCC.low[i] = zero && no_carry`.
    This is the boolean form of the Programmer's Guide pp. 244-245 carry/VCE
    rule and distinguishes sums `0x10000` and `0x10001`.
  - Else (`ne` set): keep VCC.low unchanged.
  - `vd[i] = acc_lo[i] = VCC.low[i] ? -t : s`.
- Else (`carry` clear, same-sign path):
  - If `!ne`: `VCC.high[i] = ((u16)s >= (u16)t)`.
  - Else: keep VCC.high unchanged.
  - `vd[i] = acc_lo[i] = VCC.high[i] ? t : s`.
- After VCL, VCO and VCE are **cleared** (both VCO bytes → 0, VCE → 0).
- **(See §7 — the single subtlest op; the `vce` extension and the unsigned
  compare are exactly where bit-exactness is won or lost.)**

**`VCR` — clip range (one-shot signed, no VCE, no VCO).**
Per lane, `s = vs[i]`, `t = vt_e[i]`, `sign = (s ^ t) < 0`:
- If `sign`:
  - `VCC.low[i] = (s + t + 1 <= 0)` (i.e. `s <= -t - 1`).
  - `vd[i] = acc_lo[i] = VCC.low[i] ? (~t) /* = -t-1 */ : s`.
  - `VCC.high[i] = (t < 0)`.
- Else:
  - `VCC.high[i] = (s - t >= 0)`.
  - `vd[i] = acc_lo[i] = VCC.high[i] ? t : s`.
  - `VCC.low[i] = (t < 0)`.
- Flags: sets VCC (both bytes). **Clears VCO and VCE.** Note the `~t` (`-t-1`,
  ones-complement) selection — distinct from VCH's `-t`.

### 6.8 Explicit VCC/VCO manipulation helpers (`VCCL`,`VCCH`,`VCOL`,`VCOH`,`VCE`)

> These four mnemonics `VCCL/VCCH/VCOL/VCOH` and a standalone `VCE` appear in
> the task's op list. They are **not distinct RSP opcodes** in the canonical
> ISA — the RSP has single `VCC`/`VCO`/`VCE` registers accessed via `CFC2`/
> `CTC2` (control-register moves) and produced by the compare/clip ops above.
> The `-L`/`-H` suffixes denote the **low byte** and **high byte** of VCC/VCO
> respectively (§3). If the recompiler/microcode names them, treat:
> - **`VCOL`** = VCO low byte (carry bits); **`VCOH`** = VCO high byte (ne bits).
> - **`VCCL`** = VCC low byte (primary compare/clip); **`VCCH`** = VCC high byte
>   (secondary compare/clip).
> - **`VCE`** = the 8-bit VCE register (compare-extension).
> Implement them only as **named accessors** onto the `{vco, vcc, vce}` struct
> (read/write the corresponding byte). They carry no arithmetic of their own.
> Do **not** invent per-lane math for these; they are register-slice names.

### 6.9 VSAR — read accumulator slice into a vector reg

**`VSAR`** — `vd[i] = <selected accumulator slice of lane i>`; `e` selects:
- `e = 8`  → `vd[i] = acc_hi[i]`  (bits 47..32)
- `e = 9`  → `vd[i] = acc_mid[i]` (bits 31..16)
- `e = 10` → `vd[i] = acc_lo[i]`  (bits 15..0)
- other `e` → `vd[i] = 0` (hardware returns 0/garbage; use 0 for determinism).

VSAR does **not** modify ACC or any flag; it only reads. (Historically VSAR was
intended to write the ACC too, but that path is a no-op on retail RSP; do not
implement an ACC write.) `vs` is ignored by the extraction (recompiler still
passes it; you may ignore it).

### 6.10 VMOV — move one lane

**`VMOV`** — `vd[de] = vt[e & 7]` for the single selected lane.
- Source lane = `e & 7`, independent of `de` (Programmer's Guide p. 272).
- Destination lane = `de & 7`.
- `vd[de] = vt[src]`; also `acc_lo` is loaded from `vt_e` across all lanes as a
  side effect (hardware loads the whole broadcasted `vt_e` into acc_lo, but only
  the `de` lane of `vd` is written). For fidelity: `for i in 0..8 { acc_lo[i] =
  vt_element(vt,e,i) }; vd[de] = vt[e & 7]`.
- Flags: none.

### 6.11 VRND — accumulator rounding (`VRNDN`, `VRNDP`)

**`VRNDN` / `VRNDP`** — add a shifted `vt` into ACC conditionally by sign; used
for the rounding step of DMEM-based conversions. The `vs` field is repurposed as
a 1-bit index (`vs & 1`) selecting the shift amount:
- Compute `prod = (i32)vt_e[i]`; if `(vs_index & 1) != 0`, `prod <<= 16`.
- **`VRNDP`** (round positive): if the current signed `ACC[i] >= 0`, add `prod`
  into ACC (48-bit). Then `vd[i] = clamp_signed(ACC[i] >> 16)`.
- **`VRNDN`** (round negative): if the current signed `ACC[i] < 0`, add `prod`
  into ACC. Then `vd[i] = clamp_signed(ACC[i] >> 16)`.
- Flags: none. (The task lists `VRND`; the real ops are the pair VRNDN/VRNDP.)

### 6.12 Reciprocal & inverse-sqrt (`VRCP`, `VRCPL`, `VRCPH`, `VRSQ`, `VRSQL`,
`VRSQH`)

These are the scalar table-lookup ops. They operate on **one source lane** of
`vt` (selected by `e`, masked 0..7) and write **one destination lane** of `vd`
(`de & 7`); they also load the element-selected source vector into acc_lo like
VMOV. The
32-bit input is assembled across a *pair* of instructions (`…H` supplies the
high 16 bits, then `…`/`…L` supplies the low 16 and produces the result), via a
small internal 16-bit latch `div_in` / `div_out`.

The tables are **hardware constants** — a 512-entry ROM for reciprocal and a
512-entry ROM for inverse-square-root. They are *derivable* (they encode the
first Newton-Raphson refinement seed for `1/x` and `1/sqrt(x)`), hence not
copyrightable data; regenerate them from the algorithm below rather than copying
any file.

**Common front-end (compute the 32-bit input and its normalized form):**
1. Read the 16-bit source `in16 = vt[e & 7]`.
2. For the `…L`/plain single-op form, the 32-bit operand is
   `input = (div_in_hi << 16) | (u16)in16` if a preceding `…H` set the latch,
   else `input = sign_extend16(in16)`.
3. Track the original sign. For negative input in the 16-bit range, use
   `data = -input`; for double-precision input below `-32768`, use one's
   complement `data = ~input`. The exact `input == -32768` result is
   `0xFFFF0000`; zero produces `0x7FFFFFFF` (Programmer's Guide pp. 301-305,
   310-314).
4. Count leading zeros / find the shift so the value is normalized: let
   `shift = clz(data)` (number of leading zero bits of the 32-bit magnitude).
   `index = ((data << shift) >> 22) & 0x1FF` for reciprocal
   (a 9-bit index: the top bits after normalization). For rsq the index uses
   `((data << shift) >> 22) & 0x1FE | (shift & 1)` (rsq folds the LSB of shift
   into the index because 1/sqrt depends on even/odd exponent).

**Reciprocal `VRCP` / `VRCPL`:**
5. `frac = RCP_ROM[index]` (16-bit table entry).
6. Result mantissa: `result = (0x10000 | frac) << 14`, then shift right by
   `(31 - shift)` to place it. `result = ((0x10000 | frac) << 14) >> (31 -
   shift)`.
7. Reapply sign: if input was negative, `result = ~result` (ones-complement).
8. Store `div_out = result >> 16` (high 16 for a subsequent `VRCPH` read),
   `vd[de] = (i16)(result & 0xFFFF)`.
- **`VRCPL`** = the "low" op: consumes the latched high 16 bits (`div_in`) set by
  a prior `VRCPH`, produces the low 16 of the result, latches the high 16 into
  `div_out`.
- **`VRCP`** = single-precision form: `div_in` treated as 0, i.e. operate on the
  sign-extended 16-bit input only.

**`VRCPH`** — high half: does **not** run the table. It (a) latches the high 16
bits of the *input* (`div_in = in16`) for the next `VRCPL`, and (b) writes the
previously-computed high result: `vd[de] = (i16)(div_out)`. So the idiom is
`VRCPH` (load hi input, emit hi result), `VRCPL` (load lo input, run table, emit
lo result). Sets acc_lo to broadcast `vt_e` like VMOV.

**Inverse sqrt `VRSQ` / `VRSQL` / `VRSQH`:** identical structure, but:
- Table is `RSQ_ROM[index]` (the 512-entry inverse-sqrt seed ROM).
- The index computation folds the parity of `shift` (odd vs even magnitude
  exponent) as noted above, because `1/sqrt(2^k * m)` splits by `k` parity.
- The final placement shift differs by one because of the sqrt's halved
  exponent: `result = ((0x10000 | frac) << 14) >> ((31 - shift) >> 1)`
  (integer-halved shift).
- `VRSQH`/`VRSQL` play the same high/low latch roles as `VRCPH`/`VRCPL`.

**Table generation (do this, don't copy a table):** For entry `i` in 0..512,
compute the reciprocal seed as the value that Newton-Raphson would use for
`1/x` at the normalized mantissa `m = 1 + i/512` (reciprocal) or `1/sqrt(m)`
(inv-sqrt), quantized to the RSP's 16-bit fraction. The canonical published
formula:
```
// reciprocal ROM
for i in 0..512 {
    let m = (1u64 << 10) + i as u64;                 // 10-bit mantissa 1.x
    let val = ((1u64 << 34) / m) ;                   // reciprocal in fixed point
    // round to 16-bit fraction, subtract the implicit leading 1
    RCP_ROM[i] = (((val + 1) >> 8) as u16) & 0xFFFF; // (bit-exact rounding per HW)
}
```
> The exact rounding constants of the ROM are a documented hardware detail;
> the safest path is to **derive the table once and unit-test the first/last few
> entries against a captured hardware/emulator reference**, since the low-bit
> rounding is where clean-room re-derivations most often disagree. Treat the
> two ROMs as generated constants with a golden test, not as copied data.

Newton refinement: the RSP performs exactly **one** implicit refinement step
baked into the table + the mantissa shift — there is no separate iterated
Newton loop in the instruction; the "refinement" is the ROM seed plus the
`(0x10000 | frac)` reconstruction of the implicit leading 1. So the whole op is:
normalize → table lookup → reconstruct with implicit 1 → denormalize by shift →
reapply sign.

---

## 7. The three subtlest ops (double-check these)

An implementer coding from this spec should treat these as the highest-risk
and pin them with dedicated per-branch tests before trusting them:

1. **`VCL`** (§6.7) — the clip-low completion. It consumes the VCO carry/ne,
   VCC, and the **VCE extension bit** left by a preceding `VCH`, and refines the
   result via an *unsigned* comparison of the 16-bit operands plus the `-t-1`
   extension. Every branch (carry set/clear × ne set/clear × vce set/clear) is a
   separate case, and the unsigned-vs-signed compare boundary is exactly where
   clean-room re-derivations go wrong.

2. **`VCH`** (§6.7) — the clip-high op that sets **all three** flag registers
   (VCO carry+ne, both VCC bytes, and VCE) with sign-dependent branches and the
   `s == -t-1` extension condition. Getting VCE and the ne bit right here is what
   makes the subsequent VCL exact.

3. **`VMADN` vs `VMUDN` low-part clamp** (§6.1/§6.2) — the pair that looks
   identical but differs in the result extraction: **`VMUDN` truncates** acc_lo
   with no clamp, while **`VMADN` applies the unsigned-low clamp** (result = 0 if
   `acc[47..16] < 0`, `0xFFFF` if `> 0xFFFF`, else acc_lo). The same
   truncate-vs-unsigned-clamp trap applies to `VMUDL` vs `VMADL`. Mixing these
   up produces subtly wrong low words that only surface on accumulate overflow.

Honorable mentions worth a test each: **`VMULQ`/`VMACQ`** (the low-nibble mask +
sign-biased rounding, §6.1/§6.2) and **`VABS`** (the `-0x8000` clamp/acc split,
§6.4).
