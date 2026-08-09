# Making a second AKI title playable — scoping

Read-only study, 2026-08-08. No builds, no benchmarks, no edits were performed;
every claim below is from source, committed docs, or non-source artifacts on
disk. Where something could not be determined without a build, it is marked
**unknown** with the evidence that would settle it.

Question asked: *what does it actually take to make a SECOND AKI wrestling title
playable, and which title is cheapest?*

---

## Verdict up front

**Recommend No Mercy (NW4E). It is the only viable candidate**, and the reason
is not engine similarity or ROM size — it is that **three of the other four are
already bounded out of the lane by measured, in-tree evidence**:

- **Revenge and World Tour fail host-binding discovery outright** — 0 candidates
  for a required symbol. Not "harder"; blocked at a discovery step that precedes
  everything else.
- **VPW2** passes host bindings 15/15 but is a Japanese ROM with no answer key
  and no `FN64_DISCOVER_*` environment entry, so nothing grades it.

This is a much narrower field than the "5 titles CPU-recompile" headline
suggests. The honest framing is **1 done, 1 reachable, 1 ungraded, 2 blocked**.

**Second, and more consequential for planning: the gap is smaller than the
`gate_rom_recompile` comment implies, and it has shrunk materially in the last
three days.** The two prior scoping docs on this subject
(`docs/plans/per-title-shard-generation.md`,
`docs/plans/corpus-certification-frontier.md:1786-1846`) both name hardcoded
WM2000 topology constants as the critical path. **Those constants have since
been retired.** Estimating from those docs today would overstate the work.

---

## 1. What exists beyond `gate_rom_recompile`, itemised

`gate_rom_recompile` (`crates/fn64-discover/src/bin/gate_rom_recompile.rs`)
takes a ROM, runs `run_discovery_auto`, packs every proven bank, emits Rust,
compiles it with real rustc, and probes arbitrary guest PCs. Its own scope
disclaimer is at lines 16-18. Here is everything the playable WM2000 lane has
that the gate does not produce.

| # | Item | Where | Classification |
|---|---|---|---|
| 1 | Shard pack emitter | `examples/wm2000-block-shards/build.rs` | **Mechanical** |
| 2 | Shard crate directories (32) | `examples/wm2000-block-shards/shard*/`, `overlay*/` | **Mechanical** (generatable boilerplate) |
| 3 | Shard inventory / package list | `shard_inventory.in`, `generated_runner_build/mod.rs:142-162` | **Mechanical but structural — the critical path** |
| 4 | Host-binding recognizers (15) | `crates/fn64-discover/src/host_bindings/mod.rs` | **Mechanical** (signature-scanned) |
| 5 | Boot example build | `examples/wm2000-block-boot/build.rs` | **Mechanical** |
| 6 | Boot context capture | `~/Code/aki-recomp/captures/*-boot-context.json` | **Semi-mechanical** (one automated emulator run) |
| 7 | Executable-image group (≥3 captures) | `captures/wm-general-exception-images/` | **Semi-mechanical** (automated run + human-located PCs) |
| 8 | Scripted input schedule | `reference/wm2000-routes/*.schedule` | **Bespoke** (hand-authored, frame-verified) |
| 9 | Byte-identity tuple | `.claude/skills/fn64-perf-method/SKILL.md:254-259` | **Bespoke** (per-title, only exists once a route runs) |
| 10 | RSP audio / RDP graphics | `crates/fn64-audio/`, RT64 | **Shared** — not per-title |

### 1.1 The shard pack — 22 title references, only 6 functional

I counted 22 case-insensitive `nwxe|wm2000|wm_|WM ` matches in
`examples/wm2000-block-shards/build.rs` (813 lines). Classified:

- **16 are doc comments or an env-var name** — lines 32, 121, 156, 162-163, 267,
  271, 317, 586, 613, 616, 733, 737, 740, and the `WM_BLOCK_RUNTIME_HOST_SYMBOLS`
  references at 263/270. Zero functional coupling.
- **6 are functional**, and all six are *string prefixes on Cargo package names*:
  `build.rs:324`, `:330`, `:346` (emission) and `:666`, `:669`, `:677`
  (`package_target` parsing), plus the `ROM` env panic text at `:793`.

**Everything geometric is already derived from discovery:**

- `boot_bank_va_start` (`build.rs:215-245`) takes the boot VA from the proven
  `RomMapping`, asserting agreement with the IPL3 DMA extent. The doc comment at
  `build.rs:20-23` is explicit that the virtual base is CIC-dependent and "derived
  per ROM, never assumed."
- `resident_shard_counts` (`build.rs:201-210`) is a pure tiling rule. Its test
  (`build.rs:736-741`) exercises **both** WM2000 (15,2) and No Mercy (14,3)
  splits and states "neither is privileged", plus an exhaustive sweep over every
  word-aligned split in the 1 MiB boot copy (`build.rs:747-775`).
- `package_inventory()` (`build.rs:318-353`) derives the whole package topology
  per ROM.
- Overlay extents come from `admitted_overlay_load_recipes_v1` with
  `SearchConfig::aki_family()` (`build.rs:650-662`) — no per-title table.

**This contradicts the two prior scoping docs.** Both name
`assert_eq!(…, 2, "resident-tail package topology must cover exactly two
shards")`, a "first overlay must land in shard 14" assertion, and
`Boot(index @ 0..=13)` as the blockers. **None of the three exists in the current
file** — `build.rs:740` explicitly calls them "the retired `Boot(0..=13)` / `== 2`
constants." They were removed by commits `7c1d399` ("derive resident boot/tail
topology from discovery"), `330f542` ("derive package topology"), and `f3b0ebc`
("collapse the six shard inventories into one included source"), all landed
after `docs/plans/per-title-shard-generation.md` was written on 2026-08-06.

Likewise, the "four hand-written `const` arrays, two already stale and mutually
inconsistent" is **now one file**: `shard_inventory.in` (57 lines), `include!`d
by every consumer (`generated_runner_build/mod.rs:133-143` documents exactly why).

**Classification: mechanical.** The remaining work is renaming a package-name
prefix and generating crate directories.

### 1.2 Shard crate directories — mechanical boilerplate

32 directories, each containing only a 17-line `Cargo.toml`. The sole
title-specific content is the `name =` line; `build`, `lib`, and all dependency
paths point at shared parents (`examples/wm2000-block-shards/shard00/Cargo.toml`).
Generating 38 of these for No Mercy is templating, not design.

### 1.3 The package list — the actual critical path

`SHARD_COUNT` is a compile-time constant derived from the length of
`shard_inventory.in`, and `PREPARED_PACKAGES` / `SHARD_MANIFEST_DIRS` are
`[&str; SHARD_COUNT]` fixed-size arrays built in `const` blocks
(`crates/fn64-boot-harness/src/generated_runner_build/mod.rs:142-162`).

This is the one genuinely structural item. A second title has a *different
number* of shards (No Mercy 38 vs WM2000 32), so the boot harness cannot express
it without either (a) making the inventory runtime data rather than a
compile-time array, or (b) generating a per-title inventory file and selecting
between them. The `include!` edge from a crate into `examples/` is deliberate and
documented (`mod.rs:138-141`), but it is a compile-time edge, which is precisely
what makes the count non-negotiable at runtime.

**Classification: mechanical but structural.** The counts are already computed;
this is a build-system change. Prior estimate M–L, and I agree — this is now the
*only* remaining item of that size.

### 1.4 Host-binding recognizers — found, and they are generic

The gate says the boot harness "needs host-binding recognizers this gate never
consults." **A grep for `fn recognize` in `fn64-boot-harness` correctly returns
nothing** — the harness contains zero host-binding code. The recognizers live in
`crates/fn64-discover/src/host_bindings/mod.rs` (1,782 lines); the harness
receives an already-issued catalog one level up, from the example crate's build
script.

- **16 variants** in `enum HostBindingSymbol` (`mod.rs:23-40`), of which **15 are
  the required catalog** (`WM_BLOCK_RUNTIME_HOST_SYMBOLS`, `mod.rs:44-60`). The
  16th, `OsDriveRomInit`, is explicitly optional (`mod.rs:1400-1405`).
- All 15 are **libultra OS functions** (`osCreateMesgQueue`, `osSpTaskLoad`,
  `osSetTimer`, …) — generic N64 system routines, not game logic.
- **15 role-level `is_*` structural predicates**, matched by `unique_match`
  (`mod.rs:1286`) as sliding windows over the resident image, requiring exactly
  one hit or failing loudly with `NonUniqueSemanticMatch`.
- **Zero hardcoded guest addresses and zero title checks in 1,782 lines.** The
  only literals are public hardware MMIO constants — `0xa480` (SI_STATUS_REG,
  `mod.rs:1338`) and `0xa600` (PI_DOM1_ADDR1, `mod.rs:1383`). The module doc states
  the rule: *"Addresses are outputs, never signatures"* (`mod.rs:3`). The 64DD
  recognizer's comment makes it explicit: *"recognised by what it does, not by any
  address"* (`mod.rs:1355`).

**How many are WM2000-specific? Zero.** The name `WM_BLOCK_RUNTIME_HOST_SYMBOLS`
and `discover_wm_block_runtime_host_bindings` is historical naming, not coupling.

**The measured per-title result** (`docs/plans/corpus-certification-frontier.md:1810-1814`,
produced by the in-tree probe `crates/fn64-discover/examples/probe_host_bindings.rs`):

```
WWF WrestleMania 2000     OK 15/15
WWF No Mercy (Rev A)      OK 15/15
Virtual Pro Wrestling 2   OK 15/15
WCW-nWo Revenge           FAIL -- OsSetEventMesg, 0 candidates
WCW vs. nWo World Tour    FAIL -- OsCreateMesgQueue, 0 candidates
```

**This is the single most important fact in this study.** The two 1998 WCW titles
are an earlier libultra revision the current signatures do not match. That
disqualifies them regardless of their smaller size.

*Caveat (rule 23):* these predicates pin register allocation and instruction
positions per-word, so they match one compiler's schedule.
`crates/fn64-discover/reference/clay-host-binding-variants.json` (15 entries)
documents a proposed generalization per symbol for a second engine; nothing in
`src/` reads it. So "15/15 on No Mercy" is a real measurement, but the
recognizers' generality is narrower than "structural matching" suggests.

### 1.5 The boot example — already substantially generalized

`examples/wm2000-block-boot/build.rs` (1,291 lines) has 48 lines mentioning the
title, but the geometry is derived. The decisive evidence is `build.rs:414-418`:

> Overlay COUNT is a property of the ROM, not of this lane: discovery recovers 4
> for WM2000, 5 for No Mercy, 2 for Revenge and World Tour, and 4 for VPW2.
> Pinning it at 4 made the lane WM2000-only for no reason — every geometry below
> is already derived from `overlay_recipes` itself.

Someone has already run discovery against all five ROMs and de-hardcoded this
path. The remaining assert is only "at least one overlay" (`build.rs:419-422`).

Note the exception-vector image is located by **searching the ROM for the
captured words and requiring exactly one match** (`build.rs:1195-1225`), rather
than hardcoding the offset — so it fails loudly on a ROM where the assumption
does not hold.

### 1.6 Boot context — semi-mechanical, and cheaper than feared

**This is the item the brief flagged as a potential major cost. It is not.**

A boot context is a **~950-byte JSON register snapshot at the ROM header entry
point** — the post-IPL3 CPU state. Schema `fn64.boot-context.v1`: 32 GPRs, HI/LO,
32 CP0 registers, CIC IPL3 digest, region, `entry_pc`.

It **cannot be synthesized** — stated normatively at
`scripts/capture-boot-context.zsh:4-9`: a hand-written one *"would pass schema
validation while binding register state the hardware never produced — forging
the very authority under audit."* Binding is enforced hard by
`crates/fn64-boot-harness/src/boot_context.rs:35-38`
(`BootContextLoadError::RomIdentityMismatch`).

**But it does not require a human playing the game.** The producer log
(`captures/wm-general-exception-images/run-1/producer.log`) shows a headless
mupen64plus run: *"No video plugin attached"*, *"No audio plugin attached"*,
**"No input plugin attached"**, pure interpreter, stopping at
`entrypoint pause at 0x80000400`. It is one command:

```
scripts/capture-boot-context.zsh ~/Code/aki-recomp/games/NW4E/nomercy.z64
```

**Empirical cost: four of these were produced in a 3-minute window on Aug 5**
(file mtimes 18:52, 18:53, 18:54, 18:55) — under a minute each.

**All four remaining AKI titles already have boot contexts on disk**, contrary to
the "boot context captured: NO" row in
`docs/plans/corpus-certification-frontier.md:1755-1762`, which predates them.

**All three toolchain prerequisites are PRESENT on this machine** (verified by
`ls`; they live in `/private/tmp`, which macOS reaps, so this can lapse):

```
PRESENT  /private/tmp/fn64-mupen-core-current/.../libmupen64plus.dylib
PRESENT  /private/tmp/fn64-rsp-hle-build/.../mupen64plus-rsp-hle.dylib
PRESENT  /opt/homebrew/Cellar/mupen64plus/2.6.0/include
```

**The ROM-identity problem, measured.** I hashed every AKI ROM on this machine
against the five capture digests:

| title | capture binds | on-disk ROM | match |
|---|---|---|---|
| WM2000 | `cbd44033…` | `games/NWXE/wm2000.z64` = `cbd44033…` | **yes** |
| World Tour | `3711c883…` | `Downloads/…World Tour (USA) (Rev 1).z64` = `3711c883…` | **yes** |
| No Mercy | `fc561fce…` | `games/NW4E/nomercy.z64` = `11640379…` | no |
| VPW2 | `358e9a34…` | `Downloads/…Freem Edition.z64` = `7706ed94…` | no |
| Revenge | `66c137d3…` | Unlocked `fd9996c7…` / Starrcade `d8c097f8…` | no |

Three further No Mercy variants were checked and none matches `fc561fce…`
(`(E) (V1.1)` = `381d46cf…`, `Beta Sep-11-2000` = `3dd1f846…`).

**This is a minor finding, not a blocker.** Re-capture against the on-disk ROM is
one sub-minute command. The captures were evidently made from a differently-
sourced corpus (`corpus-certification-frontier.md:1119-1120` refers to certifying
"from the corpus ROMs rather than the private paths"; that corpus root is not
recorded anywhere in this worktree — **unknown**, settled by finding the 287-ROM
directory).

### 1.7 Executable-image group — semi-mechanical, one human step

For WM2000 this is **one image, 16 bytes, 4 words at VA `0x80000180`** — the MIPS
general exception vector preamble (`captures/wm-general-exception-images/run-1/image.json`,
`byte_len:16`). Not a large gameplay trace.

Captured by `scripts/capture-wm-executable-image-group.zsh` (in-tree, MIT), which
runs the producer **≥3 times** (`:10`, `:91`) and validates byte-identity across
runs into a group receipt via `validate_executable_image_group`. Reproducibility
is enforced on producer, PCs, lineage, geometry, digest, and exact words
(`crates/fn64-discover/src/trace/mod.rs:127-143`).

**The human step:** the script requires `--capture-pc`, `--first-pc`, `--start`,
`--word-count`, `--image-id` (usage at `:18`). These are **not discovered** — a
human locates them, typically via the `FN64_WATCH_WORD` diagnostics in
`tools/mupen-trace/README.md`. `docs/BOOT-NOTES-WM2000.md:1310` calls it a
"manual producer recipe."

For a sibling title the target is the same architectural artifact (the exception
vector at `0x80000180`), so the PCs are likely to transfer nearly unchanged. But
`retired_instructions: 262016` — the point at which the guest has written the
vector — **is** title-specific and must be found empirically. **Unknown:** whether
No Mercy needs exactly one image group or more. Settled by running the boot lane
and reading what it demands.

**No executable-image group exists for any title but WM2000.**

### 1.8 Input schedule and byte-identity — genuinely bespoke

`reference/wm2000-routes/entrance-to-match.schedule` (124 lines) and
`two-player-match.schedule` (175 lines) are hand-authored controller scripts:
`port first_read end_read buttons_hex stick_x stick_y`, clocked on **controller
read ordinal**, not time. Every screen transition was *"read off a dumped frame,
not inferred from what a wrestling menu 'probably' does"*, with committed
reference frames.

The file's own header records that a prior 420,000-step route was lost to
`/private/tmp` reaping and **is no longer reproducible**. That is direct evidence
of the cost: this is menu-by-menu human work against a running emulator, and it
does not transfer between titles — No Mercy's menus differ from WM2000's.

The byte-identity tuple (`gfx_submits=16586 audio_submits=11005 …`) is likewise
per-title and only comes into existence *after* a route runs successfully. It is
a regression guard, not an input.

### 1.9 RSP audio / RDP graphics — shared, not per-title

The gate names these as separate subsystems, which is true, but they are **not
per-title costs**. I found 114 title mentions across `crates/fn64-abi/src/` and
`crates/fn64-recomp-rs/src/` and **every one is a test name or a comment** — zero
title-specific runtime logic. `crates/fn64-audio/src/rsp/` is a complete MIT-clean
RSP audio stack (verified end-to-end for OoT, per `docs/WM2000-AUDIO-STATUS.md`).

**Caveat:** shared does not mean free. `docs/plans/corpus-certification-frontier.md:1524-1531`
names three runtime blockers *every* title will hit — the overlay-tail transfer,
reconciliation speed (23.5x slower than realtime), and unproven input driving
gameplay. And WM2000 itself is **not yet playable at speed**: 52.79 ms/field =
3.17x the 16.667 ms budget. A second title inherits that.

---

## 2. Classification summary

**Mechanical** (derivable by discovery from any ROM, no human, no capture):
shard emitter geometry; boot/tail tiling; overlay recipes; all 15 host-binding
recognizers; shard runner source and digests; boot example geometry; shard crate
`Cargo.toml` boilerplate.

**Mechanical but structural** (code change, no new information):
the package inventory / `SHARD_COUNT` compile-time array. *This is the critical
path.*

**Semi-mechanical** (needs an automated emulator run):
boot context (~1 min/ROM, one command); executable-image group (≥3 runs, plus a
one-time human PC hunt).

**Bespoke** (hand analysis):
the input schedule; the byte-identity tuple; generalizing host-binding signatures
*if* a title needs it (Revenge/World Tour do; No Mercy does not).

---

## 3. Recommendation: No Mercy (NW4E)

### Why not the others

| title | disqualifier |
|---|---|
| **Revenge** | Host bindings **FAIL** — `OsSetEventMesg`, 0 candidates. 1998 libultra. |
| **World Tour** | Host bindings **FAIL** — `OsCreateMesgQueue`, 0 candidates. Also the only one of the five whose code span does not start at `0x878` (`corpus-code-span-locality.tsv:83`, 2 spans, concentration 0.63) — a different ROM layout. |
| **VPW2** | 15/15 host bindings, but Japanese, no answer key, no `FN64_DISCOVER_VPW2_*` in `.claude/local.env`. Nothing grades it, so a regression could not be detected. |

Note the smaller titles being *older* is not a soft preference here — it is the
direct cause of the host-binding failure. Size favours Revenge/World Tour;
engine generation vetoes them.

### Why No Mercy

1. **Host bindings 15/15** — measured, the gating discovery step.
2. **Closest sibling** — engine similarity to WM2000 is 0.0629 against a corpus
   random-pair median of 0.0025 (`corpus-invocations.md:88-96`), the highest pair
   in the corpus.
3. **Best-graded title in the corpus** — recall 873 solo / **925 donor**, wrong=0,
   the highest of the five (`docs/plans/keyfree-recall-roadmap.md:1-19`). Its
   answer key exists (`games/NW4E/syms/dump.toml`) and it is wired into
   `.claude/local.env` as `FN64_DISCOVER_NW4E_ROM` / `_DUMP`.
4. **Its geometry is already measured and recorded** —
   `docs/plans/per-title-shard-generation.md:99-118` has the full per-overlay
   layout: 5 overlays, 14 boot shards + 3 tail shards, **38 packages** vs
   WM2000's 32.
5. **Its two certification blockers are already found and fixed** — the
   non-terminating `partition::same_bank_overlaps` walk (zero-length CFG fence
   blocks), and `InvalidResidentSplit`, fixed in `e5e7d39`
   (`docs/plans/aki-recompile-certification.md:30-118`).

**Against it:** largest of the five (57,284 blocks / 6 banks), and
`exact_aot = 0` — No Mercy admits no exact owner at all
(`docs/DISCOVER-PLAN.md:2465-2475`), unlike WM2000's 110. Whether that matters
for booting as opposed to grading is **unknown**; the boot lane consumes
`block_aot` (1,820 for No Mercy), which is present.

---

## 4. Sequence, risks, and honest effort

### Sequence

1. **Re-capture the boot context** against the on-disk ROM.
   `scripts/capture-boot-context.zsh ~/Code/aki-recomp/games/NW4E/nomercy.z64`.
   ~1 min. *Do this first — it is cheap and it validates the whole capture
   toolchain is still alive before any code change.*
2. **Confirm host bindings on the exact ROM you will build from.**
   `cargo run -p fn64-discover --example probe_host_bindings`. The published
   15/15 was measured on "Rev A"; the on-disk ROM is an Unlocked variant
   (`11640379…`). **This is the highest-value cheap check in the whole plan** —
   rule 23: the 15/15 and the ROM you have may not be the same ROM.
3. **Generate the package inventory per title.** The structural change: make
   `SHARD_COUNT` / `PREPARED_PACKAGES` / `SHARD_MANIFEST_DIRS` per-title rather
   than one compile-time array, and generate the 38 crate directories.
4. **Rename the package prefix** in the 6 functional sites (`build.rs:324, 330,
   346, 666, 669, 677`) to be title-parameterized.
5. **Run the prepared-shard producer** for No Mercy (`producer.rs:66-121`).
6. **Capture the executable-image group** — locate the PCs, then ≥3 runs.
7. **First full build + link.**
8. **Author an input schedule** — the long pole for *playable* as opposed to
   *boots*.

Steps 1-2 are ~10 minutes and de-risk everything after them. **Do not skip step 2.**

### What is likely to break

- **Step 2 returning <15/15 on the Unlocked ROM.** The published figure is for Rev
  A. If an unlock patch touched a matched window, a recognizer goes non-unique or
  zero-candidate. Would move No Mercy toward the Revenge/World Tour category.
- **`exact_aot = 0`.** No prior title has booted from a pack with no exact owner.
  Unknown whether the lane cares.
- **Build cost.** WM2000's 32 packages → 121 MB binary. No Mercy's 38 → ~145 MB
  estimated. `SELECTED_BUILD_CARGO_JOBS_V5 = 2`
  (`generated_runner_build/mod.rs:127`) is folded into build-evidence digests, so
  it is **not tunable** — more crates cannot be absorbed by more parallelism. The
  memory guard's 3600 s ceiling may need raising. *This estimate is inherited
  from `per-title-shard-generation.md:250-264`, not measured here.*
- **Digest agreement.** `canonical_definition_sha256` must keep agreeing across
  every consumer of the inventory once it stops being one constant array. This is
  where a structural change of this shape usually bites.
- **A stale prior-doc trap.** Two committed docs describe blockers that no longer
  exist, and one (`corpus-certification-frontier.md:1828-1830`) carries shard
  layouts explicitly flagged stale by a newer doc but never corrected. Anyone
  planning from them will estimate the wrong work. **Both should be annotated.**

### Effort

Deliberately in relative sizes, not hours — no measurement was permitted, and
rule 1 says a number without a measurement behind it is not an estimate.

| Step | Size | Risk | Basis |
|---|---|---|---|
| 1. Re-capture boot context | **XS** | Low | 4 done in 3 min on Aug 5 |
| 2. Verify host bindings | **XS** | **Med** — could reclassify the title | probe exists |
| 3. Per-title package inventory | **M–L** | **Med-high** | the one structural item |
| 4. Package prefix rename | **S** | Low | 6 string sites |
| 5. Run producer | **XS** | Low | one command |
| 6. Executable-image group | **M** | Med-high | no prior art outside WM2000 |
| 7. First build + link | **M** | Med | ~145 MB at `-j2` |
| 8. Input schedule | **L** | **High** | bespoke; prior one was lost and unreproducible |

**Critical path: step 3.** Everything else is either sub-minute or well-precedented.

**The realistic honest answer to "what does it take":** getting No Mercy to
**boot** is mostly mechanical and the remaining structural item is one
build-system change. Getting it **playable** is gated on step 8, which is
irreducibly human, and then on the shared performance problem — WM2000 boots and
renders today and is still 3.17x over the frame budget. **A second title that
boots is a realistic near-term goal; a second title that is playable at speed
inherits an unsolved problem that is not title-specific.**

---

## 5. Corrections to the record

1. `docs/plans/per-title-shard-generation.md:214-231` — the three hardcoded
   topology assertions it names as blockers **no longer exist** (retired by
   `7c1d399`, `330f542`). Its "four `const` arrays, two stale" is now one
   `shard_inventory.in` (`f3b0ebc`).
2. `docs/plans/corpus-certification-frontier.md:1755-1762` — the "boot context
   captured: NO" row is stale for all four titles; captures were made Aug 5.
3. `docs/plans/corpus-certification-frontier.md:1828-1830` — shard layouts
   `[3,1,6,8]` etc. were flagged stale by `per-title-shard-generation.md:120-128`
   and never corrected.
4. This study's own correction: the brief's premise that boot captures might
   "require a working emulator and a human playing the game" is **half right**.
   They require an emulator; they do **not** require a human playing — the
   producer runs with no input plugin at all. The human-in-the-loop cost is real
   but sits in the *input schedule* (item 8) and the *executable-image PC hunt*
   (item 7), not the boot context.

## 6. Explicit unknowns

- Whether host bindings resolve 15/15 on `11640379…` (Unlocked) rather than Rev A.
  **Settled by:** `probe_host_bindings`. *Highest-value open question.*
- Whether `exact_aot = 0` blocks the boot lane. **Settled by:** running it.
- How many executable-image groups No Mercy needs. **Settled by:** the build
  demanding them.
- Actual build/link cost for 38 packages. **Settled by:** one build. Not run here.
- Where the 287-ROM corpus directory lives (source of the capture digests).
  **Settled by:** locating the ROM matching `fc561fce…`.

## Next action, and why it is not done yet (2026-08-08, coordinator)

**Run `probe_host_bindings` against the on-disk No Mercy ROM.** That is the
single highest-value open item and everything downstream depends on it.

It could not be answered from the record. `corpus-certification-frontier.md:1811`
names the title as "WWF No Mercy (Rev A)" — a **title string, not a ROM digest**
— so the 15/15 cannot be matched to an artifact. The two facts (15/15 passes;
the on-disk ROM is `11640379…`) agree with each other while describing possibly
different ROMs, which is rule 23's shape.

It also could not be run: `probe_host_bindings` is not built in any target dir,
so settling it costs a build plus a run, and the `resume NET` measurement held
the machine (rule 9 — never build beside a benchmark).

**Do it first when the machine frees.** Three outcomes:

- **15/15 on the Unlocked ROM** — No Mercy is confirmed reachable and the
  variant question closes.
- **fewer than 15** — the reachable-title count drops from 2 to 1 unless a
  Rev A ROM is obtained, and the scoping recommendation changes.
- **a different failure** — the probe itself needs attention before any title
  planning continues.

Lesson worth carrying regardless: **record the ROM digest beside every per-title
result.** A title string cannot distinguish variants, and this project has four
No Mercy images on disk of which none matches the capture's `fc561fce…`.

## The critical path, located precisely (2026-08-08, coordinator)

Verified the study's two structural claims by reading, and the second is
tighter than "a build-system change" suggests.

**The shard generator is genuinely title-agnostic.** Of 14 WM2000/NWXE
references in `examples/wm2000-block-shards/build.rs`, **8 are comments or test
names, 1 is an error message, and 5 are Cargo package-name prefixes**
(`wm2000-block-shard-`, `-resident-tail-shard-`, `-overlay-` at `:324`, `:330`,
`:346`, `:666`, `:669`, `:677`). None encodes topology. Line 616 states the
generator already knows both splits: *"the index the split falls in is
per-title (WM2000: 14, No Mercy: 13)"*.

**The binding is in the boot harness, and it is a hardcoded path, not a
constant.** `generated_runner_build/mod.rs:142-143`:

```rust
const SHARD_INVENTORY: &[(&str, &str)] =
    &include!("../../../../examples/wm2000-block-shards/shard_inventory.in");
```

`SHARD_COUNT`, `PREPARED_PACKAGES` and `SHARD_MANIFEST_DIRS` are all derived
from that one `include!` in const context, so they follow automatically — they
are **not** independent per-title constants. The 37-entry inventory file is the
single source of truth and already describes itself that way.

Alongside it, `generated_runner_build/build.rs` hardcodes the same directory
**six times** (`:867`, `:874`, `:876`, `:882`, `:888` and `wm_shard_root`).

**So the per-title surface is one `include!` path plus six string literals in
one file** — not a build-system redesign. A second title needs its own shard
crate directory and inventory, and these seven sites need to select between
them. That is a smaller change than the study's M–L estimate implies, though
selecting at compile time across two inventories in const context is the part
that needs design rather than editing.

**Unverified:** whether anything outside `generated_runner_build` assumes the
WM2000 inventory. Grepped only that crate.

## Correction: the "missing emit.rs" failure is a stale path, not a missing file

`50d2c21` reported `generated_runner_build::tests::part1::independent_emitter_source_measurement_matches_the_linked_receipt`
as failing because `fn64-recomp-rs-codegen/src/emit.rs` is "missing from disk
entirely -- looks like a file-split in flight elsewhere."

**Checked: the split is committed, not in flight.** `emit.rs` became the
directory `emit/` (`mod.rs`, `ops.rs`) in `42307ab`, the #119 consolidation
wave. Nothing is missing.

The actual defect is a **stale hardcoded path** at
`generated_runner_build/build.rs:1121`:

```rust
for label in ["Cargo.toml", "src/lib.rs", "src/emit.rs"] {
```

and the comment two lines above says why it cannot simply be widened:

> This order is part of `GeneratedRunnerEmitterSourceReceiptV2`'s wire. The
> generic source-tree helper sorts labels and therefore cannot measure this
> receipt independently without changing its digest.

So the fix is **not** one line: the label list is wire-format for a receipt
digest, and replacing `src/emit.rs` with `src/emit/mod.rs` + `src/emit/ops.rs`
changes the digest and therefore the receipt version. That is a deliberate
change to an identity artifact, not a typo repair.

**Why the distinction matters:** "a file is missing, a split is in flight"
invites waiting for someone else to finish. "A committed split left a receipt's
label list stale" names an owner and a decision — whether to version the
receipt. The first reading would have parked this indefinitely.

Recorded rather than fixed: it is an identity-artifact change and belongs with
whoever owns the receipt schema.

## RESOLVED: No Mercy passes 15/15 on the ON-DISK ROM (2026-08-08)

The open variant question is closed. Ran the probe against the actual file
rather than trusting the record:

    $ cargo run --release -p fn64-discover --example probe_host_bindings -- \
        $FN64_DISCOVER_NW4E_ROM
    nomercy  OK  15/15

**ROM digest `11640379fdf534b3…`** — the Unlocked variant on disk, NOT the "Rev A" the
frontier doc's 15/15 was recorded against. Both pass.

So the pessimistic branch does not fire: **No Mercy does not reclassify toward
the blocked pile.** It remains the reachable second title, and the shard
selector landed in `50d2c21` can now be pointed at a real generated tree.

**Two process notes, both mine:**

The record could not answer this. `corpus-certification-frontier.md:1811`
files the result under the title string `"WWF No Mercy (Rev A)"` — a title,
not a digest — so two facts agreed while describing possibly different ROMs.
**The remedy already recorded above (state the ROM digest beside every
per-title result) is applied here: the digest is in this section.**

I also briefly concluded the probe "does not exist" because `git grep` over
`crates/*/src/bin/*` returned nothing. It is an `examples/` target
(`crates/fn64-discover/examples/probe_host_bindings.rs`, committed 2026-08-05),
and the frontier doc names that path two lines above the table I was reading.
A search of the wrong directory is indistinguishable from absence — the same
shape as rule 20, self-inflicted while investigating.
