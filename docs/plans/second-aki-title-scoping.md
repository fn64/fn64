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
| 1 | Shard pack emitter | `recomps/wm2000/packages/wm2000-block-shards/build.rs` | **Mechanical** |
| 2 | Shard crate directories (32) | `recomps/wm2000/packages/wm2000-block-shards/shard*/`, `overlay*/` | **Mechanical** (generatable boilerplate) |
| 3 | Shard inventory / package list | `shard_inventory.in`, `generated_runner_build/mod.rs:142-162` | **Mechanical but structural — the critical path** |
| 4 | Host-binding recognizers (15) | `crates/fn64-discover/src/host_bindings/mod.rs` | **Mechanical** (signature-scanned) |
| 5 | Boot example build | `recomps/wm2000/packages/wm2000-block-boot/build.rs` | **Mechanical** |
| 6 | Boot context capture | `~/Code/aki-recomp/captures/*-boot-context.json` | **Semi-mechanical** (one automated emulator run) |
| 7 | Executable-image group (≥3 captures) | `captures/wm-general-exception-images/` | **Semi-mechanical** (automated run + human-located PCs) |
| 8 | Scripted input schedule | `recomps/wm2000/reference/wm2000-routes/*.schedule` | **Bespoke** (hand-authored, frame-verified) |
| 9 | Byte-identity tuple | `.claude/skills/fn64-perf-method/REFERENCE.md:322-332` | **Bespoke** (per-title, only exists once a route runs) |
| 10 | RSP audio / RDP graphics | `crates/fn64-audio/`, RT64 | **Shared** — not per-title |

### 1.1 The shard pack — 22 title references, only 6 functional

I counted 22 case-insensitive `nwxe|wm2000|wm_|WM ` matches in
`recomps/wm2000/packages/wm2000-block-shards/build.rs` (813 lines). Classified:

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
paths point at shared parents (`recomps/wm2000/packages/wm2000-block-shards/shard00/Cargo.toml`).
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

`recomps/wm2000/packages/wm2000-block-boot/build.rs` (1,291 lines) has 48 lines mentioning the
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

Captured by `recomps/wm2000/scripts/capture-wm-executable-image-group.zsh` (in-tree, MIT), which
runs the producer **≥3 times** (`:10`, `:91`) and validates byte-identity across
runs into a group receipt via `validate_executable_image_group`. Reproducibility
is enforced on producer, PCs, lineage, geometry, digest, and exact words
(`crates/fn64-discover/src/trace/mod.rs:127-143`).

**The human step:** the script requires `--capture-pc`, `--first-pc`, `--start`,
`--word-count`, `--image-id` (usage at `:18`). These are **not discovered** — a
human locates them, typically via the `FN64_WATCH_WORD` diagnostics in
`tools/mupen-trace/README.md`. `recomps/wm2000/docs/BOOT-NOTES-WM2000.md:1310` calls it a
"manual producer recipe."

For a sibling title the target is the same architectural artifact (the exception
vector at `0x80000180`), so the PCs are likely to transfer nearly unchanged. But
`retired_instructions: 262016` — the point at which the guest has written the
vector — **is** title-specific and must be found empirically. **Unknown:** whether
No Mercy needs exactly one image group or more. Settled by running the boot lane
and reading what it demands.

**No executable-image group exists for any title but WM2000.**

### 1.8 Input schedule and byte-identity — genuinely bespoke

`recomps/wm2000/reference/wm2000-routes/entrance-to-match.schedule` (124 lines) and
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
`crates/fn64-cpu-runtime/src/` and **every one is a test name or a comment** — zero
title-specific runtime logic. `crates/fn64-audio/src/rsp/` is a complete MIT-clean
RSP audio stack (verified end-to-end for OoT, per `recomps/wm2000/docs/WM2000-AUDIO-STATUS.md`).

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
references in `recomps/wm2000/packages/wm2000-block-shards/build.rs`, **8 are comments or test
names, 1 is an error message, and 5 are Cargo package-name prefixes**
(`wm2000-block-shard-`, `-resident-tail-shard-`, `-overlay-` at `:324`, `:330`,
`:346`, `:666`, `:669`, `:677`). None encodes topology. Line 616 states the
generator already knows both splits: *"the index the split falls in is
per-title (WM2000: 14, No Mercy: 13)"*.

**The binding is in the boot harness, and it is a hardcoded path, not a
constant.** `generated_runner_build/mod.rs:142-143`:

```rust
const SHARD_INVENTORY: &[(&str, &str)] =
    &include!("../../../../recomps/wm2000/packages/wm2000-block-shards/shard_inventory.in");
```

`SHARD_COUNT`, `PREPARED_PACKAGES` and `SHARD_MANIFEST_DIRS` are all derived
from that one `include!` in const context, so they follow automatically — they
are **not** independent per-title constants. The 37-entry inventory file is the
single source of truth and already describes itself that way.

Alongside it, `generated_runner_build/build.rs` hardcodes the same directory
**six times** (`:867`, `:874`, `:876`, `:882`, `:888` and `shard_root`).

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
as failing because `fn64-cpu-runtime-codegen/src/emit.rs` is "missing from disk
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

## Generated No Mercy's shard topology — it works, with two caveats (2026-08-08)

    $ python3 recomps/wm2000/scripts/generate-wm-shard-topology.py \
        --rom $FN64_DISCOVER_NW4E_ROM --output-root <scratch>
    generated_packages=38

**The derived topology is genuinely No Mercy's, not WM2000's:**

| | overlays | shape | inventory entries |
|---|---:|---|---:|
| WM2000 (committed) | 4 | — | 37 |
| No Mercy (generated) | **5** | **[2, 2, 5, 7, 5]** | **42** |

So `package_inventory()` really is title-generic, and combined with the
selector from `50d2c21` the build can now express this title. That is the
mechanical half of a second title, done.

### Caveat 1 — the generator hardcodes WM2000 in its OUTPUT PATHS

It wrote to `<root>/recomps/wm2000/packages/wm2000-block-shards/` and
`<root>/recomps/wm2000/packages/wm2000-block-boot/`, i.e. the *topology* is per-title but the
*directory and package names* are not. The selector added in `50d2c21` selects
by directory name, so those must differ before two titles can coexist.

**This is the remaining piece of the same generalization** — small, and now
precisely located: the generator's path construction, not its topology
derivation.

### Caveat 2 — the shape does NOT match the recorded figures

`corpus-certification-frontier.md` records No Mercy as `[3, 3, 5, 8, 5]`,
total 24. The generator produced `[2, 2, 5, 7, 5]`, 42 inventory entries and
38 packages. **Both cannot be right.**

Not investigated here. Candidates: the recorded figures predate a change to
tiling or shard sizing; they were measured on the Rev A ROM rather than the
on-disk Unlocked variant (the same variant question that just resolved for
host bindings); or "shards per overlay" and "inventory entries" count
different things — note 42 entries against 38 packages means the inventory
carries rows that are not overlay shards.

**Whoever brings up No Mercy must reconcile this before trusting either
number.** Recording it rather than picking the one that suits: a figure in a
doc and a figure from a generator disagreeing is exactly the shape that cost
this project a day, and the generator's output is at least reproducible from a
named ROM.

## The topology discrepancy, narrowed (2026-08-09)

Recounted both sides with the *record's* counting method — overlay shards only,
not total inventory entries. My earlier "42 entries vs 24" compared different
quantities and was not a like-for-like disagreement.

**Validated the method against ground truth first.** The record says WM2000 is
`[3, 1, 6, 8]` total 18, and the committed `shard_inventory.in` counted that way
gives exactly `[3, 1, 6, 8]`. **So the record's counting is correct and the
method reproduces it.**

Applying the same count to the generated No Mercy tree:

| | overlay-0 | 1 | 2 | 3 | 4 | total |
|---|---:|---:|---:|---:|---:|---:|
| recorded | 3 | 3 | 5 | 8 | 5 | **24** |
| generated | 2 | 2 | 5 | 7 | 5 | **21** |
| delta | −1 | −1 | 0 | −1 | 0 | **−3** |

**Every overlay differs by exactly 0 or 1, never more.** That is the signature
of an off-by-one in tiling or a boundary/rounding difference — not two different
analyses of the ROM, and not the wrong ROM. Three overlays lost one shard each;
two are unchanged.

**Still unresolved, and now cheap to resolve.** Candidates, in the order I would
check them:

1. **A tiling change landed between the record (2026-08-07) and now.** The
   shard-count derivation at `recomps/wm2000/packages/wm2000-block-shards/build.rs:312` is
   per-generation; a change to `SHARD_BYTES` or to `div_ceil` boundary handling
   would move exactly the overlays whose extent is near a shard boundary and
   leave the others alone — which is the pattern observed.
2. **ROM variant.** The record predates the Unlocked-vs-Rev-A finding, and the
   host-binding result was filed under a title string rather than a digest.
   **But note WM2000 reproduces exactly**, so the deriver is not generally
   drifting — this would have to be a No-Mercy-specific extent difference.
3. **The record was computed by a different code path** than
   `package_inventory()`.

**The decisive test is cheap:** re-derive WM2000's overlays from the *current*
generator and compare against the committed inventory. If current-WM2000 also
comes out one-short on some overlays, hypothesis 1 is confirmed and the
*record* is stale, not the generator. I did not run it — the generated WM2000
tree I diffed earlier was byte-identical to committed, which already argues
against hypothesis 1, so hypothesis 2 or 3 is more likely. **Stated as
unresolved rather than concluded.**

### RESOLVED by the decisive test: the generator is correct, the record is suspect

Ran the test named above — re-derived WM2000 from the **current** generator:

    overlay-0: 3   overlay-1: 1   overlay-2: 6   overlay-3: 8   total 18

**Exactly the recorded and committed WM2000 shape.** So the generator reproduces
a known-good title bit for bit while producing `[2,2,5,7,5]`=21 for No Mercy
against a recorded `[3,3,5,8,5]`=24.

**Hypothesis 1 (a tiling change landed since the record) is eliminated.** A
changed shard boundary would have moved WM2000's overlays too, and it did not.

That leaves the No-Mercy-specific explanations, and the record is now the weaker
side of the disagreement:

- **ROM variant.** The record predates the Rev-A-vs-Unlocked finding. Different
  overlay *extents* between revisions would produce exactly this: three overlays
  crossing a shard boundary in one image and not the other, two unaffected.
  **This is the leading explanation** — it is the only one consistent with
  WM2000 reproducing exactly, and the variant question was already live for host
  bindings on this same title.
- **The record came from a different code path** than `package_inventory()`.

**Practical consequence: trust the generator, not the record.** The generated
figure is reproducible from a named ROM on demand; the recorded one is a
2026-08-07 note whose ROM is not identified by digest. Anyone bringing up No
Mercy should generate the topology rather than transcribe `[3,3,5,8,5]`, and
should expect 21 overlay shards.

Recording the digest discipline again because it would have prevented this
entirely: **a per-title figure without its ROM digest cannot be reconciled
later.**

## The two WCW titles are NOT the same problem (2026-08-09)

`corpus-certification-frontier.md:1810-1814` records both as "FAIL — 0
candidates" and bounds them out of the lane together. A per-symbol probe —
each recognizer run independently over the same window, so nothing is hidden
by the `?` short-circuit in `discover_wm_block_runtime_host_bindings` — shows
they fail for different reasons and to wildly different degrees.

| symbol | WM2000 | NoMercy | VPW2 | Revenge | WorldTour |
|---|:-:|:-:|:-:|:-:|:-:|
| osCreateMesgQueue | 1 | 1 | 1 | 1 | **0** |
| osEPiStartDma | 1 | 1 | 1 | 1 | **0** |
| osGetThreadPri | 1 | 1 | 1 | 1 | 1 |
| osSendMesg | 1 | 1 | 1 | 1 | **0** |
| osSetEventMesg | 1 | 1 | 1 | **0** | **0** |
| osSetThreadPri | 1 | 1 | 1 | 1 | **0** |
| osStartThread | 1 | 1 | 1 | 1 | **0** |
| __osSiDeviceBusy | 1 | 1 | 1 | 1 | **0** |
| osSetTimer | 1 | 1 | 1 | **0** | **0** |
| **subtotal** | 9/9 | 9/9 | 9/9 | **7/9** | **1/9** |

Out of **9**, not 15 — these are the directly-callable predicates; the other
six (osCreateThread, osRecvMesg, four RSP task roles) resolve through
multi-stage call-chain logic not yet expressible as standalone checks. Labelled
honestly rather than inflated to 15.

**Revenge: 7/9, two genuine behavioural gaps** — `osSetEventMesg` and
`osSetTimer`, both truly absent rather than shadowed. The original probe named
only the first because the chain short-circuits. **Plausibly two widenings.**

**World Tour: 1/9, and probably not a recognizer problem.** Eight independent
behavioural recognizers do not all miss for eight coincidental revision
differences. The surviving one, `osGetThreadPri`, is a 6-word predicate — the
most likely to match incidentally. **The leading hypothesis is scan coverage:**
the probe hard-codes a 1 MB window at file offset `0x1000`, VA `0x80000400`,
and if World Tour's resident libultra lies outside it, every recognizer misses
for one reason rather than eight.

Note ROM size does not separate them: World Tour is 12 MB, Revenge 17 MB,
WM2000 32 MB. **The 12 MB title scores 1/9 and the 17 MB title 7/9**, so this
is layout, not size.

### Two corrections to the record, and one to me

**The record's "0 candidates, bounded out of the lane" is true of one title
and misleading for the other.** Revenge resolves 7 of 9; calling it bounded out
alongside World Tour hid a tractable case behind an intractable-looking one.

**And my own "two single-symbol gaps, one each" was wrong** — I read the `15`
on the FAIL line as a score. It is `WM_BLOCK_RUNTIME_HOST_SYMBOLS.len()`,
printed unconditionally on the error path (`probe_host_bindings.rs:30`), with
no numerator. **A denominator printed without its numerator invites exactly
this misreading**; the per-symbol probe prints both on both paths.

## The title-parameterized generator works end to end (2026-08-09)

First use of `--title` (`fb51ab5`) against a non-WM2000 ROM:

    $ python3 recomps/wm2000/scripts/generate-wm-shard-topology.py \
        --rom "…/WCW-nWo Revenge - Starrcade Edition (USA) (v1.01).z64" \
        --title revenge-block-shards --output-root <scratch>
    generated_packages=27  title=revenge-block-shards

**Output is fully title-specific** — directories `revenge-block-shards/` and
`revenge-block-boot/`, packages `revenge-block-shard-NN`,
`revenge-block-resident-tail-shard-NN`, `revenge-block-overlay-N-shard-NN`.
Nothing named `wm2000` anywhere in the tree.

**And the topology is genuinely Revenge's**, not WM2000's copied under a new
name:

| | overlays | shape | inventory entries | banks |
|---|---:|---|---:|---:|
| WM2000 (committed) | 4 | [3, 1, 6, 8] | 37 | 5 |
| Revenge (generated) | **2** | **[4, 6]** | **31** | **3** |

Two overlays against four, and the 3-bank structure matches what
`gate_rom_recompile` independently reported for this ROM. The deriver is
reading the image, not templating.

### What this closes

**Every mechanical blocker between a certified ROM and a shard tree is now
gone.** The chain that was three separate walls this morning —

1. `PREPARED_PACKAGES` a fixed array → selectable by directory (`50d2c21`)
2. generator hardcoding WM2000 in output paths and package names → `--title`
   (`fb51ab5`)
3. Revenge rejected at discovery → 15/15 and `unsupported = 0` (`10a73c7`)

— is walkable end to end for two titles, Revenge and No Mercy.

**What remains is not mechanical.** A generated tree is not a booting game: it
still needs a boot context captured against the specific ROM (one command,
~1 min), the executable-image PCs located (a human hunt, no prior art outside
WM2000), and an input schedule authored (bespoke, the acknowledged long pole).
Those three are what separate "recompiles" from "plays", and no tooling landed
tonight shortens the last two.

## Revenge boot context captured — and a fourth ROM-variant collision

    $ scripts/capture-boot-context.zsh "…/WCW-nWo Revenge - Starrcade Edition (USA) (v1.01).z64"
    → captures/WCW-nWo-Revenge---Starrcade-Edition--USA---v1-01--boot-context.json
      normalized_rom_sha256  d8c097f8880032fc…   (matches the certified ROM)
      entry_pc               0x80000400

**Three of Revenge's four bring-up items are now done:** certified
(`unsupported = 0`), shard tree generated with a title-specific prefix, boot
context captured against the same image.

### There were already three different Revenge images in play, and none matched

A capture for Revenge **already existed** — but it binds `66c137d3…`, and the
two ROMs on disk are `d8c097f8…` (Starrcade v1.01, certified above) and
`fd9996c7…` (the donor `grade-all.sh` defaults to). **Three distinct images,
no two the same.**

Had the existing capture been reused on the strength of its filename saying
"Revenge", it would have bound register state the certified ROM never
produced — and the script's own header explains why that is worse than having
none: a hand-made or mismatched context *"would pass schema validation while
binding register state the hardware never produced — forging the very
authority under audit."*

**This is the fourth ROM-variant collision this session** (host bindings,
No Mercy topology, VPW2 block count, now the boot context). Every one had the
same cause: **an artifact filed under a title string rather than a ROM
digest.** The capture written above records its digest; the recognizer results
and the recompile receipts now do too.

### What is actually left for Revenge

Only the two human-gated items: **locating the executable-image PCs** (no
prior art outside WM2000) and **authoring an input schedule** (bespoke). Every
mechanical prerequisite is satisfied.

## Revenge executable-image group captured — the "human PC hunt" was a script

    status            validated
    capture_count     3            (three agreeing producer runs)
    image_id          general-exception-preamble
    capture_pc        0x80000180
    byte_len          16
    image_sha256      64bbe2d15fedd71b9f0926df5435fcbf3df504d6a2f8070e24af1bdd6de74845
    authority_sha256  dfe2dcebdb11232e9842648361f56cd8d1d2be923bbe6819a3e55012aa9a7d08

**Revenge's image digest `64bbe2d1…` differs from WM2000's `92d005d9…`**, so
this is Revenge's own exception preamble, captured — not inherited.

### The scoping doc was wrong twice about what needs a human

It called this *"a human PC hunt with no prior art outside WM2000."* It is
neither:

- **The address is architectural.** `source_closure/mod.rs:18` defines
  `MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1` as a compiled-in `[u32; 6]`;
  the discovery path iterates that fixed list asking who owns each. **No scan
  for unknown PCs exists anywhere in it.**
- **The producer is headless.** `tools/mupen-trace/mupen_trace.c` single-steps
  the public `m64p_debugger` API to the target PC, reads N words and exits,
  with no input and no video plugin attached.
- **The wrapper is title-agnostic.** `recomps/wm2000/scripts/capture-wm-executable-image-group.zsh`
  contains **zero** `wm2000` literals; only its filename says "wm". Every
  ROM-specific value is a flag.

Same doc had already called the boot-context capture human-gated, and that
turned out to be one command. **Two "needs a person" claims, both wrong, both
believed until someone ran the thing.**

### Two operational traps hit while doing it

**A reported-present prerequisite was absent.** The RSP plugin was reported at
`fn64-rsp-hle-build/mupen64plus-rsp-hle/projects/unix/…`; it actually lives at
`fn64-rsp-hle-build/src/projects/unix/…`. The script's own path validation
caught it — *"every input must be an absolute regular non-symlink file"* — and
cost one restart rather than a bad capture. **Validate paths at the boundary,
not by reporting them.**

**`/private/tmp/fn64-wm-trace-build/` had been reaped empty by macOS**, which
is exactly the hazard `capture-boot-context.zsh`'s header documents, and why
the producer is rebuilt from source rather than trusted as a leftover binary.
Any flow assuming a `/tmp` binary survives will fail this way.

### Revenge's bring-up status

| item | state |
|---|---|
| host bindings | **15/15** |
| CPU recompile | **`unsupported = 0` of 1,749** |
| shard tree | **generated**, `revenge-block-*` |
| boot context | **captured**, binds `d8c097f8…` |
| executable images | **validated**, `64bbe2d1…` |
| input schedule | **outstanding — genuinely bespoke** |

**Five of six done, and the sixth is the only one that really requires a
person**, because an input schedule is a description of gameplay rather than a
property of the ROM.

## The input schedule IS genuinely bespoke — checked, not assumed

Having been wrong twice about what needs a person, I checked this one rather
than asserting it. **It holds.**

`recomps/wm2000/reference/wm2000-routes/entrance-to-match.schedule` (124 lines) is a route
through *this game's menus* — boot → Exhibition → Single Match → in-match
gameplay — and its own header states the method:

> Every screen named in the comments below was **read off a dumped frame**, not
> inferred from what a wrestling menu "probably" does — the representative
> frames are committed alongside in `recomps/wm2000/reference/wm2000-frames/` and each section
> cites the one that evidences it.

**That cannot be derived from the ROM.** It requires running the game, looking
at frames, identifying which screen each is, and deciding what a successful
session looks like. There is no fixed constant to read, no architectural
address, no title-agnostic script — the three things that made the previous
two items mechanical.

**So the boundary between tooling and human work is now established rather
than assumed:**

| item | mechanical? | evidence |
|---|---|---|
| host bindings | **yes** | signature-scanned, zero addresses |
| CPU recompile | **yes** | one env var, no per-game config |
| shard tree | **yes** | `--title`, topology derived from the image |
| boot context | **yes** | one command, ~1 min, headless |
| executable images | **yes** | architectural PC, headless producer |
| **input schedule** | **NO** | a description of gameplay, read off frames |

Five of six are tooling. The sixth is a person watching a game.

**A second caution from that header, worth repeating because it already cost
this project a result:** the 420,000-step route that first reached WM2000's
match-setup screen was written to `/tmp`, macOS reaped it, and *"that run is
now unreproducible — it survived only as prose."* **A route recipe is evidence
and belongs in the repository**, which is why these two schedules are
committed. The same reaping ate the trace-producer build directory tonight.

## No Mercy bring-up: five of six, and Revenge's cascade does NOT recur here

WWF No Mercy (USA), normalized ROM
`11640379fdf534b39f34678036ad8e4cdc9b80b4f2cc72411433363372123976` (no test
here checks it -- the harness that did moved to recomps/wm2000 with the game
code, so this is a recorded observation, not a gated claim), the Unlocked
variant, 2026-08-09. Every artifact below records that digest.

| item | state |
|---|---|
| host bindings | **15/15** on this image |
| CPU recompile | **`unsupported = 0` of 57,284 blocks / 6 banks** |
| shard tree | **generated**, `nomercy-block-*`, 38 packages |
| boot context | **captured**, binds `11640379…`, `entry_pc = 0x80000400` |
| executable images | **validated**, `25b1ec80…`, 3 agreeing runs |
| input schedule | **not needed for boot** — `FN64_CONTROLLER_SCHEDULE` is an `Option` |

### The filed boot context was the wrong image, exactly as suspected

`captures/WWF-No-Mercy--USA---Rev-A--boot-context.json` binds
**`fc561fce443010b1…`**, not the ROM on disk. Its filename says "Rev A"; the
certified image is the Unlocked variant. Re-captured to
`captures/nomercy-11640379-boot-context.json`.

**That is the fifth ROM-variant collision** in this line of work (host
bindings, No Mercy topology, VPW2 block count, Revenge's boot context, now No
Mercy's), and the cause has been identical every time: **an artifact filed
under a title string rather than a ROM digest.** The new file is named for the
digest.

### Executable images: No Mercy's own, not inherited

    status            validated
    capture_count     3
    image_id          general-exception-preamble
    capture_pc        0x80000180
    byte_len          16
    image_sha256      25b1ec80d013b3c0f94f807b38d72a5f75a2dc81cb30ae5cecb903c4ec0ef690
    authority_sha256  8a793e726804f579dec3fd579a8d2e7462764bee1e5f3a5f95e5de59a8a5acd8

Three distinct preambles now measured — WM2000 `92d005d9…`, Revenge
`64bbe2d1…`, No Mercy `25b1ec80…` — so the image is genuinely per-title and
copying one across titles would bind the wrong bytes.

### The topology, and the count the older note got wrong

    generated_packages=38  title=nomercy-block-shards

**5 overlays, shape [2, 2, 5, 7, 5]** = 21 overlay shards, plus 14 boot-prefix
and 3 resident-tail shards. The overlay shape matches the 2026-08-09 note; its
**"42 inventory entries" does not — the generator emits 38.** Trust the
generator, and prefer counting its output to quoting a remembered figure.

| | overlays | shape | inventory entries | banks |
|---|---:|---|---:|---:|
| WM2000 (committed) | 4 | [3, 1, 6, 8] | 37 | 5 |
| Revenge (committed) | 2 | [4, 6] | 31 | 3 |
| **No Mercy (generated)** | **5** | **[2, 2, 5, 7, 5]** | **38** | **6** |

### The resident-tail clamp is a NO-OP on this ROM, measured before building

Revenge's first boot cost four cascading failures because its overlay
invalidation union stops 21,600 bytes short of the boot copy's end, and both
the shard generator and `build.rs` assumed it never would. **The obvious move
was to assume No Mercy is Revenge-shaped. It is not, and assuming so would
have been the same error in the other direction.**

Measured from the library's own recipe recovery
(`admitted_overlay_load_recipes_v1`) rather than inferred:

    split (min load_start)   0x800d9960
    invalidation union end   0x8016da30
    boot copy end            ~0x80100400   (fixed 1 MiB IPL3 DMA)

**The union runs well PAST the bank end**, which is WM2000's geometry, not
Revenge's — so `tail_image_end = load_end.min(union_end)` selects the bank end
and the clamp changes nothing.

Confirmed a second way, because a single derivation of a geometry fact is what
let the off-by-one shard survive in the first place: running **both** the
unclamped generator (`recomps/wm2000/packages/wm2000-block-shards/build.rs`) and the clamped
one (`recomps/wm2000/packages/revenge-block-shards/build.rs`) against this ROM yields
**byte-identical 38-package inventories**. Two implementations that disagree
on Revenge agree here, which is the direct evidence that the clamp is inactive.

The clamped copy was taken as No Mercy's `build.rs` source anyway. It is a
strict generalization — it reduces to the unclamped rule exactly when the union
reaches the bank end — so it costs nothing today and removes a class of
surprise if this title's geometry is ever re-derived.

### The two generator traps still bite, and both were reapplied

Neither is title-specific and neither is fixed upstream, so a third title will
hit them again:

1. **The boot `Cargo.toml` is templated from WM2000's verbatim**, so it names
   a `wm2000-shell` `[[bin]]` whose source does not exist, carries a
   window/input dependency stack nothing links, and puts the
   `[profile.release.package.*]` override on a package absent from the graph —
   where Cargo silently ignores it. All three reapplied as Revenge's committed
   manifest documents.
2. **The generator emits no `build.rs`**, so `PACKAGE_PREFIX` must be
   retargeted by hand or a leaf crate reaches `package_target` with the wrong
   prefix and panics.

`write_topology` rewrites the boot manifest from the WM2000 template on **every
regeneration** and drops edit 1 each time.

## No Mercy first boot: T3 — runs 150k+ steps, then a FlashRAM store fn64 itself pointed at

Built and booted 2026-08-09, ROM `11640379…`, `FN64_RENDER=reference`,
headless, **no controller schedule** (`FN64_CONTROLLER_SCHEDULE` is an
`Option`). Binary `recomps/wm2000/packages/nomercy-block-boot`, canonical program artifact
`9f5ff066916d21df1debe89fc8e78488dacb15596cead357ad08cb851a9bc06c` (no test owns it; rebuilding needs the ROM).

| tier | result |
|---|---|
| **T1** workspace builds | **reached** — 38 shards, `Finished release in 5m 22s` |
| **T2** boots / first entry | **reached** — *"first-entry BootContext matches exactly"* |
| **T3** runs without faulting | **reached then lost** — 150,000+ steps, overlay entry at 92,061 |
| **T4** non-uniform frames | **not reached** — `gfx_submits=0` throughout |

The guest is genuinely executing, not spinning: `audio_submits` climbs
monotonically 7 → 14 across the run, `idle_steps=0` at every heartbeat, and it
enters its first overlay generation `[0x800d9960,0x800f2940)` at step 92,061.

### The failure, which is fn64's and not the title's

    canonical catalog stopped on non-architectural guest fault:
    CpuFault { at: ExecutionKey { bank: BankId(6054620885710679290),
                                  pc: GuestPc(0x8003d518) },
               kind: MemoryFault { addr: 0xffffffffa8010000 } }

**Deterministic** — identical PC, fault address, step (92,061) and `sim_time`
across two runs.

`0xA8010000` masks to physical `0x0801_0000`, inside **`PI_DOM2_ADDR2`
(`0x0800_0000..=0x0fff_ffff`) — the SRAM/FlashRAM save-media window**
(`pi/mmio.rs:167`).

Disassembling the trapping instruction identifies it as a save-media register
write helper, a five-instruction leaf:

```text
0x8003d508  8c82000c   lw   $2, 0xc($4)     load a base out of a struct
0x8003d50c  3c03a000   lui  $3, 0xa000      KSEG1 uncached mask
0x8003d510  00451025   or   $2, $2, $5      add the offset
0x8003d514  00431025   or   $2, $2, $3      force uncached
0x8003d518  ac460000   sw   $6, 0x0($2)     <-- THE TRAPPING STORE
0x8003d51c  03e00008   jr   $ra
```

**The base it loads at offset `0xc` is one fn64 wrote itself.**
`save.rs:451` — inside `osFlashInit_recomp` — does
`storage.write_u32(base + 12, FLASH_KSEG1_BASE)` with
`FLASH_KSEG1_BASE = 0xA800_0000` (`save.rs:39`). So the ABI hands the guest an
`OSPiHandle` containing that pointer, the guest reads field `0xc` back and
stores through it, and **the memory path then refuses the store.** The two
halves of fn64 disagree about whether that address is addressable.

The mechanism is a missing arm, not a wrong value: `write_raw_mmio_word`
(`pi/timing.rs:169`) handles PIF RAM and live device MMIO and has **no
save-media window case at all**, so the store returns `false`, is not backed
RDRAM, and surfaces as an anonymous unbacked-memory fault. Its read sibling has
the same gap in the other direction — `cartridge_rom_window_offset`
(`timing.rs:35`) accepts only `PI_DOM1_ADDR2`, the cartridge ROM.

**This is a recurrence of a defect class the file already documents.**
`timing.rs:44-52` records exactly this shape for the N64DD window: a region
reachable by PI *DMA* but not by *programmed CPU access*, surfacing as
`MemoryFault { addr: 0xffffffffa6000000 }` — *"a message that says nothing
about which device is missing."* Same sentence applies here with `a8010000`.

### Why this was not fixed in the same session, deliberately

The save window differs from the N64DD case in a way that matters: the N64DD
fix could encode "no ASIC answered" as zero because *no control flow depends on
the value*. FlashRAM is a **command sequencer** — stores into its window are
commands (erase, page-write, status-read) whose ordering and status effects the
guest does branch on, and `save.rs` already models that sequencer for the shim
path. Wiring the CPU-store path to it is a real ABI change needing its own
tests, and **guessing the semantics would fabricate hardware behaviour** — the
error `timing.rs:56-63` names when it refuses to invent an open-bus value.

**What the next session needs** is a `PI_DOM2_ADDR2` arm in
`write_raw_mmio_word`/`read_raw_mmio_word` routed to the existing
`crate::save` FlashRAM state, plus the decision of whether this title uses
FlashRAM or SRAM at that base. The fault names the exact call site, so this is
scoped rather than exploratory.

### One trap worth recording, because it cost the Revenge re-run a result

**Without `FN64_BLOCK_CONTINUE_AFTER_OVERLAY` set, the boot binary breaks at
first overlay entry and exits 0** (`main.rs:1259`), having rendered nothing.
`render-benchmark.zsh:184` exports it, so the scripted lane never sees this and
a hand-rolled invocation does. The failure presents as **rc=0 with a
progress-shaped log line** — nothing says "stopped early on purpose". The
check that distinguishes a real run is `gfx_submits`, never the exit code.

### A filename that will lie again

The reference backend's dump prefix is the hardcoded literal
`"fn64-wm2000-block"` (`main.rs:827`) for **every** title, so No Mercy's PNGs
would land named `fn64-wm2000-block-NNNN.png`. Given that title-string filing
has now caused five ROM-variant collisions in this session, any frame committed
from a non-WM2000 title must be renamed and filed under its ROM digest.
