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
(`examples/wm2000-block-shards/build.rs:161-170`) reads
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

`examples/wm2000-block-shards/build.rs:449-453` emits, per shard:

```rust
let _ = write!(metadata, "pub static WORDS: &[u32] = &[");
for word in words {
    let _ = write!(metadata, "{word:#010X}, ");
}
```

`examples/wm2000-block-shards/lib.rs:10` `include!`s that into every shard
crate, and `lib.rs:12-15` exposes it:

```rust
pub fn code_bank() -> CodeBank {
    CodeBank::new(BankId::new(BANK_ID), GuestPc::new(VA_START), WORDS.to_vec())
}
```

**It is linked and called in the shipped binary.**
`examples/wm2000-block-boot/src/dense_aot.rs:6-...` registers all 32 shards'
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

`examples/wm2000-block-boot/build.rs:1124-1133` emits
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
`crates/fn64-recomp-rs/src/semantic/mod.rs`, 1,877 lines, sharing the AOT
decoder and honoring the same `BlockExit` contract, with a differential
equivalence test (`crates/fn64-recomp-rs/tests/interp_differential.rs`).

It is **excluded from the shipping build by compile error**:
`crates/fn64-recomp-rs/src/lib.rs:33-34` makes `production-aot` and
`dev-interpreter` mutually exclusive, and
`examples/wm2000-block-boot/Cargo.toml:46` selects `production-aot`.
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

`examples/wm2000-block-boot`, `--release --features rt64`, both lanes, separate
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

### Reproducing

```
scripts/rom-content-audit-build.zsh                 # both lanes
scripts/rom-content-audit-size.zsh                  # sizes + per-section delta
python3 scripts/rom-content-audit-search.py \
  --rom "$ROM" \
  --binary target-audit-verifyoff/release/wm2000-block-boot \
  --binary target-audit-verifyon/release/wm2000-block-boot \
  --require-control target-audit-verifyon/release/wm2000-block-boot
```

`--require-control` is not optional decoration: it is what makes a reported
absence mean anything.
