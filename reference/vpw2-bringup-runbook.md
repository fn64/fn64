# VPW2 boot bring-up runbook

Virtual Pro Wrestling 2 — Oudou Keishou (Japan), the fourth AKI title to attempt
through the fn64 block lane. Written 2026-08-09 from what actually worked for
WWF No Mercy (`09155dd`) and WCW/nWo Revenge before it, so the next session runs
commands rather than rediscovering them.

**Nothing here has been executed.** Every step states its expected output and
its failure discriminator so a run can be judged without guessing.

## The ROM, bound by digest

    path    /Users/jer/Code/roms/n64/Virtual Pro Wrestling 2 - Oudou Keishou (Japan).z64
    sha256  358e9a345438155c6bd57da4bbf0f7a9fa1b4f7d5b1b726e8076c38f0f987e52
    size    33,554,432 (32 MiB)
    cart id NA2J   region byte 0x4a ('J', Japan — NTSC, so no PAL timing concern)

File every artifact under that digest, never under the title string. Five
variant collisions happened in one session on the other titles; No Mercy's filed
boot context bound a *different ROM* than the one on disk.

## Prior status

15/15 host bindings resolve. CPU-recompiles clean: 49,329 blocks / 5 banks,
`unsupported=0`. What it lacks is everything downstream of recompilation — no
shard pack, no boot example, no captures, and (per
`docs/plans/second-aki-title-scoping.md`) **no answer key and no
`FN64_DISCOVER_*` entry, so nothing grades it.**

## Step 1 — bookkeeping: register the ROM for discovery

Add to `.claude/local.env` alongside the existing NW4E/NWXE/OOT entries:

    export FN64_DISCOVER_NA2J_ROM="/Users/jer/Code/roms/n64/Virtual Pro Wrestling 2 - Oudou Keishou (Japan).z64"

A `_DUMP` entry (symbol dump) exists for the other titles but is not required to
boot — WM2000's and No Mercy's dumps come from the aki-recomp checkout, and VPW2
has none. Boot does not read it; grading does.

**Discriminator:** if a later step reports "no ROM for NA2J", this step was
skipped.

## Step 2 — generate the shard topology

    python3 scripts/generate-wm-shard-topology.py \
      --rom "$FN64_DISCOVER_NA2J_ROM" \
      --output-root . \
      --title vpw2-block-shards

`--title` sets both the directory under `examples/` and the Cargo package-name
prefix. Omitting it silently regenerates WM2000's tree.

**Expected:** a new `examples/vpw2-block-shards/` with N package directories and
a `shard_inventory.in`. For calibration, No Mercy produced **38** packages
(14 boot prefix + 21 overlay + 3 resident tail, overlay shape [2,2,5,7,5]);
WM2000 has 32. VPW2 recompiles 49,329 blocks against No Mercy's 57,284, so
expect a comparable count — a wildly different number is worth understanding
before building.

**Then apply the generator trap that is not fixed upstream** (`09155dd`
reapplied it rather than fixing it): `PACKAGE_PREFIX` in the generated
`examples/vpw2-block-shards/build.rs` (No Mercy's is at `build.rs:318`) must
read `"vpw2-block"`, not the templated WM2000 value.

## Step 3 — create the boot example

Copy `examples/nomercy-block-boot/` to `examples/vpw2-block-boot/` — it is the
closest template because its Cargo.toml already carries the three edits WM2000's
needs and No Mercy's had to make. Then, per the `09155dd` commit message, verify
all three are present and retargeted:

1. **Drop the `wm2000-shell` `[[bin]]`.** WM2000's manifest declares a second
   binary whose source (`src/shell.rs`) does not exist in a generated tree. No
   interactive lane for a new title yet.
2. **Drop the shell-only deps** — winit, pixels, gilrs, toml, dirs. They exist
   for `wm2000-shell` only.
3. **Retarget the profile package key.** `[profile.release.package.<name>]` must
   name *this* package. Cargo **silently ignores** a key naming a package that
   is not in the graph, and the result is a binary whose whole profile is
   attributed to one inlined frame — this is the trap that made `sample`
   attribute 99.99% of a profile to `run_one_step`.

Also rename the package and `[[bin]]` to `vpw2-block-boot`, and point its
dependency at `../vpw2-block-shards`.

**Discriminator:** `cargo metadata` from inside `examples/vpw2-block-boot`
resolves cleanly and lists the vpw2 shard packages. A "package not found" for a
shard name means the prefix in step 2 was not retargeted.

## Step 4 — capture the boot context and executable images

**These cannot be synthesized. Do not fabricate them** — a synthesized authority
artifact forges the very authority under audit.

Both come from real emulator runs against this exact ROM, filed under its
digest, following the shape of the existing artifacts:

    ~/Code/aki-recomp/captures/nomercy-11640379-boot-context.json
    ~/Code/aki-recomp/captures/nomercy-general-exception-images/{run-1,run-2,run-3}/image.json
    ~/Code/aki-recomp/captures/nomercy-general-exception-images/group-receipt.json

Read the No Mercy receipt for the schema: it records `schema`, `status:
validated`, `capture_count: 3`, `image_id: general-exception-preamble`,
`lineage: cpu_produced`, `capture_pc`, `va_start`, `byte_len`, `image_sha256`,
`authority_sha256`. Three runs must agree; the group is only `validated` when
they do.

**Verify the binding before using them:** every `image.json` and the boot
context carry `normalized_rom_sha256`, and it must equal the ROM identity
`358e9a345438155c6bd57da4bbf0f7a9fa1b4f7d5b1b726e8076c38f0f987e52` (a ROM identity, so no test owns it). No Mercy's
filed context bound `fc561fce…` — a different ROM — and only this check caught
it.

## Step 5 — save type: UNRESOLVED, and do not guess

**The generator templates WM2000's `SaveType::SramBanked` with a comment reading
"NWXE verifies SRAM…". NWXE is WM2000's cart ID.** That stale template shipped
into No Mercy's harness and cost a full debugging layer before anyone noticed
the title was FlashRAM. Treat the templated value as unknown, not as a default.

**Negative result, recorded so it is not repeated:** a byte-level fingerprint
for the libultra flash command immediates (`lui reg, 0x78xx/0x4bxx/0xd2xx/
0xb4xx/0xa5xx`) and the `lui reg, 0xa800` handle base does **not** discriminate.
Run against three ROMs of known type:

| ROM | known type | lui 0xa800 | 0x78 | 0x4b | 0xd2 | 0xb4 | 0xa5 |
|---|---|---:|---:|---:|---:|---:|---:|
| VPW2 | unknown | 1 | 6 | 28 | 63 | 17 | 13 |
| No Mercy | **FlashRAM** | 2 | 13 | 31 | 57 | 29 | 18 |
| WM2000 | **SRAM** | 2 | 10 | 24 | 58 | 23 | 12 |

WM2000 scores like No Mercy, so these are incidental immediates in 32 MiB of
data, not a driver fingerprint. **The method that did work on No Mercy** was
disassembling the boot path: locate `osFlashInit` by its *behaviour* — a
function that writes device type 8, latency 5, pulse 0x0C, page size 0x0F into
consecutive `OSPiHandle` byte fields and a `0xA8000000` base at offset 12 — then
confirm the command constants are reached through `osEPiWriteIo`. Absent that
cluster, the title is SRAM.

Note this matters less than it did: the FlashRAM API is now bound
(`7d2f13f` — `osFlashInit`, `osFlashSectorErase`, `osFlashReadArray` recognized
across 287 ROMs with zero false positives), so a FlashRAM title is now supported
rather than blocked. But the harness must still declare the right device, and
`require_flash_len` hard-panics on a 32 KiB store.

## Step 6 — build and boot

From the standalone workspace — `cargo run -p` from the repo root **fails**:

    cd examples/vpw2-block-boot
    export ROM="$FN64_DISCOVER_NA2J_ROM"
    G=~/Code/aki-recomp/captures/vpw2-general-exception-images
    export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
    export FN64_BOOT_CONTEXT=~/Code/aki-recomp/captures/vpw2-<digest-prefix>-boot-context.json
    export FN64_ABSENT_N64DD=1
    export FN64_RENDER=reference
    export FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
    export FN64_BLOCK_MAX_STEPS=1500000
    export FN64_RENDER_DUMP_DIR=<somewhere>
    export FN64_RENDER_DUMP_LIMIT=4000
    cargo run --release --bin vpw2-block-boot

**`FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1` is load-bearing.** Without it the binary
stops at first overlay entry and **exits 0** having rendered nothing. That
failure reads like success. **Judge by `gfx_submits`, never the exit code.**

No controller schedule is needed — `FN64_CONTROLLER_SCHEDULE` is an `Option`,
every port reads neutral, and boot frames come from the attract sequence.
Schedules gate gameplay only.

## Step 7 — read the tier reached

The ladder, as used for Revenge and No Mercy:

- **T1** — the workspace builds.
- **T2** — boots; log says `first-entry BootContext matches exactly`. A mismatch
  means the capture and the ROM disagree; go back to step 4's digest check.
- **T3** — runs without faulting; overlay entry reported, `idle_steps=0`,
  `audio_submits` climbing.
- **T4** — a frame renders: `gfx_submits > 0` and a NON-CLEAR dump.

**Filing a frame:** the reference backend's dump prefix is the hardcoded literal
`fn64-wm2000-block` for **every** title, so VPW2's PNGs land named
`fn64-wm2000-block-NNNN.png`. Rename before committing and file under the ROM
digest with a README recording the invocation — not just the outcome. Recording
only the outcome is what cost Revenge a full re-diagnosis.

## Known-shaped failures, with what each means

Each of these cost a run on another title. If VPW2 hits one, the diagnosis is
already done:

| symptom | meaning |
|---|---|
| exit 0, ~small step count, `gfx_submits=0`, evidence dump | `FN64_BLOCK_CONTINUE_AFTER_OVERLAY` unset |
| `package not found in workspace` | ran `-p` from the repo root instead of `cd`-ing into the example |
| `MemoryFault addr 0xffffffffa80…` | save-media window; wrong `SaveType`, or a flash path not bound |
| `OSPiHandle has invalid device type 8` | title is FlashRAM, harness declared SRAM |
| `unattributed executable mutation` | a host shim wrote guest RDRAM without declaring the span |
| `uncommitted child writer event(s)` | a span was declared but the child transaction never committed |
| `raw RDP opcode 0xNN has no public command width` | check the payload's *plausibility* before the opcode — a stale-buffer read decodes as garbage commands |

## Open questions

- **Answer key.** VPW2 has none, so `grade-all` cannot score it. Booting does not
  need one; regression-grading does. Bookkeeping, deferred.
- **Japanese ROM.** Region is NTSC so timing is unaffected, but nothing has
  verified whether discovery, the boot-context capture, or the recognizers key
  on cart ID or region anywhere. Worth a grep before blaming a failure on the
  title.
- **Save type**, per step 5.
