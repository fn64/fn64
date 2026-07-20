> **SUPERSEDED (2026-07-20).** This design was not implemented as written.
> The hardcoded-paths problem it targets was solved on `main` the opposite
> way: the `fn64-discover` gates now read their ROM/answer-key paths from
> **`FN64_DISCOVER_*` environment variables** (`FN64_DISCOVER_ROM`,
> `FN64_DISCOVER_DUMP`, `FN64_DISCOVER_TABLES`, `FN64_DISCOVER_REQUEST_DMA`,
> …), not from a `--config gates.toml` file — see `gate_decomp_functions.rs`
> and `gate_decomp_reference.rs`. The generic `FN64_DISCOVER_*` names avoid
> the specific objection this spec raised against env vars (that per-game
> names like `FN64_OOT_ROM` bake game identity into the variable and don't
> scale): one set of generic vars is supplied per game by the caller, so the
> binaries hardcode no game names. No `gates.toml` exists. Kept as a
> historical design record; do not implement.

---

# H3: gate binaries read ROM/answer-key paths from a config file

Status: design note
Date: 2026-07-17
Roadmap item: Phase H, H3 (`docs/ROADMAP.md`)

## Problem

`crates/fn64-discover/src/bin/{gate_b1,gate_b2,gate_d1}.rs` hardcode personal
absolute paths as Rust `const`s:

```rust
const OOT_ROM: &str = "/Users/jer/Downloads/Legend of Zelda, The - Ocarina of Time (USA).z64";
const NW4E_ROM: &str = "/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64";
const OOT_DUMP: &str = "/Users/jer/Code/aki-recomp/games/OOTU/syms/dump.toml";
```

These gates cannot run on any machine but the author's, and Phase H's own
goal — "fn64 owns its toolchain, everything needed to build and run it lives
in fn64, except a user's own game content" — is violated twice over: the
paths are personal, and several point at a legacy sibling checkout
(`aki-recomp`) that Phase H is separately cutting.

## Constraint: no-game-content rule

`docs/DESIGN.md` §1.0 states the rule this design must not violate: "Exactly
one class of input is legitimately out-of-tree — ROMs and anything
ROM-derived, which the no-game-content rule bars from git forever."

The gates' external inputs fall in two categories:

- **ROM files** (`.z64`) — unambiguously barred, forever out-of-tree.
- **Ground-truth answer keys** (`syms/dump.toml`, `segments.csv`,
  `overlays.json`) — full decomp-derived symbol dumps (OoT's key alone names
  13,358 zeldaret-authored function identifiers). These carry human-authored
  naming and organization from the decomp project, not just bare address
  facts, so they stay out-of-tree under the same rule, even though a bare
  `(address, size)` tuple alone would have a weaker copyright claim.

This is a different question from the project's existing small **testdata
fixtures** (`crates/fn64-discover/testdata/*.csv`, `*.txt`), which are
already committed and already correctly scoped: short, mechanically-named
(`func_80012345`) address/size lists with a provenance comment, used by
`gate_b2` as an in-repo smoke check. Nothing about this design changes those
— they stay as they are, and this design does not attempt to grow them into
a substitute for the full answer keys.

## Design

### Config file, not env vars, not CLI flags-per-game

A single TOML file supplies every ROM path and ground-truth path across all
three gate binaries. Passed via one required CLI flag:

```bash
gate_d1 --config gates.toml
```

Rejected alternatives and why:

- **Env vars per game** (`FN64_OOT_ROM`) — bakes game identity into the var
  name; doesn't scale past three hardcoded games.
- **CLI flags per game** (`--oot-rom`, `--nw4e-rom`) — same problem one
  layer down; the binary's flag set still hardcodes which games exist.
- **Config file** — the binary has zero hardcoded game names. Adding a
  fourth ROM is a config edit, not a code change. This is also the only
  option that keeps ROM-specific detail (paths, labels, which optional
  fields a given game has) entirely out of source, which is the point.

### Shape

```toml
[[game]]
label = "oot"
rom = "/Users/jer/Downloads/oot.z64"
ground_truth = "/Users/jer/Code/aki-recomp/games/OOTU/syms/dump.toml"
segments_csv = "/Users/jer/Code/aki-recomp/refs/oot-decomp/baseroms/ntsc-1.0/segments.csv"

[[game]]
label = "nw4e"
rom = "/Users/jer/Code/aki-recomp/games/NW4E/nomercy.z64"
ground_truth = "/Users/jer/Code/aki-recomp/games/NW4E/syms/dump.toml"
overlays_json = "/Users/jer/Code/aki-recomp/games/NW4E/overlays.json"

[[game]]
label = "nwxe"
rom = "/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64"
ground_truth = "/Users/jer/Code/aki-recomp/games/NWXE/syms/dump.toml"
```

`ground_truth` names the field by role (the answer key discovery output is
graded against), not by the upstream tool's own filename convention
(`dump.toml` is N64Recomp's internal name for this file, not a description
of what it is).

### Shared struct (`fn64-discover`, new small module)

```rust
#[derive(Deserialize)]
struct GateConfig { game: Vec<GameEntry> }

#[derive(Deserialize)]
struct GameEntry {
    label: String,
    rom: Option<PathBuf>,
    ground_truth: Option<PathBuf>,
    segments_csv: Option<PathBuf>,
    overlays_json: Option<PathBuf>,
}
```

Uses the `toml` + `serde` dependencies already present in
`fn64-discover/Cargo.toml` — no new dependency.

### Shared loading helper

All three binaries need identical `--config` parse + load + error-format
logic. This lives once in `fn64-discover`'s library (the new config module),
not duplicated three times:

```rust
// crates/fn64-discover/src/gate_config.rs
pub fn load_from_args() -> GateConfig { /* parses --config, reads, exit(1) on failure */ }
```

Each `bin/gate_*.rs` calls this one function and gets back a `GateConfig`;
per-binary code only differs in which `label`s and fields it then looks for.

### Per-binary behavior

Each gate iterates `config.game`, matching against the `label`s it has code
paths for (`"oot"`, `"nw4e"`, `"nwxe"` today).

An entry whose label a gate has no code for is not silently ignored — this
design's whole point is that nothing here fails silently. It prints one line
and moves on:

```
NOTE: gate has no checks for label "ott" — ignoring
```

This is a `NOTE`, not a `SKIP`: the config is well-formed, the gate simply
doesn't implement checks for that label (a gate capability gap, not a config
error), but the entry is still surfaced so a typo like `"ott"` for `"oot"`
is visible instead of the game quietly never being checked.

For a recognized label, missing fields that gate's checks need:

```
SKIP: nw4e: missing ground_truth — NW4E grading skipped
```

Loud, non-fatal — printed, that game's dependent block skipped, `exit_code`
untouched by the skip itself. Unaffected checks (other games, or blocks
whose required fields are present) still run.

Missing `--config` flag, unreadable file, or empty `[[game]]` list is an
error: usage message, `exit(1)`. A gate invoked with nothing to check is
almost certainly a mistake, not an intentional no-op.

### What is committed vs. not

- `crates/fn64-discover/gates.toml.example` — committed, placeholder paths,
  documents the shape.
- `crates/fn64-discover/gates.toml` — gitignored (personal machine paths).
- `crates/fn64-discover/testdata/*` — unchanged, already committed, already
  correctly scoped (small, mechanically-named, cited).
- Derived constants that are facts, not paths (`OOT_SHA1`, `OOT_FUNCTIONS`,
  `OOT_SECTIONS`, `NW4E_MAIN_ENTRY_VRAM`, `NWXE_MAIN_ENTRY_VRAM`,
  `NWXE_MD5`, `OOT_BOOT_CODE_END`) stay as Rust `const`s — unaffected by
  this change.

### Relationship to CI

These gates require real commercial ROMs and decomp-derived answer keys that
can never ship in this repo. They are not, and cannot become, a CI merge
gate — they are a manual/local verification tool for whoever has the ROMs
locally. (Separately worth a `ROADMAP.md` wording pass if any existing text
implies otherwise, but that correction is out of scope for this change.)

## Non-goals

- Does not change unit tests (already synthetic-byte-based, already correct
  per the project's own stated split between "mechanism proof" and "real
  bytes proof").
- Does not expand `testdata/` fixtures into full answer-key substitutes.
- Does not add a freely-licensed, source-buildable ROM (e.g. the MIT
  `n64-systemtest`, or Unlicense libdragon examples) as a CI-gated
  discovery-correctness fixture. Real candidates exist for this and were
  surveyed during this design's discussion, but building one into CI is a
  new capability (fn64's first-ever CI-gated real-discovery check), not a
  config refactor — it deserves its own separate design pass, tracked as a
  roadmap follow-up rather than folded into H3.
- Does not touch any file outside `fn64-discover`'s three gate binaries plus
  the new shared config-loading module.

## Acceptance

- No `/Users/...` or other personal/legacy-repo paths remain as `const`s in
  any `fn64-discover/src/bin/gate_*.rs`.
- The `--config` parse/load/error-format logic exists once, in
  `fn64-discover`'s new config module, and all three `gate_*.rs` binaries
  call it rather than each re-implementing it.
- Given no `--config` flag at all, each gate prints usage and `exit(1)`.
- Given `--config` pointing at an empty or all-fields-missing config, each
  gate prints one `SKIP:` line per unmet check and `exit(0)`.
- Given `--config` containing a `[[game]]` entry whose `label` the gate has
  no checks for, the gate prints one `NOTE:` line naming the unrecognized
  label and continues — it is never silently dropped.
- Author's own machine, with a real `gates.toml`, reproduces today's grading
  numbers unchanged.
