# RT64 port-card standing brief

The content every RT64-to-Rust port card needs and that no card should restate
from memory. A dispatcher links a section here instead of retyping it; an
executor reads the whole file once and treats it as part of the card.

This is the port-card-specific layer on top of [`../AGENTS.md`](../AGENTS.md),
which remains controlling — clean-room sourcing, loud traps, the 10-run
deterministic and 20-run concurrency bars, "not verified" as a respectable
status, and the **one vector type per port** rule (`AGENTS.md`'s "Behavior
rules"). Dispatch-level concerns — lane allocation, model profiles, who owns a
claim — are in [`RT64-PORT-ORCHESTRATION.md`](RT64-PORT-ORCHESTRATION.md).

Every rule below was earned by a specific wrong answer. The citations are the
point; keep them when you quote a rule.

---

## 1. Measure, never assert

This section exists because four cards were dispatched in one day carrying a
false premise, and every one of those premises entered through retyped
boilerplate. The lanes caught all four — after dispatch. A brief may state a
question; it may not state its answer.

| Quantity | Why a briefed value is wrong | What the card does instead |
|---|---|---|
| **Test baselines** | Concurrent lanes move the crate count between writing the brief and running it. | Measure the crate and workspace counts in the target worktree before touching anything; report before/after, never a quoted figure. Landed cards say "baseline measured not quoted" for exactly this reason. |
| **Lint-docs count** | Worktrees lag the branch, so the branch's count is not the worktree's. Briefed counts were wrong twice in one day, with different numbers. | `git stash && python3 scripts/lint-docs.py && git stash pop` in the *target worktree*. A pre-existing error is a baseline to preserve, not a regression to fix. `ac3a5f49` shipped with "1 error before and after, unchanged"; `35be4241` records that the briefed "clean at 981 refs" did not match its checkout at all. |
| **Whether a module is already ported** | Ports land outside `fn64-render-wgpu` too, and digest-less modules read as `not-started` in the inventory. | `grep` for the symbol and for the source path across all crates before claiming a gap. `f4850c00` found four "not-started" sources fully implemented in `crates/fn64-render/src/settings.rs`; a batch card refused 21 of 21 `src/common` files, and had they been dispatched as ports they would have built competing duplicates. |
| **Whether a claimed blocker exists** | Prose describing an *un-ported* dependency's requirements parses as the *module's* requirements. | Read `lib.rs` for the `mod` line and run the tests before describing anything as blocked. `5f70326f`: five modules totalling 4,499 lines — `rt64_frame_compatibility`, `rt64_framebuffer_geometry`, `rt64_framebuffer_storage`, `rt64_render_target_geometry`, `rt64_upload_geometry`, all declared in `crates/fn64-render-wgpu/src/lib.rs` — were described as sharing one blocker. All five were landed, compiled and tested. (Grep the `mod` line by name: that commit's own line numbers have already gone stale.) |
| **Whether a digest re-freeze is needed** | The parity ledger is frozen in two independent digests, not one. | `tools/check_rt64_port_parity.py:739` freezes `{id, authority, observable_layers, earliest_observable}` against `CONTRACT_DIGEST`; **`:740-741` separately freezes the whole `{id, states}` ledger** against `STATE_DIGEST`, refusing with "row state ledger drifted; closure or reopening requires an explicit reviewed ledger update". Any state change needs both reviewed. |

**Corollary for parity rows.** Before proposing a row, check what its RT64
observation actually is. `feature::deferred-frame-history`
(`docs/rt64-port-parity.json:71`) is `RT64_PASS` / `RUST_PENDING`, and its
expected observation is the 104-byte pre-submission projection emitted from
RT64's own FullSync/advanceWorkload Workload snapshots
(`docs/RT64-PORT-PARITY.md:100-103`). No other engine can produce it. A row
whose observation is an internal snapshot of one implementation is not a parity
row.

---

## 2. Port authority: two pins, on purpose

Declared in [`rt64-port-authority.json`](rt64-port-authority.json), reported in
[`RT64-PORT-AUTHORITY.md`](RT64-PORT-AUTHORITY.md).

| Pin | Commit | Role |
|---|---|---|
| Executable oracle | `f0728a2520d5aa735886240de3fee75cc805f6d6` | `active-gated`. Every retained native RT64 receipt is bound to it. |
| Rust-port source | `5473732a822a4423b5696e7cb18fecc425a59875` | `reviewed-not-runtime-qualified`, 9 commits ahead. **Cards read C++ from here.** |

The divergence is deliberate: the candidate carries merged TMEM/framebuffer
synchronization and viewport draw-region fixes plus a Plume lifetime redesign,
so it is the intended semantic source but invalidates every retained receipt.
`RT64-PORT-AUTHORITY.md`'s "Port-source qualification still required" lists the
eight items that must pass before the pins converge.

- **Neither pin is ever "fixed" to match the other.** An agent did that and the
  tool correctly refused —
  `tools/rt64_port_inventory.py:248` requires the checkout's `HEAD` to equal the
  declared commit and raises "`<selection> checkout is at the wrong authority
  pin`". If a checkout is at the wrong pin, move the checkout.
- **Cite the pin your file came from.** Where a file is byte-identical across
  both trees (`port_delta: unchanged`, identical `oracle.sha256`), say so and
  the citation is unambiguous either way — `crates/fn64-render/src/settings.rs`
  is the worked example, citing the oracle for `rt64_user_configuration` and the
  port source for the enhancement/emulator families.

---

## 3. Hazards, each with the card that found it

### 3.1 HLSL/C++ `min`/`max` return their FIRST argument on a false comparison

`min(a,b)` is `b < a ? b : a` and `max(a,b)` is `a < b ? b : a`. With `a = NaN`
the comparison is false, so the result is `NaN` — where Rust's NaN-*suppressing*
`f32::min(NaN, 1.0)` returns `1.0`. **Write the literal ternary in the source's
argument order**, not the Rust intrinsic.

- `crates/fn64-render-wgpu/src/rt64_rsp_process.rs:296-320` — both the `max` and
  the `min` case, plus the non-obvious follow-on: HLSL `clamp` expands to
  `min(max(x,lo),hi)` and *is* NaN-collapsing where the bare calls are not, so
  its agreement with `f32::clamp` is a coincidence of bound ordering and is
  still written out longhand.
- `crates/fn64-render-wgpu/src/rt64_preset_light.rs:641-644` — the same rule for
  `std::min`.
- Swapping `min`'s argument order is a standard mutation; see §5 for the time it
  survived.

### 3.2 Assert every mask, shift and derived constant two independent ways

A literal alone cannot catch an off-by-one in a mask. Assert the same quantity
from a literal *and* from a derivation, and reconcile them.

`08c10916` (`crates/fn64-render-wgpu/src/rt64_extra_params.rs`): the executor
hand-wrote the mask union as `0x7F7FF`, one nibble position off. The test
asserted it both as a literal and as `0x7FFFF & !0x01000`; they contradicted and
the defect was caught by construction. The test now derives it three ways.
`87c26879` and `84249c73` applied the same discipline; `2a5eb72f` cross-checked
opcode values against a third independent source.

### 3.3 Hand-derived expectations are fallible — derive every non-obvious constant twice

Three cards caught their own wrong derivations, and the tests are what caught
them (`288cdf8d`):

| Asserted | Actually |
|---|---|
| RT64's `3.14159265f` differs from `core::f32::consts::PI` | Both are `0x40490fdb`; f32 cannot separate them. |
| `1e-20` squared underflows to zero | It is `1e-40` — subnormal, not zero. |
| `0.3f32 + 0.4f32 == 0.7f32` | One ULP apart. The port was right and the expectation wrong. |

The inverse also happens: `e16b54ae` found `atan2` and a source `M_PI` literal
genuinely *are* one ULP apart (`0x40490fda` vs `0x40490fdb`), and an executor's
f64 Python model had hidden it. **Model in the target precision, not in f64.**

### 3.4 Algebraically-equal float forms are not interchangeable

`2e915940`: `rt64_tile_processor.cpp:40-43` writes the lerp as
`prev + (cur - prev) * w`; a sibling in the same directory writes
`cur - delta * (1 - w)`. Algebraically equal, hand-written in both cases, and
**not bit-identical in f32** — a standing invitation to tidy one into the other.

The self-correction is the transferable part: the first draft demonstrated the
difference with a *guessed* witness `(0.1, 0.3, 0.7)`, which rounds identically,
as most triples do. A real witness needed exhaustive search
(`prev=0.3, cur=-0.2, w=0.12`), and the claim was weakened from "these differ"
to "there exist inputs where they differ". **A spot-check on an arbitrary triple
would have supported merging the two helpers.**

### 3.5 Out-of-order declarations and bit gaps are usually genuine — pin both orderings

`2a5eb72f`: `F3D_G_MV_MATRIX_1 = 0x9e` is declared before `MATRIX_2/3/4` but is
numerically last; the 16 values form a complete step-2 run over `0x80..=0x9e`
while declaration order steps +2 twelve times, then +8, −6, +2, +2. Both orders
are asserted separately so neither can be tidied into the other.
`S2DEX_G_BG_FLAG_FLIPT = 0x10` skips bits 1–3 after `FLIPS = 0x01`; also pinned.
Mutations that "smooth" an irregularity are the ones worth writing.

### 3.6 UB: do not reproduce

Deviate minimally, label the test as pinning a **DEVIATION**, and disclose it in
Nonclaims. Worked examples in `e16b54ae` and `288cdf8d`: HLSL's implicit
float-to-uint is undefined for negative, NaN or `>UINT_MAX` inputs, so the ports
use Rust's saturating `as u32` and their tests claim only Rust's behavior with an
explicit no-parity claim; an out-of-bounds `StructuredBuffer` read returns
`Option` rather than reproducing device-defined behavior. Where Rust is
deliberately *louder* than the source — an unconditional bounds check where the
C++ has genuine UB — say so in the header (`430700fb`).

### 3.7 Field declaration order is NOT pinnable in safe Rust

`ac3a5f49` disproved this by mutation on three structs, reordering two adjacent
same-typed fields while leaving the constructor untouched: `RenderIndices` 4/4
passed, `VideoInterfaceCb` 3/3, `RdpTileAddressing` 2/2 — including a test
literally named `..._declaration_order_is_pinned`. Field-init shorthand binds by
**identifier**, not position, and the readback reads fields by name too, so both
ends of the supposed pin are name-bound. Exhaustive destructuring catches
add/remove but not reorder; indexed-array literals are still name-keyed;
`clippy::inconsistent_struct_constructor` does not fire.

The correction lives at
`crates/fn64-render-wgpu/src/rt64_shared_params.rs:255-276` and applies to every
`in_source_order` / `to_source_order` pair in the crate. Those constructors stay
— they are readable transcriptions, just not pins.

**What IS pinnable:** constant values; array order, where the ordering genuinely
is positional (`rt64_render_flags` gets this right); and an index accessor
derived from `operator[]`, as in `rt64_hlsl_interop`'s `Int2/3/4` and `Quat`.
Detecting a reorder needs a macro emitting the declaration and an order witness
together, which rewrites the port's source text — out of scope for a tests-only
card.

### 3.8 No `repr(C)`, size, alignment or ABI claim

`src/shared/rt64_hlsl.h:18-19` declares an alignment mismatch across its own
HLSL/C++ boundary in upstream's own words: *"These types do not have the same
alignment in HLSLPP as HLSL. We define them and auto-convert them wherever is
possible."* Quoted and relied on at
`crates/fn64-render-wgpu/src/rt64_hlsl_interop.rs:30-40` and
`crates/fn64-render-wgpu/src/rt64_render_flags.rs:113-121`; also stated in
`AGENTS.md`'s vector-type rule. C++ bitfield allocation order within a storage
unit is implementation-defined on top of that. Settling any of it needs a real
shader compile, which no card so far has done.

---

## 4. Digest citation discipline

`tools/rt64_port_inventory.py:79` is `SHA256_LITERAL = re.compile(r"\b[0-9a-f]{64}\b")`
and `:429` collects every match in a module's text. **Any bare 64-hex literal in
your module is a port signal**, regardless of the prose around it.

- **Cite only what you ported from, or deliberately refused.** `e16b54ae`'s first
  draft quoted the digests of four *dependency* headers for provenance; those
  four would have asserted four ports the module does not make. Removed, and the
  files are now cited by path plus owning module — which also cut the drift from
  20 entries to 16.
- **A digest credits the whole file.** Port state is derived at file granularity,
  so a partial port is credited in full. **A per-file drift disclosure is
  mandatory** — full / partial-with-fraction / cited-but-not-ported — because the
  metric cannot see the difference. `85aa0799` is the extreme case: it cites
  `rt64_shader_compiler.{cpp,h}` for a refusal of all 168 lines, and without the
  disclosure that reads as 168 ported lines. `2e915940` discloses 4 of 68 lines,
  roughly 6%.
- **Verify drift past the fail-fast, not by the printed count.** The checker uses
  `require()`, which raises on the *first* failure, so one reported line proves
  nothing about the rest. Run its own `ported_as_for` logic over every entry and
  report the exact set that moved (`f4850c00`: 4 of 276, all intended, zero
  collateral).
- Compute each digest yourself with `shasum -a 256` against the pinned checkout
  **and** cross-check it against `docs/rt64-port-inventory.json`'s
  `sources.port.sha256`. Note whether `sources.oracle.sha256` matches and whether
  `port_delta` is `unchanged`.

---

## 5. Mutation testing

Mutate, run, restore, and confirm the module is byte-identical afterwards.
Report kills as `N for N` with each mutant named. Two failure modes both
appeared in one day:

1. **A mutant can survive because the test never reaches the mutated code.**
   `288cdf8d`'s M2 — swapping `hlsl_min`'s argument order inside
   `distance_from_line_segment` — survived because the NaN-order test exercised
   the helper directly and never went through the public function. Fixed with a
   `3e38` segment where both `l2` and `dot` overflow to `+inf`, making `d/l2`
   NaN-reachable at non-zero `l2`. A surviving mutant is first a question about
   your test's *reach*.
2. **A surviving mutant may be genuinely equivalent — and the proof is the
   deliverable.** `46e69b3d` reported 14 mutants, 12 killed, **both survivors
   proven equivalent**: a strict `< '9'` conjunct is dead on every reachable
   input, because any `'9'` the carry loop walks over is rewritten to `'0'`
   first. That proof also corrected three overreaching "both halves are tested"
   claims. Never report a survivor without either a kill or a proof.

---

## 6. Verification bar

Run all of it in the target worktree, and report measured numbers.

| Step | Command | Note |
|---|---|---|
| Crate + workspace tests | `cargo nextest run -p <crate> --offline`, `cargo nextest run --workspace --offline` | Baseline first. An unchanged count is the proof that a docs-only change moved no behavior. |
| **Release profile** | `RUSTFLAGS="-C debug-assertions=off" cargo nextest run -p <crate> --offline` | **Not optional.** Four `#[should_panic]` tests resting on `debug_assert!` inverted into failures under it — one in `430700fb`, three more swept in `1e8c47e7`. Overflow checks follow the same flag. |
| Determinism | 10 consecutive focused runs, identical | `AGENTS.md`'s bar. |
| Type check | `cargo check -p <crate>` | 0 errors. |
| Format | `rustfmt --edition 2021 <file>` — **per file only** | Never a crate root, never recursive: rustfmt follows `mod` children. A broad invocation on a dirty module root produced roughly 36,000 lines of noise in unrelated legacy modules (`docs/RT64-PORT-DASHBOARD.md:862`). |
| Docs | `python3 scripts/lint-docs.py` | Against the stashed baseline (§1). Every `path:line` you cite must resolve. |

**Counting test totals: strip ANSI first.**

```sh
cargo nextest run -p <crate> --offline 2>&1 | tail -1 \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -oE "[0-9]+ (passed|skipped|failed)"
```

Without the `sed`, `grep -oE "[0-9]+ passed"` returns **nothing** — nextest emits
a colour reset between the number and the word — and a script reading that count
reports 0.

**Never make the test count profile-dependent.** Both `430700fb` and `1e8c47e7`
rejected `#[cfg(not(debug_assertions))]` gating for this reason: a
profile-dependent count is a worse defect than the bug being fixed. Put the
`cfg!` *inside* a helper instead.

---

## 7. Refusal is a first-class outcome

A refusal with evidence is a delivered card. Landed examples:

| Card | Result |
|---|---|
| `85aa0799` | `rt64_shader_compiler.{cpp,h}`: **168 of 168 lines refused**, read in full, ported not at all — Windows DXC/COM glue. (~30 of `rt64_buffer_uploader.cpp`'s 175 lines ported alongside it.) |
| `bf8f527c` | Lights.hlsli: ~45 of 279 lines ported, **~234 refused** — most of a raytracing shader has no CPU meaning. |
| `4c2f803e` | RSPProcessCS.hlsl: 31 of 134 ported, **103 refused**. |
| `f4850c00` | 21 `src/common` files, **21 of 21 refused** — 17 unportable plumbing, 4 already fully owned. |
| `2e915940` | `src/render` batch, 2,303 lines: one four-line port, **28 of 29 files refused** with evidence. |

**A batch card must state in its brief that a near-total refusal is the expected
answer.** Without that sentence, executors pad. The decision table converting N
ambiguous rows into a settled question *is* the deliverable — and killing a
briefed candidate with evidence (`2e915940` killed all three of its own) counts
as delivering it.

---

## 8. Inventory regeneration is a batch operation

`tools/rt64_port_inventory.py` rewrites `docs/rt64-port-inventory.json` and
`docs/RT64-PORT-INVENTORY.md` **wholesale from a tree snapshot**, so a per-card
regeneration clobbers a concurrent lane's entries. Three lanes independently
flagged this.

- A port card **discloses** its expected drift in its own doc header and leaves
  the file alone (`crates/fn64-render-wgpu/src/rt64_framebuffer_geometry.rs:20-26`
  and `rt64_extra_params.rs:37-51` are the pattern).
- A separate `docs: regenerate inventory for …` commit lands **after every lane
  in the batch**. That commit is the only writer.
- The regeneration recipe is in `docs/RT64-PORT-INVENTORY.md:10-11` and needs
  both clean checkouts.

---

## 9. Module doc-header shape

Landed modules converge on these `//! ##` sections. Use the ones that apply, in
this order; a card that has nothing to say under a heading omits it rather than
padding it.

| Heading | Content |
|---|---|
| *(lead)* | What is ported, the pin commit, the cited paths with whole-file SHA-256 and line counts, and how each digest was verified. |
| `Cited sources and their digests` | Where the lead gets long — one row per file. |
| `Inventory drift, per file` / `Inventory drift disclosure` | §4 and §8. Full / partial-with-fraction / cited-but-not-ported. |
| `Ported / refused boundary, and the criterion` | State the criterion, then both lists. The standing criterion: *a construct is ported when its behavior is fully determined by values and control flow present in the cited file — no GPU, no ImGui context, no type from an uncited file.* |
| `Verbatim key logic` / `Verbatim key structure` | The source excerpt a reviewer reads the port against, with line numbers. |
| `Reuse, not new type` | Which existing fn64 type this uses and why; `AGENTS.md`'s vector-type rule and its three exceptions. 49 of 51 `rt64_*` modules carry this heading. |
| `Overlap with fn64's own types` | Where fn64 already owns the same fact by another route. |
| `Admitted domain` | The input range the port claims. |
| `Scope status` | **Present tense, and say DONE when done.** Prose describing an un-ported dependency's requirements reads as the module's own; write "Deliberately not ported (a scope boundary this card chose, not work this module is waiting on)" (`5f70326f`). |
| `Nonclaims` | Unwired (`mod`, not `pub mod`), no production admission, no behavior change, no `repr(C)`/size/alignment/ABI claim, every labelled DEVIATION. |
| `Open questions` | Frontiers reported rather than silently guarded. |

Worked examples of different shapes:
`crates/fn64-render-wgpu/src/rt64_extra_params.rs` (full port of one header),
`rt64_shared_params.rs` (16-header batch),
`rt64_framebuffer_geometry.rs` (partial port across three files),
`crates/fn64-render/src/settings.rs` (a port outside `fn64-render-wgpu`).

---

## 10. Card checklist

- [ ] Baselines measured in the target worktree — crate tests, workspace tests, `lint-docs` (stashed), not quoted from the brief.
- [ ] Grepped for an existing port and for a real blocker before accepting either premise.
- [ ] Read C++ from the port-source pin `5473732a`; checkout `HEAD` matches.
- [ ] Every digest recomputed with `shasum -a 256` and cross-checked against the inventory; nothing cited that is not ported or deliberately refused.
- [ ] Per-file drift disclosed; inventory not regenerated.
- [ ] Every mask/shift/derived constant asserted two independent ways and reconciled; every non-obvious constant derived twice, in the target precision.
- [ ] `min`/`max` written as the source's literal ternary in the source's argument order.
- [ ] No field-declaration-order pin claimed; no `repr(C)`/size/alignment/ABI claim.
- [ ] UB deviations labelled DEVIATION in the test and disclosed in Nonclaims.
- [ ] Mutations run and restored; every survivor either killed or proven equivalent.
- [ ] Release profile (`-C debug-assertions=off`) run; test count identical across profiles.
- [ ] 10 consecutive focused runs identical; `cargo check` clean; `rustfmt` per file.
- [ ] Every `path:line` in the header and the commit message confirmed to resolve.
