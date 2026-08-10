# Per-title dense-AOT shard package generation

Scoping note, not an implementation. Question asked: what does it take to
generate the dense-AOT shard crate set for a NON-WM2000 AKI title, and is the
existing generator already ROM-parameterized?

**Verdict up front: the *emitter* is title-generic; the *package inventory* is
not.** The code that turns ROM bytes into a shard's Rust source derives every
geometry it needs from discovery and would work on No Mercy unchanged. What
blocks a second title is that the *list* of shard packages is four hand-written
`const` arrays plus 35 committed Cargo crate directories, all naming WM2000.
Two of those four arrays are also **already stale and mutually inconsistent**
(one of them does not compile), which is a pre-existing bug this scoping found
and which must be fixed before any per-title work begins.

---

## 1. The generator's actual input contract

### The one real generator

`examples/wm2000-block-shards/build.rs` is the generator. It is a single file
compiled in two roles (`build.rs:543-546`): Cargo runs `main()` as the legacy
per-crate build script, and `examples/wm2000-block-shards/producer.rs:6-8`
`#[path]`-includes the same file as an inert module so the one-shot producer can
call it 35 times in one process.

`WmShardGenerator` (`build.rs:149-153`) holds exactly three fields:

```rust
pub struct WmShardGenerator {
    rom: fn64_discover::rom::NormalizedRom,
    overlay_recipes: Option<Vec<OverlayLoadRecipeV1>>,
    host_calls: Vec<u32>,
}
```

All three come from ROM bytes alone. `from_rom_bytes` (`build.rs:156-177`)
normalizes the ROM, then discovers host bindings from the resident 1 MiB
signature. `overlay_recipes` (`build.rs:460-472`) runs
`recover_overlay_regions` + `admitted_overlay_load_recipes_v1` with
`SearchConfig::aki_family()` — no per-title table, no pack file, no capture.

**Input contract, precisely:**

| Input | Source | file:line |
|---|---|---|
| ROM bytes | `ROM` env var (legacy) / `--rom` (producer) | `build.rs:551`, `producer.rs:82` |
| Package name | `CARGO_PKG_NAME` (legacy) / `PACKAGES` loop (producer) | `build.rs:550`, `producer.rs:51` |
| Output dir | `OUT_DIR` / `--output` | `build.rs:556`, `producer.rs:83` |
| 4 provenance digests | `--generator/discovery/emitter/runtime-source-sha256` | `producer.rs:84-103` |

There is **no `FN64_DISCOVER_ROM`, no BootContext, and no pack file** in the
generator. It is a pure ROM → Rust function. It never executes guest code
(`producer.rs:3-4`).

### The WM2000-specific constants that *are* present

Only four, all in the resident-bank path — the overlay path has none:

- `ROM_START = 0x1000`, `BOOT_BYTES = 0x10_0000`, `VA_START = 0x8000_0400`
  (`build.rs:15-17`) — these are IPL3-generic across the affine-boot-bank
  family, not WM2000-specific.
- `PackageTarget::Boot(index @ 0..=13)` (`build.rs:364`) — hardcodes that boot
  shards 0..13 are full-width. **Title-specific.**
- `PackageTarget::Boot(14)` + its assertion (`build.rs:374-380`) — hardcodes
  that the boot/overlay split falls in shard 14. **Title-specific.**
- `assert_eq!(..., 2, "resident-tail package topology must cover exactly two
  shards")` (`build.rs:402-406`) — **title-specific, and No Mercy violates it
  (see §4).**
- `package_target` (`build.rs:475-498`) parses the literal prefix
  `"wm2000-block-..."`. **Title-specific, but purely cosmetic string parsing.**

Everything else — shard count, VA, bank ID, digests — is derived:
`shard_count = image_byte_len.div_ceil(SHARD_BYTES)` (`build.rs:313`).

---

## 2. What "shard layout `[3,3,5,8,5]`" means

The notation comes from `docs/plans/corpus-certification-frontier.md:1828-1830`:

```
WM2000   overlays=4  shards per overlay [3, 1, 6, 8]  total 18
NoMercy  overlays=5  shards per overlay [3, 3, 5, 8, 5] total 24
VPW2     overlays=4  shards per overlay [3, 2, 6, 8]  total 19
```

It is a per-overlay-generation count of 64 KiB dense shards. One entry per
recovered overlay recipe; the value is
`generation_source_span(recipe).div_ceil(65536)`, i.e. `build.rs:313` with
`SHARD_BYTES = 64 * 1024` (`build.rs:18`).

### These recorded numbers are STALE

They predate commit `6ae673e` ("bound overlay generations to their text
extent"), which changed generations from whole-loaded-image to text-bounded and
took WM2000 from `[3,1,6,8]`=18 to `[2,1,5,7]`=15. I re-derived the current
values by calling `generation_source_span` directly against both ROMs:

```
WM2000    overlays=4  layout [2, 1, 5, 7]     overlay total 15   (matches the tree: 32 pkgs)
NoMercy   overlays=5  layout [2, 2, 5, 7, 5]  overlay total 21
```

So No Mercy is **`[2,2,5,7,5]` = 21 overlay shards**, not `[3,3,5,8,5]` = 24.
`docs/plans/corpus-certification-frontier.md:1828-1830` should be corrected.

Measured No Mercy geometry (`nomercy.z64`,
sha256 `11640379fdf534b3…`):

```
overlay 0: rom 0x57310..0x81210   load 0x800d9960  text_end 0x800f2940  span 0x20000  shards 2
overlay 1: rom 0x81210..0xae390   load 0x80106760  text_end 0x8011ebd0  span 0x20000  shards 2
overlay 2: rom 0xae390..0xfd250   load 0x80106760  text_end 0x80147ee0  span 0x41780  shards 5
overlay 3: rom 0x144be0..0x1bd150 load 0x800d9960  text_end 0x80144670  span 0x70000  shards 7
overlay 4: rom 0xfd250..0x144be0  load 0x80106760  text_end 0x801470b0  span 0x40950  shards 5
first_overlay_load_start = 0x800d9960
boot prefix   0xd9560 -> 14 boot shards       (WM2000: 0xe1790 -> 15)
resident tail 0x26aa0 ->  3 tail shards       (WM2000: 0x1e870 ->  2)
```

**Total No Mercy package count: 14 + 3 + 21 = 38** (WM2000 today: 15 + 2 + 15 = 32).

---

## 3. 64 KiB tiling and mid-shard generations — already honored

`generation_source_span` (`crates/fn64-discover/src/overlay_recipe.rs:63-72`) is
the single source of truth:

```rust
pub fn generation_source_span(recipe: &OverlayLoadRecipeV1) -> u32 {
    let text_len = recipe.text_end - recipe.load_start;
    let rounded = text_len.div_ceil(DENSE_SHARD_BYTES) * DENSE_SHARD_BYTES;
    let image_len = recipe.rom_end - recipe.rom_start;
    if rounded <= image_len { rounded } else { text_len }
}
```

**The generator already honors it.** `build.rs:435-437` computes the overlay
`source_end` as `recipe.rom_start + generation_source_span(recipe)` and nothing
else, with an explicit comment (`build.rs:429-434`) that this is derived from the
one shared helper "so these shard extents match the ones the pack emits."

Every other consumer derives from the same function:
`dense_aot_pack.rs:87-91`, `generation_topology/mod.rs:351-354`,
`wm2000-block-boot/build.rs:743,1058,1116,1133`.

**Per-title generation will NOT hit the bug that was just fixed for WM2000.**
The fix in `0eafd7f` was structural (collapsing five independent extent
decisions into one), not a WM2000 special case. The fallback branch — exact
text length when the image is shorter than one shard — is exercised by WM2000
overlay 1 (`span 0x5df0` inside a `0xd640` image). No Mercy has no such overlay
(smallest span is `0x20000`), so it exercises only the common rounded path,
which is the better-tested branch. This item is **clear**.

---

## 4. Cost for one additional title (No Mercy, NW4E)

### 4a. The actual blocker: the package inventory is four hand-written arrays

`PREPARED_PACKAGES` / `PACKAGES` are duplicated in four places. They must agree
because `canonical_definition_sha256` folds every shard extent into a digest
checked across all of them:

| File | Declared | Actual entries |
|---|---|---|
| `examples/wm2000-block-shards/build.rs:23` | `[&str; 35]` | **35** |
| `examples/wm2000-block-shards/materializer.rs:18` | `[&str; 35]` | **32** |
| `crates/fn64-boot-harness/src/generated_runner_build/mod.rs:133` | `[&str; 35]` | **35** |
| `examples/wm2000-block-boot/build.rs:28` | `[&str; 32]` | **32** |

Plus `SHARD_MANIFEST_DIRS` (`generated_runner_build/mod.rs:170`, 35 entries) and
35 committed crate directories under `examples/wm2000-block-shards/`.

### PRE-EXISTING BUG FOUND — fix this first

Commit `6ae673e` reduced the inventory 35 → 32 (dropping
`overlay-0-shard-02`, `overlay-2-shard-05`, `overlay-3-shard-07`) but only
updated **two of the four** lists. Consequences:

1. **`materializer.rs:18` does not compile.** It declares `[&str; 35]` and
   supplies 32 initializers. Verified by compiling the file standalone:

   ```
   error[E0308]: mismatched types
     --> materializer.rs:18:34
      | expected an array with a size of 35, found one with a size of 32
   ```

   This is latent only because `materializer.rs` is reachable solely through
   `prepared_build.rs:1` (prepared mode, gated on
   `FN64_WM_PREPARED_SHARD_ROOT`) and via `#[cfg(test)]` in
   `producer.rs:9-11`. `cargo check` and `cargo check --tests` on
   `wm2000-prepared-shard-producer` both pass today — the file is simply never
   built. **The moment prepared mode is exercised, it fails to compile.**

2. `wm2000-block-shards/build.rs:23` and
   `generated_runner_build/mod.rs:133` still list all 35, so the producer would
   emit 3 surplus shards that `wm2000-block-boot/build.rs:28` does not expect.

3. 3 surplus crate directories remain on disk (`overlay0-shard02`,
   `overlay2-shard05`, `overlay3-shard07`).

**This must be resolved before per-title work starts** — otherwise a second
title is built on an inventory that is internally inconsistent by three
packages.

### 4b. Hard assertions No Mercy trips

Even with a correct package list, three constants reject No Mercy:

1. **`build.rs:402-406`** — `assert_eq!(…, 2, "resident-tail package topology
   must cover exactly two shards")`. No Mercy's resident tail is `0x26aa0` =
   **3 shards**. Hard panic.
2. **`build.rs:376-380`** — asserts the first overlay load lands in static-prefix
   shard 14, i.e. `VA_START + 14*64K < first_overlay_start <= VA_START + 15*64K`.
   No Mercy's `first_overlay_start = 0x800d9960`; `VA_START + 14*64K =
   0x800e0400`. `0x800d9960 < 0x800e0400`, so **the assertion fails** — No Mercy's
   split falls in shard **13**, not 14.
3. **`build.rs:364`** — `Boot(index @ 0..=13)` treats shards 0-13 as full-width,
   but for No Mercy shard 13 is the partial one. Wrong extents even if (2) is
   relaxed.

All three are the same defect: boot/tail topology hardcoded instead of derived
from `first_overlay_start`. The fix is mechanical — the quantity is already
computed by `first_overlay_start()` (`build.rs:452-458`).

### 4c. What is NOT derivable from the ROM

| Input | ROM-derivable? | Evidence |
|---|---|---|
| Overlay recipes, shard counts, VAs, bank IDs | **Yes** | `build.rs:460-472`, `:313`, `:331` |
| Host call bindings (15 symbols) | **Yes** — No Mercy resolves **15/15** | `probe_host_bindings` run: `nomercy OK 15/15` |
| Shard runner source + digests | **Yes** | `build.rs:264-291` |
| **BootContext** (initial COP0/GPR state) | **NO — capture only** | `scripts/capture-boot-context.zsh:4-9`: "It cannot be synthesized… a hand-written one would pass schema validation while binding register state the hardware never produced — forging the very authority under audit." |
| **Executable image groups** (≥3 captures each of CPU-written exception images) | **NO — capture only** | `wm2000-block-boot/build.rs:9-13, 358-371` |
| Controller schedules / route input | **NO — capture only** | route-run captures under `~/Code/aki-recomp/captures/` |

**Shard generation alone is NOT enough to boot a title.** It gets you the CPU
code; BootContext and exception-image captures are separate prerequisites.

**Capture status for No Mercy — a real blocker.** A boot context exists at
`~/Code/aki-recomp/captures/WWF-No-Mercy--USA---Rev-A--boot-context.json`, but it
binds `normalized_rom_sha256 = fc561fce443010b1…`, while the on-disk ROM
`~/Code/aki-recomp/games/NW4E/nomercy.z64` is `11640379fdf534b3…`. Neither
`~/Downloads/WWF No Mercy (Unlocked).z64` (same `11640379…`) nor
`~/Downloads/WWF No Mercy (E) (V1.1) [!].z64` (`381d46cf…`) matches. The
binding is enforced hard — `crates/fn64-boot-harness/src/boot_context.rs:35-38`
returns `BootContextLoadError::RomIdentityMismatch`. So **either the USA Rev A
ROM must be located, or the boot context must be re-captured against the ROM we
have.** Re-capture is the cheaper path:

```
scripts/capture-boot-context.zsh ~/Code/aki-recomp/games/NW4E/nomercy.z64
```

No executable-image group exists for No Mercy at all — only
`~/Code/aki-recomp/captures/wm-general-exception-images/` (WM2000). That needs
`scripts/capture-wm-executable-image-group.zsh`, ≥3 captures.

### 4d. Command sequence, assuming the generator is made title-generic

The generation step itself is one command (`producer.rs:66-121`):

```sh
cargo run --release -p fn64-wm-prepared-shard-producer --bin fn64-wm-prepared-shard-producer -- \
    --rom      /Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64 \
    --output   /absolute/new/private/nw4e-prepared-tree \
    --generator-source-sha256 <sha256 of examples/wm2000-block-shards/build.rs> \
    --discovery-source-sha256 <sha256 of the discovery source set> \
    --emitter-source-sha256   <sha256 of the emitter source set> \
    --runtime-source-sha256   <sha256 of the runtime source set>
```

Writes, per package, a `runner.rs` + `metadata.rs` + `identity.v1` sidecar under
`--output`, plus a root `manifest.v2`; prints
`schema= normalized_rom_sha256= prepared_manifest_sha256=`. The output tree is
**ROM-derived game content** and must stay outside git (`build.rs:3-4`,
`materializer.rs:3-6`). `--output` must not already exist.

Then the boot lane consumes it:

```sh
ROM=/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64 \
FN64_BOOT_CONTEXT=/absolute/nw4e-boot-context.json \
FN64_EXECUTABLE_IMAGE_GROUPS=NW4E_GROUP_A,NW4E_GROUP_B,NW4E_GROUP_C \
FN64_WM_PREPARED_SHARD_ROOT=/absolute/new/private/nw4e-prepared-tree \
scripts/build-wm2000-withheld-pair.zsh /absolute/new/output-dir
```

### 4e. Build/link cost — scales, and No Mercy is worse

WM2000: 32 packages → 121 MB binary
(`docs/plans/corpus-certification-frontier.md:446`). No Mercy is **38
packages, ~19% more**, so expect roughly **~145 MB** and proportionally longer
link.

Compounding factors:
- `SELECTED_BUILD_CARGO_JOBS_V5 = 2` (`generated_runner_build/mod.rs:127`) is
  folded into build-evidence digests (`build.rs:394-396`) — it is **not** a
  tunable, so more crates cannot be absorbed by more parallelism.
- Memory guard defaults to 2048 MiB
  (`scripts/memory-guard.zsh:17`); the pair script raises it to 4096
  (`build-wm2000-withheld-pair.zsh:26`) with a 3600 s ceiling
  (`FN64_GUARD_MAX_SECONDS`). A 19% larger link at `-j2` may exceed the
  one-hour timeout; budget a raise.
- `RUNNER_BYTES = 2048` (`build.rs:21`) already exists to keep rustc under a
  measured memory ceiling; 64 KiB shards are split into 32 subrunners each.
- The dev profile keeps target crates at opt-level 0 with only build scripts at
  opt-level 1 (`wm2000-block-shards/Cargo.toml`), which is the deliberate
  trade already tuned for WM2000.

---

## 5. Effort and risk

| Work item | Effort | Risk |
|---|---|---|
| **Fix the 35-vs-32 inconsistency** (4 arrays + `SHARD_MANIFEST_DIRS` + 3 stale dirs) | S | Low — mechanical, but blocks everything |
| Derive boot/tail topology from `first_overlay_start` instead of `0..=13` / `Boot(14)` / `assert_eq!(…, 2)` (`build.rs:364,374-380,402-406`) | S–M | Low — the quantity is already computed at `build.rs:452-458` |
| Generate the package list per title instead of 4 `const` arrays | **M–L** | **Medium-high** — crosses the crate boundary into `fn64-boot-harness`; `SHARD_MANIFEST_DIRS` and 38 Cargo crate dirs must be generated, and `canonical_definition_sha256` must keep agreeing across all consumers |
| Title-agnostic package naming (`package_target`, `build.rs:475-498`) | S | Low — string parsing only |
| Re-capture No Mercy BootContext against the on-disk ROM | S | Medium — needs the mupen toolchain at the paths in `capture-boot-context.zsh:31-33` |
| Capture ≥3 No Mercy executable-image groups | M | **Medium-high** — no prior art outside WM2000 |
| First full No Mercy build + link | M | Medium — ~145 MB at `-j2`; may exceed the 3600 s guard |

**Critical path is the package-list generation (M–L).** It is the item
`corpus-certification-frontier.md:1837-1845` already identified as "a
build-system change, not a constant to relax," and this scoping confirms that
assessment while finding it is worse than recorded: it is **six** places (four
arrays, `SHARD_MANIFEST_DIRS`, and the committed crate directories), and three
of them are currently out of sync.

## Verdict

**The generator is title-generic in its emission core and title-specific in its
inventory and resident-bank topology.**

- ROM → shard source: **already generic.** Driven purely by `--rom`/`ROM`; no
  BootContext, no pack file. No Mercy passes host bindings 15/15.
- 64 KiB tiling / mid-shard generations: **already correct.**
  `generation_source_span` is the single source of truth and the generator
  derives from it (`build.rs:435-437`). No repeat of the WM2000 bug.
- Package inventory: **needs work first**, and is currently *broken* — a
  compile error latent behind a feature gate.
- Resident boot/tail topology: **needs work**, three hardcoded constants that
  No Mercy demonstrably violates.
- Captures: **must precede booting**, and No Mercy's existing boot context binds
  a ROM we do not have.

Recommended order: (1) fix the 35/32 inconsistency; (2) derive boot/tail
topology; (3) generate the package inventory per title; (4) re-capture No Mercy
BootContext; (5) capture executable-image groups; (6) build.
