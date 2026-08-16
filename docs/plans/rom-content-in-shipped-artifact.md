# ROM content in a shipped fn64 build: an audit, and what it takes to remove it

Design study, 2026-08-08. Read-only; nothing was built, measured, or changed.

**Question:** the owner wants to *"release builds that don't have copyrighted
rom assets baked in but are built with enough information to recompile and load
required rom assets at runtime."*

This is a **content audit**, not an architecture proposal. Static
recompilation is not in question — the owner accepts a per-title build derived
from his ROM. The binding constraint is that the *shipped artifact* carry no
copyrighted ROM content while still running once the user supplies their own
ROM at launch.

**Scope boundary, stated once:** this traces which bytes reach which artifact.
It does not assess what is or is not lawful, and nothing here should be read as
legal advice. "Verbatim" and "derived" below are descriptions of the data flow,
not legal conclusions.

---

## Verdict

**Two channels embed verbatim ROM content, both are the same bytes twice, and
both exist to verify — not to execute.** Nothing that *runs* depends on them.

| channel | what it is | bytes | status |
|---|---|---|---|
| `EXPECTED_WORDS` | ROM instruction words, in the runner | ~1.94 MiB | **already gated off** |
| `WORDS` / `code_bank()` | the same ROM words, in metadata | ~1.94 MiB | **needs a digest substitution** |
| translated instruction bodies | derived code | — | structural, stays |
| ROM *data* (assets, tables, text) | **none embedded** | 0 | already runtime |
| boot context, exec-image captures | separate files, not linked | — | ship-or-not is a choice |

**The residual after both are removed is transformed code only** — the emitted
`match pc` arms. I could not find any embedded ROM *data* section: no asset
table, no text, no IPL3/CIC region. The guest's data all flows from
`fn64_abi::load_rom` at runtime. That is the finding that makes this tractable.

**The one real piece of work** is that the runtime ROM-vs-build check currently
compares the user's ROM **word-for-word against the embedded copy**
(`receipts.rs:1209-1213`). Remove the copy and that check needs a digest
instead. That is a contained change to one validation path, not a redesign.

---

## 1. Every channel by which ROM bytes reach the artifact

### 1a. `EXPECTED_WORDS` — verbatim, in the executed source, ALREADY SOLVED

`crates/fn64-recomp-rs-codegen/src/emit/mod.rs:594-598` writes each shard's ROM
words as a literal array into the runner:

```rust
let _ = write!(out, "    const EXPECTED_WORDS: &[u32] = &[");
for word in words {
    let _ = write!(out, "{word:#010X},");
}
```

It exists for the `verify_live_words` detector — re-reading each instruction
from guest memory before executing it, to catch self-modifying writes
(`emit/mod.rs:567-574`, `:601-613`).

**This is already gated.** `emit_live_word_verification()`
(`recomps/wm2000/packages/wm2000-block-shards/build.rs:161-170`) reads
`FN64_WM_SHARD_VERIFY_LIVE_WORDS`; when off, the emitter is passed
`verify_live_words: false` (`build.rs:392`) and the `if verify_live_words`
guard at `emit/mod.rs:593` skips the array entirely. The whole detector
disappears with it.

Two things to carry, both already documented in the flag's own doc comment
(`build.rs:110-160`), which is unusually careful and should be read before
acting:

- It is a **detector**, not redundant work. Turning it off is a
  defence-in-depth removal. Two other mechanisms still cover executable-write
  detection — declared writes reaching `classify_live_executable_write`, and
  `activate_for_fetch_with_digest` re-digesting live memory before activating
  a generation — but `write_barrier.rs:52-57` lists writers that bypass the
  declaration channel, and this detector was the belt-and-braces for exactly
  those.
- It is also the **single largest per-instruction cost in the emitted body**,
  at a measured 3.10 ns/instruction against a 10.6 ns total. So removing it is
  a content win and a performance win simultaneously — which is worth flagging
  because it makes the trade unusually favourable, and because a change that
  helps two metrics at once deserves more scrutiny of the third (correctness),
  not less.

**Status: nearly solved.** The gate is uncommitted (it is in the working tree's
modified `build.rs`), but the mechanism is complete.

### 1b. `WORDS` in `metadata.rs` — verbatim, and NOT yet gated

This is the channel that is not solved, and it is the same bytes again.

`recomps/wm2000/packages/wm2000-block-shards/build.rs:449-453` emits, per shard:

```rust
let _ = write!(metadata, "pub static WORDS: &[u32] = &[");
for word in words {
    let _ = write!(metadata, "{word:#010X}, ");
}
```

`recomps/wm2000/packages/wm2000-block-shards/lib.rs:10` `include!`s that into every shard
crate, and `lib.rs:12-15` exposes it:

```rust
pub fn code_bank() -> CodeBank {
    CodeBank::new(BankId::new(BANK_ID), GuestPc::new(VA_START), WORDS.to_vec())
}
```

**It is linked and called in the shipped binary.**
`recomps/wm2000/packages/wm2000-block-boot/src/dense_aot.rs:6-...` registers all 32 shards'
`code_bank` function pointers, and `block_program.rs:96` and `:154` invoke them
during program construction. So this is not dead weight the linker strips.

**Volume.** Upper bound for WM2000: the 1 MiB boot copy plus 15 overlay shards
at 64 KiB = **~1.94 MiB of verbatim ROM words**, and with `EXPECTED_WORDS` on,
approximately double that. Derived from `BOOT_BYTES = 0x10_0000`
(`build.rs:25`), `SHARD_BYTES = 64 * 1024` (`build.rs:26`), and the WM2000
overlay layout `[2,1,5,7]` recorded at
`docs/plans/per-title-shard-generation.md:102`. These are extents, not measured
binary sections — see falsification.

### 1c. What `code_bank()` is actually FOR — and why this is replaceable

This is the pivot of the whole study, so it is worth being exact.

The words are **never executed**. Execution is the emitted `match pc` arms in
`runner.rs`. `CodeBank` is an identity-and-admission structure. Its consumers:

- **Digest comparison against the pack:** `block_program.rs:98`
  `assert_eq!(code_bank_sha256(&code_bank), expected.code_sha256)` and again at
  `:156`. Here the words are hashed and thrown away — a digest would serve
  identically.
- **Extent checks:** `block_program.rs:99-102` compares `vram_start`/`vram_end`,
  which are `VA_START` and `BYTE_LEN` — geometry, not content.
- **Runtime admission:** `CodeSpan::resolve` (`execution/program.rs:72-75`)
  looks up a word by PC, and `fallback.rs:23-32` uses admission to guarantee the
  interpreter never runs data as code. **This matters only under
  `dev-interpreter`, which the shipping build excludes** (see footnote).
- **The ROM-vs-build check:** `receipts.rs:1209-1213`, below. This is the one
  consumer that genuinely reads the word values in production.

### 1d. Translated instruction bodies — derived, and they stay

The emitted arms at `emit/mod.rs:616-620` are one Rust block per guest
instruction, keyed by VA, with the decoded operation as Rust statements. A
`lui $t0, 0x8012` becomes Rust carrying the immediate `0x8012`.

**Description of the data flow, not a legal characterisation:** these are the
output of a decode-and-translate pass, structurally analogous to a compiler's
output — the operand survives because the program's meaning depends on it, and
the surrounding form is generated. This is the same shape as every published
static recompiler. It is also **structurally inseparable**: an operand cannot
be loaded from the ROM at runtime without making the code a data-driven
interpreter, which is the thing static recompilation is not.

Note the emitter also writes a disassembly comment per instruction
(`emit/mod.rs:618`: `// {vram:#010X}: {instr:?}`). That is source-level only
and does not reach a compiled binary.

### 1e. ROM data sections — I found NONE embedded

I looked for baked ROM *data* — the category that would be a genuine problem,
because data is copied rather than transformed — and did not find any.

- **The 1 MiB boot copy is a runtime DMA.** `pack::ROM_COPY`
  (`build.rs:887`) is a geometry triple `(rom_start, rom_end, va_start)`, and
  `main.rs:904-916` asserts it against the IPL3 contract rather than carrying
  content. The bytes arrive via `publish_ipl3_cartridge_dma`
  (`shell.rs:681-683` → `receipts.rs:930-937`), which slices the
  **runtime-loaded** ROM (`receipts.rs:952-973`).
- **All other guest data** comes from `fn64_abi::load_rom(rom.clone())`
  (`shell.rs:614`, `pi/mmio.rs:27`) and is served through the PI/DMA path as
  the guest requests it.
- **No IPL3/CIC region is embedded.** The boot copy starts at ROM `0x1000`
  (`build.rs:24`), i.e. *after* the header and IPL3.

The generated `pack.rs` is otherwise addresses, digests, and symbol VAs
(`build.rs:814-921`) — `ENTRYPOINT`, `OS_*` host-binding addresses, source
digests. Addresses and hashes, not content.

**Caveat, stated because absence-of-evidence is the weakest claim here:** this
rests on reading the two generators (`wm2000-block-shards/build.rs`,
`wm2000-block-boot/build.rs`) and finding only three word-emitting loops
(`build.rs:449`, `emit/mod.rs:594`, `wm2000-block-boot/build.rs:1128`). See
falsification for the check that would settle it against the actual binary.

### 1f. External executable images — verbatim, small, and capture-derived

`recomps/wm2000/packages/wm2000-block-boot/build.rs:1124-1133` emits
`EXTERNAL_IMAGE_NN_WORDS` arrays, linked via `EXTERNAL_EXECUTABLE_IMAGES`
(`:1136-1160`) and printed at startup (`shell.rs:606`).

These are CPU-written exception-vector images — code the guest *writes at
runtime*, captured from ≥3 reproducible traces
(`build.rs:304-336`, `:339-368`) rather than read from a ROM offset. They are
small (exception vectors) and they are still ROM-derived content in the
artifact.

They are also the item with **no clean runtime substitution**, because they are
not at a known ROM offset — they are the product of executing the guest. Options
are to capture them at first run, or to keep them as a digest plus a
runtime-reconstruction path. This is the smallest of the three verbatim
channels but the least mechanical to remove.

### 1g. Boot context and captures — separate files, a shipping choice

`FN64_BOOT_CONTEXT` (`shell.rs:592-601`) is a JSON capture of initial
COP0/GPR state, loaded at runtime from a path. It is **not linked into the
binary**, so it is a question of what you put in the download, not what is
baked. `docs/plans/per-title-shard-generation.md:209` records that it cannot be
synthesized — it is a hardware capture.

It binds a ROM hash and refuses a mismatch (`boot_context.rs:34-40`,
`RomIdentityMismatch`). Whether a boot context is itself ROM-derived content is
outside this audit's scope; it is register state produced by running the ROM.
Flagging it as a decision, not resolving it.

---

## 2. The runtime-load replacement, per channel

The user already supplies the ROM at launch (`shell.rs:589-591`), so the
delivery mechanism exists. What changes is what the build *keeps*.

### 2a. `EXPECTED_WORDS`

Set `FN64_WM_SHARD_VERIFY_LIVE_WORDS=0`. Cost: loses a defence-in-depth
detector (§1a). Gains ~3.10 ns/instruction. **No runtime load needed** — the
detector goes away rather than moving.

*Alternative if the detector is wanted:* it re-reads the instruction from guest
memory anyway; the `EXPECTED_WORDS` side could be a per-shard digest checked
once at activation instead of a per-instruction word compare. That is closer to
what `activate_for_fetch_with_digest` already does, and would keep a detector
without the array — but it is a different detector with different timing, and
sizing it is out of scope here.

### 2b. `WORDS` — the substitution, and the one real obstacle

The clean form: replace `pub static WORDS: &[u32]` with the digest that already
exists beside it. `build.rs:438-448` already emits `SOURCE_SHA256` and
`RUNNER_SOURCE_SHA256` per shard, and the pack already carries
`expected.code_sha256`.

`code_bank()` would then be constructed from words read out of the
**runtime-loaded ROM** at the shard's known `(rom_start, rom_end)` extent —
geometry the pack already has — and checked against the digest.

**The obstacle is `receipts.rs:1198-1216`.** `validate_initial_entry_image`
walks every non-reserved bank and compares the guest's RDRAM
**word-for-word against `span.words`**:

```rust
for (index, expected) in span.words.iter().copied().enumerate() {
    let actual = view.read_u32(...);
    if actual != expected {
```

Those `span.words` come from the embedded `CodeBank`. So today the shipped
binary's ROM check *is* "compare against the embedded copy." Remove the copy
and this needs to become a digest comparison over the same span — hash the
DMA'd region, compare to `code_sha256`.

**What that costs.** Word-compare becomes SHA-256 over ~1.94 MiB, once, at
startup. On the order of a few milliseconds on this hardware — well under the
existing ROM file read. It does not touch the steady-state loop, so it is
irrelevant to the 60fps bar.

**What it changes semantically, and this is the honest cost:** the current
check reports *which word* diverged and at what PC (`receipts.rs:1214-1215`),
which is a genuinely better diagnostic than "the digest did not match." A
wrong-ROM user currently gets a precise address; they would get a hash
mismatch. Mitigation is to report the failing *span*, which localises it to
64 KiB — worth doing, and worth deciding deliberately rather than discovering
after the fact.

### 2c. ROM identity verification — exists, but not where it needs to be

A hash check exists today only via the boot context (`boot_context.rs:34-40`).
There is a `NORMALIZED_ROM_SHA256` const, but it is emitted **only in prepared
mode** — `prepared_candidate_receipts()` returns `[0; 32]` when
`FN64_WM_PREPARED_SHARD_ROOT` is unset (`build.rs:102-116`), and the constant is
written from that (`build.rs:928`). So in the ordinary build it is all zeros.

**For a release build this should be unconditional:** emit the normalized ROM
SHA-256 into the pack always, and check the user's ROM against it at startup
with a clear error naming the expected title. That is the "verify it by hash"
step of the standard distribution model, and it is currently missing from the
non-prepared path. It is a small change in one build script.

Note `normalize()` (`build.rs:257`) means the check should run on the
*normalized* ROM, so byte-order variants (`.z64`/`.n64`/`.v64`) resolve to one
identity rather than three failures.

### 2d. External images

No mechanical substitution (§1f). Either capture at first run, or keep and
accept them. Sizing this properly needs their actual byte count, which I did
not measure — see falsification.

---

## 3. The residual

After 2a and 2b, what remains that is ROM-derived:

1. **Translated instruction bodies**, including immediates
   (`emit/mod.rs:616-620`). Structurally inseparable from static
   recompilation — this is the recompiler's output.
2. **Addresses and geometry** in `pack.rs` — entrypoint, host-binding VAs,
   shard extents, overlay load addresses (`build.rs:814-921`).
3. **Digests** of ROM content.
4. **External executable images** (§1f) — verbatim, small, unresolved.

**Items 1-3 are the same residual every published static recompiler carries.**
Item 4 is the one thing that does not fit that pattern, and it is the most
important output of this study: *if you want a build with no verbatim ROM words
at all, the exception-vector image captures are what stands in the way*, not the
shard words.

---

## 4. Practical release shape

**Ships:** the compiled binary (translated code, addresses, digests); the boot
context for the title; a README naming the expected ROM and its hash.

**User provides:** their own ROM.

**First run:** point at the ROM → normalize → SHA-256 → compare to the pack's
`NORMALIZED_ROM_SHA256` (§2c) → on mismatch, name the expected title and stop
→ on match, DMA the boot copy and digest-verify each shard span (§2b) → boot.

This is one binary per title. Question 3(a) from the original brief — a ROM
picker over already-built titles — remains the honest UI, and
`docs/plans/shell-frontend-gaps.md:55-59` already recommends it.

---

## What would falsify this

Each of these is a check I did **not** run, because this study is read-only and
a peer is measuring on this machine. They are ordered by how much they would
change the conclusion.

1. **The "no embedded ROM data" claim (§1e) is the weakest and the most
   load-bearing.** It rests on reading generators, not on inspecting a binary.
   **The check:** build the shipped binary, then search its rodata for the
   ROM's own byte sequences — e.g. take known 64-byte runs from ROM offsets
   outside the boot copy and grep the binary for them. Finding any embedded
   data section would change the verdict materially. *Anything short of
   searching the actual binary leaves this claim at the strength of "I read the
   code that writes the arrays and there were three of them."*
2. **The ~1.94 MiB figure (§1b) is an extent calculation, not a measurement.**
   `nm --size-sort` or a section dump on a shard rlib would give the real
   number. Rust may also deduplicate identical arrays across shards, which
   would reduce it.
3. **"`WORDS` is linked, not stripped."** I verified the call chain
   (`dense_aot.rs` → `block_program.rs:96`) but did not confirm the linker keeps
   it. A symbol check on the final binary settles it. If it were already
   stripped, §1b collapses to a source-tree concern only.
4. **The digest-substitution cost (§2b).** Asserted as "a few ms" from the data
   volume. Per `perf-method.md` rule 12, a byte count is not a timing — this
   should be measured, though at startup it is unlikely to matter.
5. **External-image volume (§1f).** Unmeasured. If they are large, item 4 of
   the residual is a bigger problem than I have implied.

---

## Footnote: the interpreter, and why it does not bear on this

Investigated under the original framing and kept only because it is one line
of relevance. There **is** a complete R4300i MIPS-III interpreter in-tree —
`crates/fn64-cpu-runtime/src/semantic/mod.rs`, 1,877 lines, sharing the AOT
decoder and honoring the same `BlockExit` contract, with a differential
equivalence test (`crates/fn64-cpu-runtime/tests/interp_differential.rs`).

It is **excluded from the shipping build by compile error**:
`crates/fn64-cpu-runtime/src/lib.rs:33-34` makes `production-aot` and
`dev-interpreter` mutually exclusive, and
`recomps/wm2000/packages/wm2000-block-boot/Cargo.toml:46` selects `production-aot`.
`dev_interpreter_artifact.rs:8` exists so a byte scan can *prove* a shipped
binary contains no interpreter.

**Why it matters here and only here:** `CodeSpan::resolve`'s admission role
(§1c) is what keeps the interpreter from running data as code
(`fallback.rs:23-32`). Since the shipping build has no interpreter, removing
`WORDS` does not weaken that guarantee **in the shipped artifact** — but it
would in a `dev-interpreter` build. So the substitution in §2b should be
conditioned on the production feature, or the admission path given the words
from the runtime ROM. Not an obstacle; a thing to not get wrong.

---

# EMPIRICAL VERIFICATION AGAINST THE BUILT BINARY

Added 2026-08-08 by a second agent. **The analysis above is not rewritten** —
this section tests it. Where a measurement contradicts the study, that is
called out explicitly as a correction.

The study named its own weakest link (falsification #1): the "no embedded ROM
data" claim "rests on reading the generators and finding only three
word-emitting loops — not on inspecting a binary." That is what this settles.

**Headline: the claim SURVIVES, with one correction to the residual and one to
the volume.** No embedded ROM *data* was found by four independent methods. But
the study understates one thing: **the gated lane still contains ~1.82 MiB of
verbatim ROM words**, because `FN64_WM_SHARD_VERIFY_LIVE_WORDS=0` removes
`EXPECTED_WORDS` only — `WORDS` is not gated and is unaffected. §2a is not a
release configuration by itself.

## What was built

`recomps/wm2000/packages/wm2000-block-boot`, `--release --features rt64`, both lanes, separate
`CARGO_TARGET_DIR`s so neither clobbers the other. Script:
`scripts/rom-content-audit-build.zsh`. Both exit 0.

| lane | `FN64_WM_SHARD_VERIFY_LIVE_WORDS` | binary | size |
|---|---|---|---:|
| `verifyon` | unset (default = on) | `target-audit-verifyon/…` | 95,303,296 B (90.89 MiB) |
| `verifyoff` | `0` | `target-audit-verifyoff/…` | 90,877,936 B (86.67 MiB) |

ROM: `wm2000.z64`, 33,554,432 B, `WRESTLEMANIA 2000`. Machine was verified free
by **CPU-time delta across all 747 processes** (rule 18 — `pcpu` reads 0.0 on
this workload): only 7 moved >0.5 s, all UI/daemon. The ~9 suspended `rustc`
(state `T`) in `/private/tmp/fn64-verify-ab` are a peer's and were left parked.

## Rule 19: the gate demonstrably took — and this check has failed before

A prior A/B of this flag nearly ran verify-on against verify-on. Counting the
arrays in the **generated source** before trusting any delta:

| lane | `runner.rs` files | files with `EXPECTED_WORDS` | total occurrences | `metadata.rs` with `WORDS` |
|---|---:|---:|---:|---:|
| `verifyon` | 33 | 32 | **1,872** | 32 |
| `verifyoff` | 33 | **0** | **0** | 32 |

1,872 → 0. The gate took. **`WORDS` is 32 in both lanes** — it is not gated,
which is the correction developed below.

## Rule 6a / 20: the search was proved able to FIND before any absence was reported

A search that finds nothing because it is malformed looks exactly like a clean
binary. Four controls, in order:

1. **Ordering transforms unit-tested.** All three byte-swaps are involutions,
   and `swap4` was proved *equal* to the little-endian `u32` storage form via an
   independent `struct.pack` round-trip. **This is load-bearing:** the
   generators emit ROM words as Rust `u32`, so on arm64 they are stored
   **byte-reversed** relative to the ROM. A search for raw z64 bytes alone
   returns a **false zero** — and indeed *every* hit below is `swap4` and **not
   one** is raw `z64`.
2. **Synthetic-needle end-to-end test.** A ROM run from offset `0x200000` was
   compiled into a real Mach-O as a `u32` array; the search found it at the
   right offset, right ordering, right section.
3. **This caught a real tooling bug.** The first `otool -l` parser mapped
   **0 sections** on every binary, because within a `Section` block `size`
   precedes `offset` and the parser assumed the reverse. Hit counts were
   unaffected, so every hit would have been reported as
   `(outside any section)` — a plausible-looking wrong answer. Fixed; the
   parser now maps 25 sections, and the script prints a loud warning if it ever
   maps zero again.
4. **The un-gated lane is the positive control for the gated one** — known
   ROM content, same binary family, same search invocation. It returns 126
   hits. `--require-control` makes a zero there a hard failure.

## Result 1 — the ROM-content search

`scripts/rom-content-audit-search.py`. 838 runs of 64 B on a 0x8000 stride
across the **whole** ROM (192 low-entropy runs skipped as coincidence-prone),
plus 14 forced named regions: header, IPL3/CIC at 0x40/0x400/0x800, boot copy,
2 MiB, 8 MiB, 16 MiB, 26 MiB. Coverage `0x0 .. 0x1a78000`. Four orderings
(z64 / swap4 / swap2 / swap2of4).

| lane | hits | distinct ROM offsets | orderings | sections | ROM span of hits |
|---|---:|---:|---|---|---|
| `verifyon` | 126 | 35 | **swap4 only** | **`__TEXT,__const` only** | `0x1000 .. 0x138000` |
| `verifyoff` | **67** | 35 | **swap4 only** | **`__TEXT,__const` only** | `0x1000 .. 0x138000` |

**The partition is clean and it is the whole finding:**

- sampled offsets **≤ 0x138000: 32 of 33 hit**
- sampled offsets **> 0x138000: 0 of 799 hit** — covering **30.8 MiB (96%)** of
  the ROM, including every asset, texture, audio and text region.

**Not a needle-length artifact.** Re-run at 48 B, 32 B and **16 B** — a much
weaker provenance claim but a far more sensitive detector — the deep region
stays at **0/798** in all three.

**Copies-per-offset halve exactly**, which is the mechanism made visible:

| | offsets with 1 copy | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| `verifyon` | 0 | 15 | 16 | 4 |
| `verifyoff` | 15 | 16 | 3 | 1 |

Every count is divided by two. That is `EXPECTED_WORDS` removed and `WORDS`
retained — not a partial or accidental difference.

## Result 2 — an independent, binary-driven coverage check

Offset-sampling can only find regions it thought to sample. So the search was
also run **in reverse**: index all 6,240,853 distinct 16-byte ROM runs, then
walk every 4-aligned window of the gated binary's `__TEXT,__const` and ask which
ROM offset it came from. This is driven by the binary's contents, so it cannot
miss a region.

- **480,384** const windows match some ROM location.
- **27 of 512** 64 KiB ROM buckets are covered at all.
- **480,360 (99.995%)** fall in the contiguous **`0x0`–`0x14ffff`** code region.
- The remaining **24** are scattered in high buckets and are **all trivial
  low-entropy patterns** — 2 distinct bytes, e.g.
  `00000000000000000000000100000001`. Coincidence, not provenance.

Two methods, opposite directions, same boundary.

## Result 3 — no ROM *data*, tested directly rather than inferred

The offset search covers data regions by construction (it samples the whole
ROM). Additionally, all **9,096** distinct printable ROM strings (≥12 chars, ≥6
distinct bytes) were searched in the gated binary. **Three matched, and all
three are non-provenant:**

- `,-./0123456789:;<` and `3456789:;<=>?@AB` — verified programmatically to be
  runs of **consecutive byte values**; they occur in any charset table.
- `Controller Pak` — **fn64's own source text**, in `fn64-abi` (`pfs.rs:1`,
  `lib.rs:741`, `si/mod.rs:303`). An N64 hardware term that coincides with the
  ROM's copy; it is not copied from it.

**No game text, no asset table, no IPL3/CIC region, no header.** The header and
IPL3 were sampled explicitly (0x0, 0x40, 0x400, 0x800) and returned zero hits in
both lanes — consistent with §1e's reasoning that the boot copy starts at
0x1000, *after* the header and IPL3.

**§1e is CONFIRMED against the binary.** It is no longer "I read the generators
and there were three loops."

## Result 4 — the volume, measured (corrects §1b and falsification #2)

The study's **~1.94 MiB is an extent calculation**. Measured:

| section | `verifyon` | `verifyoff` | delta |
|---|---:|---:|---:|
| **`__TEXT,__const`** | 8,996,480 | 7,085,632 | **1,910,848** |
| `__TEXT,__text` | 82,382,660 | 79,909,196 | 2,473,464 |
| `__TEXT,__eh_frame` | 442,660 | 438,860 | 3,800 |
| all others | — | — | ≈0 |
| **total file** | 95,303,296 | 90,877,936 | **4,425,360 (4.22 MiB)** |

**The `__TEXT,__const` delta reconciles to 0.01%.** Summing the generated
`metadata.rs` arrays gives 477,664 words = **1,910,656 B**; the measured delta is
**1,910,848 B** — 192 B of alignment padding apart. This independently proves
the delta *is* the `EXPECTED_WORDS` arrays and nothing else.

**Corrections:**

- **One copy of the ROM words is 1.822 MiB, not 1.94 MiB.** The extent
  calculation was ~6.5% high (it assumed 15 full 64 KiB overlay shards).
- **No cross-shard deduplication occurs** (falsification #2 asked). The measured
  size matches the naive sum, so Rust/the linker did not merge identical arrays.
- **Total cost of the un-gated detector is 4.22 MiB, not ~1.94 MiB**, because
  removing `EXPECTED_WORDS` also deletes the per-instruction verification
  *code*: `__TEXT,__text` drops **2.47 MiB**, which is *larger* than the array
  saving. The study only counted the array.
- **In the gated binary, ROM words are 1,910,656 B = 27.0% of
  `__TEXT,__const`** and **2.1% of the 86.67 MiB binary.** The dominant term is
  translated code (`__TEXT,__text` is 79.9 MiB), exactly as §1d describes.

## Result 5 — `WORDS` is linked, not stripped (settles falsification #3)

`nm` on the **gated** binary finds **33 `code_bank` symbols** — 32 shard
`code_bank` functions, all `T` (external, defined), plus `code_bank_sha256`.
The linker keeps them. §1b's concern is real and is not a source-tree-only
issue.

Also confirmed on the shipping binary: **the interpreter is absent** — zero
occurrences of the `dev_interpreter_artifact` marker, consistent with the
footnote.

## Result 6 — the external images are 16 bytes, and §1f's premise is WRONG

Falsification #5 asked for their volume. Measured, and **this is a correction**:

- **One** image is packed, not three. The build log says
  `1 captured exception images`, and the three `FN64_EXECUTABLE_IMAGES` paths
  are **three reproductions of the same capture** used to agree on one image,
  not three separate images.
- It is **4 words = 16 bytes**: `general-exception-preamble`, generation 0, at
  VA `0x80000180`. Present in both binaries (verified at file offset
  `0x4c9f350` gated), in the `swap4` `u32` form.
- **§1f says these are "not at a known ROM offset — they are the product of
  executing the guest." That is false for this image.** All four words appear
  **verbatim in the ROM at offset `0x37380`** (verified by direct search of the
  big-endian ROM bytes).

**Consequence:** §1f called this "the item with no clean runtime substitution"
and §3 called it "the one thing that does not fit the pattern … what stands in
the way" of a build with no verbatim ROM words. **Both overstate it.** It is
16 bytes at a known ROM offset, so it has exactly the same digest+extent
substitution available as `WORDS`. Item 4 of the residual is the *smallest*
problem, not the blocking one.

Caveat: this is one image for WM2000 on this route. A title whose captured
images are genuinely CPU-synthesized would still hit §1f's argument — the
mechanism is sound, its instantiation here is not.

## What this changes

**Confirmed:** §1e (no embedded ROM data) — the load-bearing claim, now tested
four ways instead of read. §1c (words are hashed, not executed) is consistent
with all hits landing in `__TEXT,__const`. Falsification #3 (`WORDS` is linked).

**Corrected:**

1. **§2a alone does not produce a release build.** The gated lane still holds
   ~1.82 MiB of verbatim ROM words in 32 linked `WORDS` arrays. **§2b is the
   binding work item**, and this measurement is what shows it.
2. One copy is **1.822 MiB**, not 1.94; no dedup; the detector's full cost is
   **4.22 MiB** because 2.47 MiB of it is verification *code*.
3. **§1f/§3: the exception image is 16 bytes at ROM `0x37380`** and is
   mechanically substitutable. Demote it from "the thing that stands in the
   way."

**Still not verified here:** the digest-substitution cost (falsification #4) —
not measured, and it remains a startup-only cost. Neither binary was *run*;
this is a content audit of the artifact, not a behavioural test. And this is
WM2000 on one route: the method generalizes, the specific numbers do not.

**Scope, restated:** this traces which bytes are where. It is not legal advice
and nothing here should be read as a conclusion about lawfulness.

---

# IMPLEMENTATION: the geometry substitution

Added 2026-08-08 by a third agent, which **implemented** §2b rather than
studying it. Where this contradicts either section above, the contradiction is
called out.

## What `WORDS` is load-bearing for — settled from source, and it is NOT what §1c implied

The study conditioned the substitution on the `production-aot` feature because
`CodeSpan` admission is what stops the interpreter running data as code. That
caution was right to raise and **wrong in its conclusion**: the substitution is
safe unconditionally, and the reason is stronger than a feature flag.

1. **Nothing executes from `WORDS`.** The production path is
   `run_inner` (`execution/program.rs:1264-1336`). Its one contact with the
   words is line 1317:
   ```rust
   if let Err(fault) = self.code.resolve(entry) { ... }
   ```
   `resolve` returns `Result<ResolvedInstruction, CpuFault>` where
   `ResolvedInstruction { key, word }` — and the `Ok` value is **bound to
   `if let Err`, i.e. computed and discarded**. Execution is `run(...)`, the
   generated `match pc` arms.
2. **Admission reads a length, not a value.** `CodeSpan::resolve`
   (`program.rs:72-75`) is `self.words.get((offset / 4))`, and every admission
   call site takes `.is_some()` (`:763`, `:834`, `:857`, `:1317`). A span of the
   right length filled with arbitrary words admits and rejects exactly the same
   PCs. **Admission is geometry.**
3. **`classify()` — the only function that decodes a resolved word
   (`program.rs:868-880`) — has zero non-test callers** in the workspace.

Applying the per-call-site test "does this *check* something, or *derive* state
the program depends on": **every non-test reader of `.words()` is a SHA-256
hasher** (`main.rs:66`, `shell.rs:270`, `catalog_v1.rs:250`,
`program.rs:1092/1130`). Only `evidence_snapshot` derives state — and
substitution preserves it **byte-identically, because the recovered words are
equal**. That is why this is a substitution rather than a gate.

## The mechanism

Each shard emits `ROM_START`/`ROM_END` beside the existing `VA_START`/`BYTE_LEN`
instead of a literal array. `code_bank()` recovers the words from the user's ROM
via `fn64_cpu_runtime::shard_words`.

**Why it matches by construction, not by luck:** the build-time decode at
`wm2000-block-shards/build.rs:488-491` is literally
`rom.bytes[byte_start..byte_end].chunks_exact(4).map(u32::from_be_bytes)`, and
`shard_words` performs the identical operation on the same offsets of the same
normalized image. Boot and overlay shards share that one path. So the existing
`assert_eq!(code_bank_sha256(&code_bank), expected.code_sha256)`
(`block_program.rs:98`, `:156`) cannot pass with the wrong bytes and cannot fail
with the right ones.

**Verification reuses the strongest check that already existed** rather than
adding one. No new surface.

### Proven against the committed artifact, before anything was rebuilt

The construction argument above is structural. It was also tested against real
data: take the digests `block_program.rs:98` actually asserts, out of the
existing `pack.rs`, and check whether reading the ROM at the build-recorded
geometry reproduces them.

| generation | shards | reconstruct from ROM geometry |
|---|---:|---|
| `BOOT_SHARDS` | 15 | **15/15**, contiguous from ROM `0x1000` |
| `RESIDENT_TAIL_SHARDS` | 2 | **2/2**, from the split at ROM `0xe2790` |
| `OVERLAY_*_SHARDS` | 15 | **not brute-force verified** — see below |

Boot shard 0's recorded `code_sha256` is `[7, 130, 99, 212, ...]`; hashing ROM
`0x1000..0x11000` as big-endian words gives `078263d47acc...`. Same bytes.

**The overlays are unverified by construction-analogy, and checked at build.**
`resolve_generation` sets their `source_start` to `recipe.rom_start`, the same
plain-ROM-offset mechanism confirmed on the other 17, and `code_bank_sha256`
proves it at startup and fails loudly if not. Locating their base by scanning
was too slow to be worth it; this is stated as unchecked rather than implied.

### A self-correction worth keeping, because it looked like a real defect

The first pass of that table read **14/16 boot** and **0/2 resident tail**.
Both numbers were wrong, and both were the *probe's* error rather than the
code's:

- "14/16" assumed 16 full 64 KiB boot shards. There are **15**, the last
  partial at `0x1790`; the probe only tested full-size spans.
- "0/2" assumed the resident tail followed the 1 MiB boot copy at ROM
  `0x101000`. It does not — it is the **second half of the same copy**, from
  the split.

Reading `resolve_generation` (`wm2000-block-shards/build.rs:536-600`) settled
it for all three generation kinds at once. **This is rule 23's shape:** several
narrow probes agreeing on one wrong offset assumption looks exactly like
corroborated evidence of a bug. Each probe answered the question literally
asked; none answered the question meant.

## Corrections to §2b and §2c

1. **The diagnostic does NOT regress.** §2b predicted exact-diverging-PC would
   become a hash mismatch, and proposed 64 KiB span granularity as mitigation.
   That assumed *removal*. Under substitution the words exist at runtime, so
   `receipts.rs:1209-1213` keeps its word-for-word compare and its exact PC.
   Nothing is paid and no mitigation is needed.
2. **`NORMALIZED_ROM_SHA256` is now unconditional** (`build.rs`), derived from
   `rom.sha256` and still cross-checked against prepared receipts in prepared
   mode. It is enforced at startup by `assert_normalized_rom_identity()` in
   `block_program.rs` — the one construction path both binaries share — naming
   the expected title and both digests.
3. **The interpreter footnote is retired, not satisfied.** The substitution is
   safe even in a `dev-interpreter` build: the words come from the same
   geometry with the same values, so admission extent *and* interpreted content
   are unchanged. No feature conditioning was needed.

## The exception image (confirms Result 6)

Verified independently: the 4 words at ROM `0x37380` are
`3C1A8003 275A6790 03400008 00000000`, and that 16-byte run occurs **exactly
once** in the ROM. The build now *searches* for each captured image and
**panics unless the match is unique**, rather than hardcoding the offset — so a
title whose images are genuinely CPU-synthesized fails loudly instead of
silently binding an arbitrary offset. §1f's "no clean runtime substitution" is
refuted for this image, as Result 6 found.

## The acceptance test had to change, and this is the important part

**The audit's positive control does not survive its own fix.**
`--require-control` used the verify-on binary as the control: a lane known to
embed ROM words, so a zero there meant the search was broken. After the
substitution **both lanes are clean**, so that control has nothing to find — it
would hard-fail or be quietly dropped, leaving a zero that proves nothing.
That is precisely the false-zero shape the audit's own Mach-O parser bug
produced (0 sections mapped, every hit reading `(outside any section)`).

So `scripts/rom-content-audit-search.py` gained a **binary-independent
synthetic control**: it plants a known ROM run in all four orderings and
requires the matcher to find each at its planted offset, plus a never-planted
run it must *not* find. Proved in **both** directions (rule 6a):

- healthy → `4/4 orderings planted and found -- PASS`, exit 0
- `swap4` made a no-op (the exact endianness defect that causes a false zero)
  → `SYNTHETIC CONTROL FAIL: ordering swap4 planted at 0xc0 but found at 0x40`,
  exit 2

`--require-clean BINARY` makes acceptance machine-checked; proved against a
synthetic binary carrying 256 B of real ROM words in `swap4` (correctly FAILs,
exit 1) and a clean one (PASSes, exit 0).

**The archived pre-change binaries are retained as the real-Mach-O positive
control** — a synthetic buffer proves the matcher works, but only a binary of
this same program known to carry ROM words proves it works *on this shape*.

## MEASURED RESULT

Built `recomps/wm2000/packages/wm2000-block-boot`, `--release --features rt64`, geometry
substitution plus `FN64_WM_SHARD_VERIFY_LIVE_WORDS=0`.

### The search: zero ROM words

| binary | size | hits | orderings | sections |
|---|---:|---:|---|---:|
| **geometry** | 88,988,512 | **0** | — | 25 |
| `verifyon` (archived control) | 95,303,296 | 126 | swap4 only | 25 |

Three controls, all passing, which is what makes the zero mean anything:
**synthetic 4/4 orderings** (binary-independent), **archived control 126 hits**,
**`--require-clean` PASS**. 838 sampled runs across the whole ROM, four
orderings. 25 sections mapped, so the audit's 0-section parser bug is absent.

### Size, per section

| section | `verifyoff` | geometry | delta |
|---|---:|---:|---:|
| **`__TEXT,__const`** | 7,085,632 | 5,180,416 | **−1,905,216** |
| `__TEXT,__text` | 79,909,196 | 79,923,384 | +14,188 |
| **file total** | 90,877,936 | **88,988,512** | **−1,889,424** |

**The `__TEXT,__const` drop reconciles to 0.3%** — removed arrays measured
1,910,656 B, section fell 1,905,216 B. That is the arrays and nothing else,
which is far stronger evidence than the whole-file delta.

**Attribution caveat:** the `+14,188` in `__TEXT,__text` is a *different
agent's* telemetry work that landed in the same binary. It is not the geometry
change and is not claimed as such. A clean attribution would need their tree
built without this change.

**Against the original un-gated binary: 6,314,784 B smaller (6.02 MiB).**

### The boot caught a real bug that every static check missed

First run **panicked at startup**: the exception image's `words()` was called by
the banner at `main.rs:737`, *before* `load_rom` published the ROM at `:766`.

This is the load-bearing lesson of the whole exercise. At the moment it failed,
**all of the following were already true and verified**: 32/32 shard digests
reproduced from geometry, 0 of 32 `WORDS` arrays, 0 of 33 `EXPECTED_WORDS`,
zero ROM-word hits with both controls passing. None of it proved the binary
ran, and it did not.

Fix: **a count is pure geometry and must not need a ROM.** Added `word_count()`
returning `(rom_end - rom_start) / 4`. The error was reaching for the words when
only the length was wanted — the same conflation the substitution exists to
avoid. Every other ROM-dependent call site was then audited rather than fixing
only the one that fired; `main.rs` was the sole exposed site.

### GUEST BYTE-IDENTITY: PASSES, 8 of 8 — and the "deviation" was my instrument

**Final result. The geometry substitution is guest byte-identical.**

| run | binary | result |
|---|---|---|
| **geometry** | 88,988,512 B, 0 ROM words | **8 of 8 match** |
| control (change reverted) | 95,326,192 B, 126 ROM words | **8 of 8 match** |
| verify-ON (archived) | 95,303,296 B | **8 of 8 match** |
| peer 19:21 | pre-dates all of today | **8 of 8 match** |

`gfx_submits=11153`, `audio_submits=7685`, `sp_tasks=18838`,
`vi_interrupts=8386`, `controller_ops=2390`, `sim_time=13112786076`,
`fields=7699`, `render_error=None`.

#### The phantom, kept because the error is the instructive part

For ~75 minutes this section reported a **303-submit deviation** in
`gfx_submits` (11153 expected, 10850 observed) and three 25-minute experiments
were run to attribute it. **There was no deviation. The checking script was
wrong.**

Both numbers appear in the same log, on different lines, as different metrics:

```
[wm2000-block-progress] ... gfx_submits=11153 ...          <- run total
[frame-census] steady-state rendering evidence:
                  gfx_submits=10850 across the span        <- warmup excluded
```

The script scanned the whole file and took the **last** match. The census line
follows the progress line, so it compared a steady-state span count against a
whole-run expectation. The 303 gap is exactly the fields excluded by
`warmup_gfx=300`.

**Three lessons, each earned here:**

1. **Proving a check can fail is not proving it reads the right thing.** The
   script was tested against an injected wrong *value* and passed. It was never
   tested for reading the wrong *line*. Rule 6a done halfway.
2. **Agreement across runs is not validity when every run uses the same
   instrument.** Four binaries agreeing on "10850" felt like strong
   corroboration; it was one parsing bug reproduced four times. This is rule 23's
   shared-blind-spot in a new place.
3. **A comment is not a verification.** The offending line was annotated
   `# last occurrence wins: the summary line is emitted at end of run` — an
   assumption written down and never checked.

Two further defects surfaced in the same script while fixing it: `sim_time` was
matched off heartbeat lines rather than the `done:` line, and `fields` was
ambiguous between `total_fields=8295`, `transient_fields=595` and steady
`fields=7699`. All three counters are now anchored to named lines, and a log
without a `[wm2000-block-progress]` line is a hard exit rather than a silent
fallback to a scan that can match a different metric of the same name.

#### What the wasted runs did establish

Not nothing: the geometry binary, the reverted control, and the verify-ON binary
independently produce **byte-identical guest state**. That is stronger evidence
than the single run originally planned — obtained expensively.

#### The investigation record below is left standing

Everything that follows was written while the phantom was believed real. It is
retained rather than deleted: the attribution reasoning was sound given the
input, and it documents how three candidate causes were excluded. Read it as a
worked example whose premise turned out false.

**`10850` predates the geometry binary by 35 minutes.** Two runs from another
agent's mirror A/B — logs timestamped **19:21** and **19:26** — both report
`gfx_submits=10850` with identical `sim_time` and `audio_submits`. The geometry
binary was not built until **19:56**. Three independent runs across at least two
binaries, all `10850`.

So the geometry substitution **did not** cause it, established by a run that
predates the change rather than by elimination. It also shows the counter is
**stable run-to-run** (two runs five minutes apart, same value), so this is not
a drifting counter — the recorded `11153` is simply stale for every binary built
today. That mattered to exclude: a drifting `gfx_submits` would have undermined
every byte-identity claim made against this route.

**What those two runs were, checked rather than assumed.** They are from the
peer's mirror work, but they are **not** the gated/ungated A/B pair: both report
identical `exec_mirror_calls` (70.126 fast / 278.854 slow per field), so they are
two runs of the *same* lane. They therefore establish **run-to-run stability of
`gfx_submits` on one binary**, which is what excludes the drifting-counter
worry — but they do **not** eliminate mirror gating as a cause. That variable
remains open; it was tempting to claim and the logs do not support it.

They also differ from the geometry run in one instrumented respect worth
recording: both have `FN64_FRAME_CENSUS_POPULATIONS` armed (33
`[frame-populations]` lines) while the default route has none. That is the same
gating that made `over_budget` absent below.

**Method note worth more than the result:** all of this came from reading the
run-log directory, not from running anything. Three 25-minute experiments were
queued to answer a question two finished runs had already answered. Check for
existing evidence before designing an experiment.

**A constraint that rules out whole classes of explanation:** `gfx_submits` and
`audio_submits` come from the *same* `task_log()` borrow —
`host.rs:591` returns `(gfx_count(), audio_count())` as one tuple and
`frame_census.rs:539` reads `.0`. Any effect on sampling, census timing, or
counter plumbing generally would move both. Only graphics moved. Whatever this
is, it is specific to graphics-task dispatch.

For scale: 303 submits is ~55k steps' worth at the observed end-of-run rate
(~555 per 100k steps), so it is not a rounding or tail-truncation artifact.

### `FN64_WM_SHARD_VERIFY_LIVE_WORDS`: a mechanism that exists but does not fire

**This section was first written as "the flag is NOT guest-neutral — a
correction to §2a," to explain a `gfx_submits` deviation that turned out not to
exist (above). §2a needs no correction: there was no effect to explain.**

**What survives is a genuine source finding with no measured consequence**, kept
because the mechanism is real and someone will otherwise rediscover it. The
original framing is left visible rather than swapped out — it is an example of
building an explanation for a phantom, which is the hazard of reasoning from a
mechanism toward an observation instead of the other way round.

Sequence, so the reasoning is auditable:

1. Source inspection found a real mechanism (below) by which the detector could
   change block boundaries. Stated as "not guest-neutral."
2. A verify-ON run of the same route was then measured, with the detector
   confirmed armed (**1,872 `EXPECTED_WORDS` occurrences** in its generated
   source versus 0 in the geometry lane — rule 6, lanes proven different).
3. **`ImageChanged` occurred 0 times across the full route.** The early-exit
   path was never taken, so no block boundary was ever altered.

**Honest statement: the detector *can* change block structure; there is no
evidence it does on this workload; its removal is not demonstrated to carry a
behavioral cost.** The mechanism is real and worth knowing about — it is not
observation-only in the general case — but §2a's practical claim survives for
WM2000 on this route.

Leading hypothesis for the deviation, with a **mechanism confirmed in source**
though its firing on this route is **not yet demonstrated**.

The study (§2a) treats the live-word detector as observation-only: "a
defence-in-depth removal" plus a performance win. As a *content* channel that is
right. As a *behavioral* channel it is wrong. The emitted macro
(`emit/mod.rs:571`) is:

```rust
if let Err(miss) = verify_precompiled_instruction_word($bank, ...) {
    finish!(BlockExit::ImageChanged { at: ..., miss });
}
```

`finish!` **exits the runner loop**, and downstream `program.rs:1329` reads:

```rust
if !matches!(result.exit, BlockExit::ImageChanged { .. }) {
    self.observe_execution_destination(...);
}
```

So on a genuine word mismatch the detector **changes where a block ends and
suppresses the execution-destination observation**. WM2000 reloads overlays
continuously (45 overlay/generation events in the measured run), which is
exactly when a verified PC can legitimately hold different words.

This also fits the gfx-only asymmetry: the verification is emitted into the AOT
guest runners that generate display lists; audio dispatch does not traverse
those per-instruction checks the same way. And the timeline fits — `11153` was
recorded on a **verify-ON** binary, while every lane since has been verify-OFF.

**Not yet shown: that it fires here.** The mechanism requires an actual mismatch
at a verified PC. Confirming it needs a verify-ON run of the same route; a
mechanism that *could* explain an effect is not one that does.

**And a candidate explanation was refuted at zero cost by reading two adjacent
functions.** `FN64_FRAME_CENSUS_POPULATIONS` looked like it might select a
different source for `gfx_submits` (`frame_census.rs:538-539`):

```rust
let counters = population_split_enabled().then(Counters::sample);
let gfx_submits = counters.map_or_else(|| crate::task_counts().0, |c| c.gfx_tasks);
```

It does not. `Counters::sample()` (`:267-268`) opens with
`let (gfx_tasks, audio_tasks) = crate::task_counts();` — **both branches read
the same function.** Armed or unarmed, `gfx_submits` is the same value. The
populations gate explains `over_budget`'s absence and nothing else.

Worth recording as method: the cheap test was reading the callee, not running a
25-minute experiment. Look for the cheap test first.

### A negative worth keeping: WM2000 never self-modifies at a verified PC on this route

`ImageChanged = 0` over the full 1.5M-step route, with the detector armed
(1,872 `EXPECTED_WORDS` occurrences). **No live instruction word ever mismatched
its baked expectation.**

That is a fact about WM2000, not only about the flag. The route performs
continuous overlay reloading — 45 overlay/generation events, four distinct
generations entered — and the working assumption was that reloads would trip the
detector at some verified PC. They do not. Overlay generations are swapped as
whole immutable images through the generation catalog rather than by writing
over live code that a runner is mid-execution on.

Useful to whoever next reasons about self-modifying code here: the
defence-in-depth detector that §2a debates removing **caught nothing on the one
route we have measured**, which is a materially different situation from "it is
protecting us and we are turning it off." It says nothing about other titles or
other routes.

**If confirmed, the release consequence is independent of the geometry work:**
turning the detector off is not free, and §2a's cost accounting is incomplete.
The two changes must stay separated — geometry is a *content* change with a
byte-exactness criterion (met: 32/32 digests, zero ROM words); the flag is a
*behavioral* change with a measurable effect (open).

### Observed in passing during the owner's windowed run — NOT investigated

The owner launched `wm2000-shell` against the geometry binary with his own ROM.
It loaded the ROM at launch, rebuilt all 32 code banks from geometry, matched the
boot context, rendered **1,080+ frames at p50 34.4 ms**, and exited cleanly —
**with zero copyrighted ROM content compiled in.** The release shape works end
to end.

Two defects were visible in that session. Both are recorded as observations
only; neither was investigated and neither is related to the ROM work:

- **Audio produced 1,071,984 samples, all zero** (`nonzero=0` throughout). That
  is silence being *produced*, not starvation — the delivery path is healthy and
  the content is empty, which per rule 22 are different failures.
- **`osViSwapBuffer calls=0`** across the whole session.

Neither is caused by the geometry substitution: the guest byte-identity tuple
matches exactly, so the emulated program is unchanged. They predate this work.

### An instrument trap worth generalizing: a warning gated on the flag it warns about

While checking the byte-identity tuple, `over_budget` came back NOT FOUND. It
turned out to be benign — the counter lives in the `[frame-populations]` block
(`frame_census.rs:1150`), gated on `FN64_FRAME_CENSUS_POPULATIONS`
(`:454`), which `render-benchmark.zsh` never exports. The channel is
known-absent on the default route, not silently dark.

**But the reason it looked silent is the durable lesson.** There *is* a
NOT-ARMED notice for exactly this case at `frame_census.rs:1331`
(`[frame-populations] counters NOT SAMPLED (FN64_FRAME_CENSUS_POPULATIONS
unset)`). It did not print because **the block that would print it sits behind
the very gate it warns about.** A warning conditioned on the same flag it warns
about is unreachable by construction: it can only fire when it is not needed.

This is the sharper form of the peer's filtered-warning case recorded in
`render-benchmark.zsh` (two 25-minute runs lost to a NOT-ARMED notice that was
printed but filtered out of the watched stream). There the warning existed and
was hidden; here it exists and cannot fire. Both produce the same observation —
a clean-looking log from an unarmed instrument — so **"no warnings in the log"
is not evidence that the instruments were armed.** Check the gate, not the
absence of complaints.

Practical consequence adopted here: the recorded expectation file distinguishes
`NOT FOUND` from `match` rather than only flagging mismatches, so a run that
emitted nothing cannot report "all match". That check produced this false alarm,
which is the correct trade — a false alarm from a working check beats a quiet
pass from a broken one.

## KNOWN LIMITATION: hermetic build-identity mode cannot work, and the reason is structural

`wm2000-block-boot` has a build-provenance mode entered by one CLI argument
(`runner_reports.rs:2-16`). It deliberately does not load a ROM
(`main.rs:742-744`) and its launcher runs the child with **`.env_clear()`**
(`fn64-boot-harness/src/generated_runner_build/build.rs:1874`), so no `ROM`
variable exists. It nevertheless reaches `construct_catalog_program`, because
the identity early-return sits *after* the call (`main.rs:951` constructs,
`:985` returns). With geometry-sourced words that mode now **panics** in
`shard_words`.

**The tension is real and does not dissolve.** `.env_clear()` exists precisely
so a build identity cannot depend on ambient environment — that is what makes
the attestation reproducible. Geometry-sourced words *require* ambient
environment, because the ROM is the user's file named by an env var. The mode
wants to attest what was built without possessing what it was built from.

**Reordering does not fix it** (checked, not assumed).
`generated_runner_build_identity` takes the constructed `CatalogBlockProgramV1`
and draws nine payload fields from `generated_runner_source_attestation()`,
including `program_identity_sha256` — which is the `evidence_snapshot` identity
and hashes every instruction word (`program.rs:1130`), while the attestation
re-hashes actual span words (`catalog_v1.rs:250`). Moving the early return
above `:951` would leave nothing to report.

**What was explicitly rejected:** letting `code_bank()` yield placeholder words
when no ROM is published, with identity mode skipping the digest asserts. The
`code_bank_sha256 == expected.code_sha256` checks are the entire reason
geometry-sourced words can be trusted. A lane where they may pass against
fabricated input would destroy that property — worse than the panic.

**Not a blocker for a release build.** No gate binary references
`runner_reports`; the three scripts naming `generated_runner_build` cite it as a
source path in linter allowlists, and none invoke it. Normal boot, render, and
the ROM-content result are unaffected. Recorded as an open design conflict
rather than a defect to patch: whoever revisits it must choose between hermetic
identity attestation and a content-free artifact, because under this design
they are mutually exclusive.

### Reproducing

**The geometry lane, end to end** — build, rule-19 source check, and the search
with all three controls:

```
scripts/rom-content-audit-accept.zsh
```

Exits non-zero unless the build succeeds, the generated source carries **0**
`WORDS` / **0** `EXPECTED_WORDS` / geometry in **every** shard, and the search
reports zero ROM-word hits with its controls passing.

**Guest byte-identity** (the emulated program must be unchanged):

```
recomps/wm2000/reference/wm2000-routes/render-benchmark.zsh \
  --binary target-audit-geometry/release/wm2000-block-boot --steps 1500000
python3 scripts/check-byte-identity.py scripts/byte-identity-1p5M.txt \
  /tmp/fn64-render-benchmark-<pid>-<stamp>.log
```

Read the **unfiltered** per-run log the script prints, not the filtered stream:
the allowlist is for watching a live run and has hidden a fatal panic before.

**The original two-lane audit**, unchanged:

```
scripts/rom-content-audit-build.zsh                 # both lanes
scripts/rom-content-audit-size.zsh                  # sizes + per-section delta
python3 scripts/rom-content-audit-search.py \
  --rom "$ROM" \
  --binary target-audit-verifyoff/release/wm2000-block-boot \
  --binary target-audit-verifyon/release/wm2000-block-boot \
  --require-control target-audit-verifyon/release/wm2000-block-boot
```

**On the controls, all three of which are load-bearing:**

- `--require-control` proves the search finds ROM content in a **real binary**
  known to contain it. Not optional decoration.
- The **synthetic control** (always on) proves the matcher works in all four
  orderings against a planted needle, and rejects a needle never planted. It
  depends on no binary — necessary once *both* lanes are clean, at which point
  `--require-control` has nothing left to find.
- `--require-clean` is the acceptance assertion itself.

Keep `target-audit-verifyon/` — it is the only archived binary known to embed
ROM words, and therefore the only real-binary positive control available.
