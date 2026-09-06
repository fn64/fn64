# fn64 cleanup, fix, and land-on-main plan (2026-09)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking. Each task is one PR. Read
> `AGENTS.md` and "Quality bar" below before touching anything.

**Goal:** Everything fn64 can do today is buildable from `origin/main`, with
zero warnings, one configuration surface, the type invariants the project
already promises, tests that prove parity against an external oracle, and the
largest files split by concern.

**Architecture:** Seven phases in dependency order. Phase 0 lands stranded
work, because every later phase edits files that the stranded branches also
edit. Phases 1 to 4 are mechanical hygiene, configuration, types, and
structure. Phase 5 adds the tests that the earlier phases exposed as missing.
Phase 6 is the only performance work and is gated on Phase 1's release
profile and the perf method. Phase 7 fixes the contract and the narrative.

**Tech stack:** Rust 2021 workspace, cargo-nextest, clap 4 (new), thiserror 2
(new), proptest 1 (new, dev-only), cargo-deny (new, CI-only), Python 3 lint
scripts.

**Spec:** This document argues from the 2026-09-05 fresh-look review. The
measurements in "Baseline" are the spec; each task names the number it moves.

## Baseline (measured 2026-09-05 on `origin/main` = `7628f240`)

| Fact | Value |
|---|---|
| Rust lines / crates / files over 2,000 lines | 685k / 17 / 36 |
| `fn64-render-wgpu` total / non-test lines | 199,774 / ~68,000 |
| `production.rs` total / non-test lines | 22,397 / 9,741 |
| `cargo check --workspace --all-targets` warnings | 106 (41 in `fn64-render-wgpu` lib tests) |
| `[profile.release]` in `Cargo.toml` | absent |
| Duplicate dependency trees | wgpu 0.19.4 + 30.0.0, naga, thiserror 1 + 2, bitflags 1 + 2, hashbrown, objc2 |
| Distinct `FN64_*` env vars read in non-test code / call sites | 288 / 228 |
| Argument-parsing crates | none; `fn64-shell` accepts only `--demo` |
| `fn64-discover` `[[bin]]` targets | 51 |
| `unsafe` blocks / `// SAFETY` comments | 1,132 / 168 (937 sites in `fn64-abi`) |
| Tests listed by nextest / property tests | 8,857 / 0 |
| RT64 oracle (`--features rt64`) runs in CI | never |
| Local branches with unmerged commits / live worktrees / open PRs | 25 / 10 / 0 |
| September perf work on main (`FN64_AUDIO_PRIORITY`, `render_join_wait_ns`) | does not exist on main; only on `perf/wm2000-audio-lockfree-land` |
| Docs / RT64-prefixed docs | 120 / 65 |
| `cargo-deny` or `cargo-audit` | none |

## Global constraints

- Clean-room protocol in `AGENTS.md` applies to every task. No GPL runtime
  code, no m2c, cite the allowed source for any behavior claim.
- `cargo nextest run --workspace` is the green gate. `cargo test` is not.
- `python3 scripts/lint-docs.py`, `python3 scripts/lint-hot-path-env.py`,
  `python3 scripts/lint-source-pins.py` (task 5.5: every test that pins a
  needle from `include_str!("<file>")` via `.find`/`.contains` must still
  find it in `<file>`), and `python3 scripts/check-reference-frame-digests.py
  --allow-missing` (task 5.5: the script now fails closed without
  `--allow-missing` when no frame and no `FN64_SHARD_ROOT` checkout is
  present, per `gates-must-fail-on-unusable-input`) must pass on every PR.
- A behavior change updates its doc in the same commit.
- `--all-features` is not a substitute for a targeted feature build; it fails
  on clean main (see `HANDOFF.md`).
- No `git stash`, `git reset --hard`, or `git checkout -- .` in a shared
  worktree. Use a temporary WIP commit.
- Never push to `main`. Every task lands through a PR.

## Quality bar (what "not slop" means here)

1. **One concern per PR, under ~1,500 non-generated changed lines.** A move
   with no logic change is its own PR, tagged `refactor(move):`, and reviewed
   by diffing `git diff -M --color-moved`.
2. **No `#[allow(...)]` to silence a warning.** Fix the cause or delete the
   code. The only exception is a documented false positive with the lint
   name and the reason on the line above.
3. **No new `FN64_*` env var** after Task 2.2 lands. New knobs go in the
   config struct and the registry doc.
4. **Every `unsafe` block carries `// SAFETY:`** stating the invariant, after
   Task 4.7. Until then, no new unsafe without one.
5. **Perf claims cite interleaved OFF/ON/OFF/ON pairs** with the within-arm
   spread, per `docs/plans/perf-method.md`. A single pair is a hypothesis.
6. **Deleted code is not moved to `evidence/` or a doc.** Git history is the
   archive.
7. **Reviewer tier:** anything that produces a number, changes a type
   boundary, or touches `fn64-abi` unsafe code gets an Opus reviewer. Pure
   moves and mechanical renames may use Sonnet.
8. **The PR description states what was verified and how**, including any
   "not verified" items. A false "done" fails review.
9. **Premise check before implementation** (added 2026-09-06 after three
   briefs stated wrong facts: 1.1, 2.2, 2.4). Every brief opens with the
   two or three facts it depends on; the implementer verifies them first
   and stops to report if any is wrong. The controller verifies them
   before dispatching.
10. **Tests named per touched crate** (added 2026-09-06 after Task 2.4
    `cargo check`ed a crate whose tests then failed at the phase gate).
    The report lists every crate from `git diff --stat` and shows a
    `cargo nextest run -p <crate>` result for each, default features
    included for feature-gated crates. `cargo check` is not a test.
11. **Presenter or configuration change checklist** (added 2026-09-06
    after the tripwire missed a gamma bug and a lane run was skipped).
    Any change to fn64-shell's surface, blit, egui, or `Knobs` plumbing,
    or to how a library crate receives configuration, requires: a
    presented-surface readback or a human eye on the window, and one
    3,000-pump WM2000 lane run compared to the last recorded arm.
12. **Source-pin tests are swept on structural refactors.** Phase 4 file
    splits and trait moves must run `rg -n 'include_str!\("lib.rs"\)|\.find\("fn ' crates`
    and re-run every test in the matching files; a pin whose needle
    moved is re-pinned to the new layout, never deleted.
13. **No incremental compilation for agents.** The workspace default
    stands; `CARGO_INCREMENTAL=1` produced 22 GB of cache in one night and
    took the volume to zero.

---

## Phase 0: land what exists

Nothing else is safe to start until the tree that people build is the tree
that has the fixes.

### Task 0.1: Land `perf/wm2000-audio-lockfree-land`

**Why:** The audio-priority VI join (the fix for the "static in red scenes"
bug) and the render-join census exist only on this branch: 134 commits, 207
files, based on `339a55d0` (2026-08-27). Main cannot reproduce the current
WM2000 behavior.

**Files:** whole branch; conflicts expected in `crates/fn64-abi/src/task_dispatch/`,
`crates/fn64-render-wgpu/src/production.rs`, `crates/fn64-shell/src/`.

- [ ] **Step 1:** Create `integrate/audio-lockfree` from `origin/main`. Merge
  `perf/wm2000-audio-lockfree-land` with `git merge --no-ff`. Resolve
  conflicts toward the branch's behavior; where main's `refactor(render):
  expose prepared triangle row bins` (`339a55d0`) touched the same lines,
  keep main's structure and re-apply the branch's logic.
- [ ] **Step 2:** Run the fn64-verify skill (clean-target pinned worktree,
  full workspace). Expected: all tests pass, three lint scripts exit 0.
- [ ] **Step 3:** Run the WM2000 3,000-pump scripted lane twice each for
  `origin/main` and the merge, interleaved (main, merge, main, merge).
  Record `underrun_sample_slots`, `over_budget`, `max_pump` per run.
  Expected: underruns 0 on the merge in all runs; over_budget within the
  documented 4-point noise floor of the branch's own numbers.
- [ ] **Step 4:** Open the PR with the four-run table in the description.
  Do not squash 134 commits into one; do squash fixup noise into the
  commit it fixes with `git rebase --autosquash` where the branch used
  `fixup!`.
- [ ] **Step 5:** After merge, delete the branch and its worktree.

**Gate:** CI green, table in PR, memory note
`wm2000-red-scene-audio-starvation` updated to cite the main commit.

### Task 0.2: Land `feat/wm-block-external-symbol-ref`

**Files:** 8 files, 2,286 insertions, all in `fn64-discover` and one in
`fn64-cpu-runtime`.

- [ ] Rebase onto post-0.1 main (`git rebase origin/main`; 6 commits, low
  conflict risk since 0.1 does not touch `fn64-discover`).
- [ ] `cargo nextest run -p fn64-discover -p fn64-cpu-runtime`, then the
  fn64-firewall skill (boundary grading + determinism gates).
- [ ] PR. Gate: firewall report attached.

### Task 0.3: Triage the other 23 unmerged branches

**Why:** 25 local branches carry unmerged commits, including
`worktree-wm2000-playable` at 764 commits and 288k inserted lines on a
2026-08-17 base. Untriaged branches are where fixes go to be re-discovered.

- [ ] **Step 1:** Generate the table with this exact command and paste it
  into the PR description of Task 0.4:

  ```sh
  for b in $(git branch --format='%(refname:short)' --no-merged origin/main); do
    n=$(git rev-list --count origin/main..$b)
    [ $n -gt 0 ] && printf "%4d %s %s %s\n" $n \
      "$(git log -1 --format=%ad --date=short $b)" $b \
      "$(git diff origin/main...$b --shortstat | tail -1)"
  done | sort -k2 -r
  ```

- [ ] **Step 2:** For each branch, one verdict, recorded in the table:
  - `land`: rebase and PR (expect: `integrate/recompiler-coverage` 51 commits,
    `fix/native-4x3-presentation`, `feat/rdram-dump-tooling`, the four
    `integrate/*` recognizer branches).
  - `superseded`: `git diff origin/main...$b -- <key files>` is empty or the
    change exists on main under another commit. Delete with
    `git branch -D`, and note the main commit that superseded it.
  - `salvage`: cherry-pick the named commits, then delete.
- [ ] **Step 3:** `worktree-wm2000-playable` specifically: after 0.1 lands,
  run `git diff origin/main...worktree-wm2000-playable --stat | tail -1`. If
  the remaining diff is under 5k lines, salvage by cherry-pick. If not, list
  the top 20 files by churn, salvage the ones with no main equivalent, and
  delete the branch. 764 commits do not rebase; do not try.
- [ ] **Step 4:** `git worktree prune`; remove worktrees whose branch was
  deleted; run `scripts/reap-idle-worktree-targets.zsh`.

**Gate:** table with a verdict per branch; `git branch --no-merged origin/main`
lists only branches with an open PR.

### Task 0.4: Working-tree and repo dirt

**Files:** `.gitignore`, `.superpowers/` (50 tracked files), `keel/`,
`raw_coverage.rs` under the reference renderer's `raster/` module (untracked in the
main checkout), `vitrine-full.png`, `.playwright-mcp/`.

- [ ] Decide `raw_coverage.rs` with its author: it is uncommitted work in the
  main checkout alongside edits to `raster/draw.rs` and `raster/mod.rs`.
  Either commit it on its own branch with its test, or delete it. Do not
  leave it untracked.
- [ ] `git rm -r --cached .superpowers` and add `/.superpowers/` to
  `.gitignore` under the existing "Agent scratch" comment. These are
  session handoff notes, the same class as `.claude-handoffs/`, which is
  already ignored.
- [ ] Add `/.playwright-mcp/` and `/*.png` to `.gitignore`; delete
  `vitrine-full.png`.
- [ ] `keel/`: five empty untracked directories that trigger the keel skill.
  Either delete the directory, or commit a `keel/README.md` naming what the
  "selective Keel launch control plane" commit (`7628f240`) expects to live
  there. Empty directories are not a control plane.
- [ ] Commit: `chore: untrack agent scratch, ignore captures, settle keel/`.

**Gate:** `git status --short` empty in a fresh clone after a build.

---

## Phase 1: build and CI hygiene

### Task 1.1: Withdrawn — measured (2026-09-06)

Measured and declined. The brief's premise was false: the WM2000 play binary
builds from the standalone `crates/fn64-shell/rs` workspace, which has carried
`lto = "fat"` + `codegen-units = 1` since `0f8f56e7` (-2.8%, measured there), so
a root profile cannot reach it; `target-cpu=native` was declined as a
cross-machine determinism hazard. Four-run OFF/ON/OFF/ON table and the full
reasoning: `docs/plans/perf-method.md`, entry dated 2026-09-06.

### Task 1.2: Zero warnings, enforced

**Files:** every file `cargo check --workspace --all-targets` warns on; new
CI job in `.github/workflows/ci.yml`.

- [ ] **Step 1:** `cargo check --workspace --all-targets 2>&1 | rg '^warning' | sort | uniq -c | sort -rn` and fix by category. Unused imports and dead
  test helpers are deleted, not allowed. Unused `Result` values get `?` or an
  explicit `let _ = ` with a one-line reason.
- [ ] **Step 2:** Add a third CI job that fails on warnings without
  perturbing the test job's cache:

  ```yaml
  lint:
    name: clippy (workspace, -D warnings)
    runs-on: ubuntu-latest
    env:
      CARGO_INCREMENTAL: "0"
      CARGO_PROFILE_DEV_DEBUG: "0"
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: rui314/setup-mold@v1
      - uses: Swatinem/rust-cache@v2
        with:
          prefix-key: v1-clippy
      - run: sudo apt-get update && sudo apt-get install -y libasound2-dev libudev-dev
      - run: cargo clippy --workspace --all-targets -- -D warnings
  ```

  If clippy's default lint set produces more than ~50 findings, land this
  job with `-D warnings -A clippy::all` first (rustc warnings only), then
  enable clippy groups one PR at a time.
- [ ] **Step 3:** Confirm the job fails on a deliberate `let x = 1;` before
  merging, then remove it.
- [ ] Commit: `ci: fail on warnings; fix the 106 existing`.

### Task 1.3: One wgpu tree

**Why:** `fn64-shell` depends on `pixels`, which pins wgpu 0.19.4, while
`fn64-render-wgpu` uses wgpu 30.0.0. The workspace compiles two wgpu, two
naga, and two `objc2` trees, and the old tree drags in `block 0.1.6`, which
rustc flags as future-incompatible.

**Files:** `crates/fn64-shell/Cargo.toml`, `crates/fn64-shell/src/framebuffer.rs`
and whichever module owns the window surface.

- [ ] **Step 1:** `cargo tree -i pixels` to list every path. Confirm `pixels`
  is the only consumer of wgpu 0.19.
- [ ] **Step 2:** Replace `pixels` with a direct wgpu 30 surface: one
  `wgpu::Surface`, one texture upload per presented field, one fullscreen
  blit pipeline. `fn64-render-wgpu` already has `rt64_fullscreen_vs` and a
  VI scanout that produces the RGBA field; the shell only needs to present
  it. Keep the 4:3 letterbox math from `fix/native-4x3-presentation` if
  Task 0.3 landed it.
- [ ] **Step 3:** `cargo tree -d --workspace -e normal | rg -E '^(wgpu|naga|block|objc2)'` shows one version each.
- [ ] **Step 4:** Windowed smoke: `scripts/play-wm2000.sh` opens a window,
  frame tripwire 120/120 byte-identical versus the pre-change build (the
  tripwire already exists; see memory `wm2000-perf-gpu-draw-is-unpresented`).
- [ ] Commit: `shell: present through wgpu 30 directly; drop pixels`.

### Task 1.4: The RT64 oracle runs in CI

**Why:** The only external ground truth (`--features rt64`, 79 cfg sites,
every conformance runner) has never run in CI. Green today proves fn64 is
self-consistent, not that it matches RT64.

**Files:** `.github/workflows/ci.yml`, `scripts/gate-rt64-parity.sh`,
`docs/rt64/RT64-PARITY.md`.

- [ ] **Step 1:** Determine the inputs `gate-rt64-parity.sh` needs that a
  hosted runner lacks: the RT64 checkout at the pinned oracle commit
  (`f0728a25`, per `docs/rt64/RT64-PORT-AUTHORITY.md`), CMake, and a Vulkan
  device. Lavapipe is already installed by the test job.
- [ ] **Step 2:** Add a scheduled job (nightly, plus `workflow_dispatch`),
  not a per-PR job, that clones RT64 at the pin into `$RUNNER_TEMP/rt64`,
  exports `FN64_RT64_DIR`, builds `-p fn64-render-conformance --features rt64`, and runs the parity runner over the committed fixture corpus with
  `WGPU_BACKEND=vulkan`. Fail the job on any divergence the parity doc does
  not list as known.
- [ ] **Step 3:** Prove it fails: temporarily flip one expected digest, run
  `workflow_dispatch`, observe red, revert.
- [ ] **Step 4:** Add the job's badge and a one-paragraph "what CI proves"
  section to `docs/rt64/RT64-PARITY.md`.
- [ ] Commit: `ci: nightly RT64 oracle parity job`.

### Task 1.4b: The parity runner executes under Lavapipe (added 2026-09-06)

**Why:** The first Actions runs (PR #172) proved the job builds end to end on
`ubuntu-latest` after three environment fixes (`libgtk-3-dev`, GTK link libs
in `build.rs`, the parity runner's stale `#![cfg(target_os = "macos")]`),
and then the gate step exits 139 (segmentation fault) immediately after
`[gate-rt64-parity] running three-way differential`, before any case runs.
The runner has only ever executed on macOS/Metal. Until it runs on Lavapipe
the "adapter differs" caveat in `docs/rt64/RT64-PARITY.md` §7 has no measurement
behind it, and the workflow is build-only on pull requests.

**Files:** `crates/fn64-render-rt64/src/ffi/*`, `crates/fn64-render-rt64/build.rs`,
`scripts/gate-rt64-parity.sh`, `.github/workflows/rt64-oracle.yml`,
`docs/rt64/RT64-PARITY.md` §7.

- [ ] **Step 1:** Reproduce on Linux (a container with the workflow's apt
  list, Lavapipe, and the RT64 checkout at the oracle pin) with a debug
  build and a core dump or `gdb` backtrace. Candidates, in order: RT64's
  Vulkan device creation against `llvmpipe` (no `VK_ICD_FILENAMES`?), SDL
  video init with no display (`SDL_VIDEODRIVER=dummy`/`offscreen`), and the
  `fn64_rt64_shim` FFI context setup assuming a Metal-backed identity
  (`backend_impl.rs` `release_identity_with_post_vi_api`).
- [ ] **Step 2:** Fix the cause in the runner, the shim, or the workflow's
  environment; never by catching the fault. If Lavapipe genuinely cannot host
  RT64, say so in §7 and move the gate to a macOS runner instead.
- [ ] **Step 3:** Remove the `if: github.event_name != 'pull_request'` on
  the gate step and the build-only step once a dispatch run is green; record
  the first Lavapipe `differing_pixels` counts against the §4 rows.
- [ ] Commit: `ci(rt64-oracle): parity gate runs under Lavapipe`.

### Task 1.5: Dependency policy

**Files:** `deny.toml` (new), `.github/workflows/ci.yml` docs job.

- [ ] Add `deny.toml` allowing `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`,
  `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `MPL-2.0`,
  `Unlicense`, `CC0-1.0`; deny `GPL-*`, `AGPL-*`, `LGPL-*`; `multiple-versions = "warn"`
  until Task 1.3 lands, then `"deny"`; `unmaintained = "warn"`; `yanked = "deny"`.
- [ ] Add `- uses: EmbarkStudios/cargo-deny-action@v2` to the docs job.
- [ ] Commit: `ci: cargo-deny licenses and advisories`. This mechanizes the
  clean-room license rule that `AGENTS.md` currently enforces by review.

---

## Phase 2: configuration and interfaces

### Task 2.1: Knob registry

**Why:** 288 distinct `FN64_*` variables are read at 228 sites, parsed ad hoc
(56 sites compare a string to `"1"`), and documented in one place
(`RT64-RUNTIME-CONTROLS.md`, since retired by Task 7.4) that covers a fraction of them.

**Files:** a new `knob-registry.py` in `scripts/`, a generated
`RUNTIME-KNOBS.md` in `docs/`, `scripts/lint-docs.py` (register the generated doc).

- [ ] **Step 1:** Write the `knob-registry.py` script that scans
  `crates/*/src` (non-test) for `FN64_[A-Z0-9_]+`, and emits a table:
  name, crate, first file:line, read count, and a classification column
  read from a new hand-maintained `knobs.toml` in `docs/`: `user`, `diagnostic`,
  `test-only`, `dead`. Unknown names fail the script.
- [ ] **Step 2:** Classify all 288. Expect the bulk under `FN64_DISCOVER_*`
  (205 reads) to be `test-only` ROM/dump paths, and `FN64_*_CENSUS`,
  `FN64_PROFILE`, `FN64_PHASE_TIMING` to be `diagnostic`.
- [ ] **Step 3:** Delete every `dead` knob and its read site in one PR per
  crate.
- [ ] **Step 4:** Wire the script into the docs CI job so the table cannot
  drift.
- [ ] Commit: `docs: generated runtime knob registry; delete dead knobs`.

### Task 2.2: One configuration surface

**Why:** Typed config structs already exist (`RenderConfig`,
`RenderRuntimeSettings`, `AudioConfig`, `RecompConfig`, `InputConfig`,
`VideoConfig`) but runtime code bypasses them and reads the environment
directly. `fn64-shell` has no argument parser; its only flag is `--demo`.

**Files:** `crates/fn64-shell/Cargo.toml` (add `clap = { version = "4", features = ["derive"] }`),
a new `cli.rs` in `crates/fn64-shell/src/`, `crates/fn64-shell/src/main.rs`,
`crates/fn64-render/src/settings.rs`, `crates/fn64-abi/src/lib.rs`.

**Design:** precedence is CLI flag, then `fn64.toml` next to the shard root
or at `$XDG_CONFIG_HOME/fn64/fn64.toml`, then `FN64_*` env var (kept for
one release for scripts), then the struct default. Parsed once in `main`
into a `Knobs` struct; library crates receive `&Knobs` or the relevant
sub-struct. No library crate reads the environment after this task except
through `Knobs::from_env_compat()`.

- [ ] **Step 1:** Write `cli.rs` with a `#[derive(clap::Parser)] struct Cli`
  covering: `--rom <path>`, `--shard-root <path>`, `--recomp {rs,c}`,
  `--render {wgpu,reference,rt64}`, `--audio-priority <bool>`,
  `--audio-priority-join-budget-ms <u32>`, `--demo`, `--config <path>`,
  `--print-config`. Every field has a doc comment; clap renders `--help`
  from it.
- [ ] **Step 2:** Write the failing test in `cli.rs`:

  ```rust
  #[test]
  fn env_var_loses_to_flag_and_wins_over_default() {
      let knobs = Knobs::resolve(
          Cli::parse_from(["fn64", "--render", "reference"]),
          None,
          |name| (name == "FN64_RENDER").then(|| "wgpu".to_string()),
      );
      assert_eq!(knobs.render.backend, RenderBackendKind::Reference);
      let knobs = Knobs::resolve(
          Cli::parse_from(["fn64"]),
          None,
          |name| (name == "FN64_RENDER").then(|| "wgpu".to_string()),
      );
      assert_eq!(knobs.render.backend, RenderBackendKind::Wgpu);
  }
  ```

  The env lookup is a closure so the test never touches the real process
  environment.
- [ ] **Step 3:** Implement `Knobs::resolve(cli, file: Option<FileConfig>, env: impl Fn(&str) -> Option<String>) -> Knobs`.
- [ ] **Step 4:** Replace the `user` and `diagnostic` env reads from Task 2.1
  in `fn64-shell`, `fn64-render`, `fn64-render-wgpu`, and `fn64-abi` with
  fields on `Knobs`, one crate per PR. `WgpuBackend`'s eight loose `bool`
  fields and its `env_exact_one`/`env_default_one` helpers
  (`production.rs:1730-1862`, `:2988-3018`) become a `ProbePolicy` struct
  constructed from `Knobs`.
- [ ] **Step 5:** Expand `scripts/lint-hot-path-env.py` `TARGETS` from a
  function allowlist to whole-crate coverage for `fn64-render-wgpu`,
  `fn64-abi`, and `fn64-runtime`; the only permitted `env::var` call in
  those crates is inside `Knobs::from_env_compat`.
- [ ] **Step 6:** `--print-config` emits the resolved `Knobs` as TOML; a
  user can save it as `fn64.toml`. Document in `README.md` "Running".
- [ ] Commit series: `shell: clap CLI and Knobs`, then `render: read knobs not env`, etc.

**Gate:** `fn64 --help` lists every user-facing knob; lint-hot-path-env passes
with the widened targets; `FN64_RENDER=wgpu fn64 --render reference` picks
reference.

### Task 2.3: One `fn64-discover` binary

**Why:** 51 `[[bin]]` targets each link their own copy of the workspace
rlibs; that is why `target/debug` reached 155 GB and why every gate build is
a full link.

**Files:** `crates/fn64-discover/Cargo.toml`, `crates/fn64-discover/src/bin/*.rs`
(51 files), a new `main.rs` in `crates/fn64-discover/src/`, `scripts/grade-all.sh`,
`scripts/gate-determinism.sh`, `scripts/play-wm2000.sh`, the fn64-firewall
and fn64-frontier skills.

- [ ] **Step 1:** Add `clap` to `fn64-discover`. Create `src/main.rs` with an
  `enum Command` whose variants are the 51 current binary names in
  kebab-case (`gate-decomp-functions`, `recompile-rom`, `corpus-index`, ...).
  Each variant's args are the hand-parsed `std::env::args()` fields from the
  corresponding `bin/*.rs`, converted to clap fields.
- [ ] **Step 2:** Move each `bin/<name>.rs` body into
  `src/commands/<name>.rs` as `pub fn run(args: <Name>Args) -> Result<(), CommandError>`.
  Mechanical; one PR per ten commands.
- [ ] **Step 3:** Keep the old binary names working for one release with
  thin shims: `target/release/gate_decomp_functions` becomes a two-line
  `fn main() { fn64_discover::cli::main_with(["gate-decomp-functions"]) }`
  only if a script cannot be updated in the same PR. Prefer updating the
  scripts.
- [ ] **Step 4:** Update `scripts/lint-discover-bin-tests.py` to the new
  layout.
- [ ] **Step 5:** Measure: `du -sh target/release` before and after a clean
  `cargo build --release -p fn64-discover`; record in the PR.
- [ ] Commit: `discover: one CLI with subcommands`.

### Task 2.4: Split `RenderBackend`

**Why:** `crates/fn64-render/src/lib.rs:1632-2183` is a 33-method trait. Eight
are `raw_dpc_*`/`plan_*`/`execute_*`; five `apply_*_settings` share one
default body returning `Err(RenderError::Backend { .. })`.

**Files:** `crates/fn64-render/src/lib.rs`, `crates/fn64-render/src/backend/{mod,raw_dpc,settings}.rs` (new),
`crates/fn64-render-wgpu/src/production.rs`, `crates/fn64-render-reference/src/lib.rs`,
`crates/fn64-render-rt64/src/lib.rs`.

- [ ] **Step 1:** Define three traits: `RenderBackend` (create, resize,
  present, observe), `RawDpcBackend` (plan, execute, publish), and
  `SettingsSink` with one method `fn apply(&mut self, scope: SettingsScope) -> Result<(), RenderError>` where `SettingsScope` is an enum over the five
  current settings structs.
- [ ] **Step 2:** Blanket-implement the old trait for `T: RenderBackend + RawDpcBackend + SettingsSink`
  so the three implementations keep compiling; convert each implementation
  in its own PR; delete the blanket and the old trait last.
- [ ] Commit series: `render: split RenderBackend (blanket impl)`, then one
  per backend.

---

## Phase 3: types

The project's own rule is "types before audits." These tasks put the
invariants the code already assumes into the type system.

### Task 3.1: `thiserror` and the end of `Result<_, String>`

**Why:** 193 hand-rolled error enums with 116 hand-written `Display` impls,
zero `thiserror`, and 211 `Result<_, String>` returns in the five core
crates. The boilerplate tax is why new code reaches for `String`.

**Files:** every file with `impl std::fmt::Display for *Error`;
`crates/fn64-abi/src/recompiled/runners.rs` (8 `String` returns),
`crates/fn64-discover/src/banks/mod.rs` (3).

- [ ] **Step 1:** Add `thiserror = "2"` to each crate. Convert `Display`
  impls to `#[derive(thiserror::Error)]` with `#[error("...")]` carrying
  the identical message text. This is pure deletion; the test that asserts
  on the message (if any) proves it.
- [ ] **Step 2:** For each `Result<_, String>` in library (non-`bin`) code,
  return the nearest existing error enum, adding a variant with the fields
  the message interpolated. `bin` code may keep `String` until Task 2.3
  gives it `CommandError`.
- [ ] Commit series: one per crate, `errors: thiserror in <crate>`.

### Task 3.2: `RdramAddr` reaches the internal seams

**Why:** `RdramAddr` (`crates/fn64-runtime/src/rdram.rs:195`) is correct, but
239 sites unwrap it to `u32` and 461 re-wrap. Example:
`crates/fn64-render/src/geometry_task_inspection/mod.rs:1226` takes
`address: u32` and calls `RdramAddr::from_offset` on line 1229;
`crates/fn64-abi/src/si/mod.rs:716` hand-rolls `to_kseg0`.

- [ ] Push `RdramAddr` into the ~40 internal `fn(.., address: u32)`
  signatures in `fn64-abi` and `fn64-render`. Raw `u32` survives only at
  the 13 `extern "C"` boundaries in `fn64-abi`.
- [ ] `RenderError::UnsupportedUcode { ucode_addr: u32 }`
  (`crates/fn64-render/src/lib.rs:1534`) becomes `ucode_addr: RdramAddr`.
- [ ] Replace the hand-rolled `offset() + 0x8000_0000` at `si/mod.rs:716`
  with `.to_kseg0()`.
- [ ] Commit: `types: RdramAddr at internal seams`.

### Task 3.3: Withdrawn — measured (2026-09-06)

Measured and declined at the implementer's premise check. `mask_address`
has zero non-test call sites: its 23 references are the definition, four
doc mentions, and 18 `#[cfg(test)]` assertions, and the three modules named
below (`rt64_rdp_state`, `rt64_framebuffer_storage`, `rt64_rsp_segment`) are
private, unwired RT64 port surface with whole-file SHA-256 pins in
`docs/rt64/RT64-PORT-AUTHORITY.md`. The refactor is also not behavior-preserving:
the `extend_rdram` arm computes `address - 0x8000_0000`, a 31-bit result
(`0x8123_4567` and `0xffff_ffff` both exceed `0x0100_0000`), which
`PhysicalAddress::try_new` rejects; the frozen test at
`rt64_rdp_state.rs:906` pins exactly that. `rt64_rsp_segment.rs:75-99` and
`rt64_frame_compatibility.rs:111-125` (card M8.12) had each already rejected
this design in-tree. The "19 of 44 signatures" figure counted inert port
surface as live seam; Tasks 3.4, 3.6 and 3.7 are premise-checked against
the same trap before dispatch. Original text follows for the record.

**Why (original):** 19 of 44 raw-`u32`-address signatures are in `fn64-render-wgpu`,
though `PhysicalAddress` (`crates/fn64-render-ir/src/address.rs:41`) exists
with a proven 24-bit bound. `rt64_rdp_state.rs:378`
`mask_address(address: u32, extend_rdram: bool)` is a free function anyone
can forget to call.

- [ ] `mask_address` becomes `PhysicalAddress::masked(raw: u32, ext: RdramExtension) -> PhysicalAddress`
  where `enum RdramExtension { Standard, Expanded }` replaces the bool.
- [ ] `rt64_framebuffer_storage.rs:311,337` (`store`/`get`),
  `rt64_rsp_segment.rs:306` (`set_segment`) take `PhysicalAddress`. ~60
  call sites.
- [ ] Commit: `types: PhysicalAddress in wgpu framebuffer/segment`.

### Task 3.4: Withdrawn — measured (2026-09-06)

Measured and declined, twice. The original brief cited three decode sites;
a premise check found two of them are private, SHA-pinned RT64 port modules
(`rt64_preset_draw_call_match.rs`, `rt64_hle_geometry.rs`, whose
`:1000-1009` comment correctly refuses the substitution because RT64's
`rgbDither()` keeps the bits in place while fn64's accessor shifts first),
and that a typed `OtherMode { high, low }` already exists at
`crates/fn64-render-wgpu/src/state.rs:203`. The re-scoped brief (route live
hand decodes through that type) then failed its own premise check: the live
wgpu path contains zero hand decodes — every cited site is a census key,
log line, initializer, or builder that constructs wire words — and every
other-mode shift/mask expression outside `state.rs` is a `#[cfg(test)]`
positive control that deliberately derives a field independently of the
accessor (`raw_dpc/mod.rs:4018`, `texrect.rs:5864`); converting those would
make the tests tautological. The invariant this task wanted ("decoding
logic for each field exists once") already holds: `OtherMode` has 27
`const fn` accessors covering both registers. `proptest` was therefore not
added here; Task 5.2 adds it. Lesson for briefs: a count of mentions is not
a count of decodes; cite the exact expression to change. Original text
follows for the record.

**Why (original):** `CycleType`, `ImageFormat`, `AlphaCompare`, and the combiner and
blend enums are typed, but `other_mode_h`/`other_mode_l` still travel as raw
`u32` and are decoded at ~25 sites (`targets/compute_batch.rs:42-43`,
`rt64_preset_draw_call_match.rs:293-294`, `rt64_hle_geometry.rs:1013`).

- [ ] Add `struct OtherModeH(u32)` and `struct OtherModeL(u32)` in
  `crates/fn64-render-wgpu/src/state.rs` with accessors returning the
  existing enums (`cycle_type()`, `alpha_compare()`, `dither_pattern()`,
  ...). The raw word is exposed only through `.bits()` for the GPU-uniform
  boundary in `compute_batch`.
- [ ] Each decode site calls the accessor; decoding logic exists once.
- [ ] Property test (also counts toward Task 5.2): for all `u32`, `OtherModeH(x).cycle_type()` equals the current free-function decode.
- [ ] Commit: `types: OtherModeH/OtherModeL newtypes`.

### Task 3.5: Named structs for tuple returns

**Files:** `crates/fn64-abi/src/host.rs:681` (`(u64,u64,u64,u64)`) and `:1278`
(`(u64,u64,u64,u64,u64)`), `crates/fn64-rt64-characterization/src/rt64_rdp_state.rs:520`.

- [ ] Replace with `SessionPhaseTotals { submitted, retired, ... }` and
  `NormalizedPrimDepth { z, dz }`. ~40 sites.
- [ ] Commit: `types: name the census tuples`.

### Task 3.6: `DpcAckGuard` typestate

**Why:** `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1022` asserts at
runtime that an "atomic DPC transaction lost its acknowledgment owner before
validation." A move-only guard makes losing it a compile error.

- [ ] Transaction open returns `DpcAckGuard` (no `Clone`, no `Copy`,
  keyed by the existing `DpcTransactionId` from
  `crates/fn64-runtime/src/dpc_schedule.rs:12`); `validate(self, guard: DpcAckGuard)` consumes it. Delete the assert.
- [ ] Add a `compile_fail` doctest showing that validating without the
  guard does not compile, matching the pattern already used for
  `PreparedNativeFill` in `fn64-render-wgpu/src/lib.rs`.
- [ ] Commit: `abi: DpcAckGuard replaces the ack-owner assert`.

### Task 3.7: Discovery identity types

**Why:** `BankAddr.bank: String` (`crates/fn64-discover/src/facts/mod.rs:21`)
is the corpus identity key with 419 references; a typo is a silent miss.
`snapshot_workspace.rs:816` already invented `StrictBankAddr` in response.
ROM offsets mix physical and virtual spaces in bare `u32` at 81 signatures
(`banks/mod.rs:735`, `overlay_regions/mod.rs:282`, `aki_reference.rs:161`).

- [ ] Stage 1: `struct BankId(Arc<str>)` with `Deref<Target = str>` so
  existing string comparisons compile; construction only through
  `BankId::new` which validates against the bank table. Fold
  `StrictBankAddr` into `BankAddr`.
- [ ] Stage 2: `RomOffset<Physical>` and `RomOffset<Virtual>` via a
  `PhantomData` tag on the existing `RomOffset(u32)`
  (`loaders.rs:43`); the 81 signatures pick one. Conversion is one named
  method that takes the `RomAddressSpace`.
- [ ] Run the fn64-firewall skill after each stage; expected: identical
  grades (this is a type change, not a mechanism change).
- [ ] Commit series: `discover: BankId`, `discover: RomOffset spaces`.

### Task 3.8: `TriangleIndex` in the plan

**Why:** `production.rs:2027-2028` indexes `raw_triangle_commands[index]`
then `triangles[scheduled.triangle_index]`, a two-hop chain across sibling
Vecs, repeated at `:7821`, `:8400`, `:8998`, `:9017`. `PlanCollector` holds
six parallel command Vecs (`:3260-3266`).

- [ ] Introduce `struct TriangleIndex(u32)` and `struct CommandIndex(u32)`;
  the Vecs are indexed only through `impl Index<TriangleIndex>`.
- [ ] Fold `triangles` and `triangle_neutral_tiles` into one
  `Vec<PlannedTriangle>`.
- [ ] Commit: `render-wgpu: typed plan indices`. Do this before Task 4.2 so
  the split moves the typed version.

---

## Phase 4: structure

### Task 4.1: One `RdpDrawState`

**Why:** `PlanCollector`'s ten `current_*` fields (`production.rs:3095-3260`)
are mirrored field-for-field by `RawDpcCarryIn` (`:1639`), copied one at a
time in `seeded`, and re-listed as nine positional params in
`seeded_from_parts` (`:3361`). The code comments on itself as "a third
instance of the same seed-then-track pattern."

- [ ] Define `struct RdpDrawState { /* the ten fields */ }` with
  `fn apply(&mut self, cmd: &RawDpcCommand)` owning the six update sites.
  Embed it in both `PlanCollector` and `RawDpcCarryIn`. Delete `seeded_from_parts`.
- [ ] The existing tests that call the nine-argument constructor are
  rewritten to build an `RdpDrawState` literal; no behavior assertions
  change.
- [ ] Commit: `render-wgpu: RdpDrawState replaces three copies`. ~250 lines
  become ~90.

### Task 4.2: Split `production.rs`

**Why:** 22,397 lines; non-test code ends at line 9,741, tests run to the end.
Six concerns share the file: census and probes (`:92-1553`, `:1863-2206`),
planning (`:1554-1729`, `:3081-3720`), backend state (`:1730-1862`,
`:2366-2987`), guest-read capture (`:3721-3885`), execution and staging
(`:3886-7580`), color and compute scheduling (`:7581-8865`), command
execution (`:8866-9346`), IR conversion (`:9628-9742`).

**Skill:** rust-module-split, which holds the trap checklist from the 47
files already split.

- [ ] **Step 1:** Move the tests first: `production.rs:9742-22397` to
  `production/tests/{plan,execute,color,census}.rs`, matching the existing
  `targets/triangle_pipeline/tests.rs` convention. Zero logic change; PR 1.
- [ ] **Step 2:** Split non-test code into `production/{census,plan,state,capture,execute,color,convert}.rs`
  along the line ranges above. The seams already exist as traits:
  `PlanCollector: ExactRawDpcPlanVisitor`, `ExecutionCollector: RawDpcExecutionView`,
  `PhysicalExecutionCoordinator`. Add `ColorCommandScheduler` as a named
  struct for the `:7581-8865` range. One PR per two modules.
- [ ] **Step 3:** While moving `:8866-9346`, replace the `as usize` casts
  around wire-word and pixel-index arithmetic (103 in the file) with
  `usize::try_from(..)?` at the boundary, and `vec![usize::MAX; ..]` at
  `:19159` (now in tests) with `Vec<Option<u32>>`.
- [ ] **Gate per PR:** `cargo nextest run -p fn64-render-wgpu` identical
  pass count; `git diff -M --color-moved` shows moves, not edits, except
  for Step 3.

### Task 4.3: Collapse `complete_execution*`

**Files:** `production.rs:6055-6067` (becomes `production/execute.rs`), and
the two coordinator implementations that follow it.

- [ ] `enum ExecutionOutcome { Plain, PreservePhysical(PhysicalState), PreservePhysicalWithEffects(PhysicalState, Effects) }`;
  one `fn complete(&mut self, bound, outcome: ExecutionOutcome)`.
- [ ] Commit: `render-wgpu: one complete_execution`.

### Task 4.4: Shared WGSL bodies

**Why:** `coverage.wgsl` and `coverage_fragment_fn.wgsl` share 45 identical
lines; `alpha_compare.wgsl` and its `_fn` variant share 20 of 63. The split
is intentional (entry-point-free callables, asserted by tests at
`alpha_compare.rs:900` and `coverage/tests.rs:895`) but the copies can drift.

- [ ] Move the shared body to `coverage_body.wgsl`; `shader_manifest.rs`
  concatenates body plus a 5-line entry-point wrapper at build time. No
  naga-oil; a `concat!`-style join in Rust is enough.
- [ ] Rebaseline the manifest hashes in the same commit and say so in the
  message; the three digest-freeze printers in `shader_manifest.rs:1183,1272,2179` produce them.
- [ ] Commit: `render-wgpu: single-source WGSL bodies`.

### Task 4.5: The remaining files over 2,000 lines

- [ ] After 4.2, list with
  `git ls-files 'crates/*.rs' | xargs wc -l | awk '$1>2000' | sort -rn`.
  Expected survivors: `targets/texrect.rs` (8,236), `raw_dpc/mod.rs`
  (7,011), the conformance parity runner (6,759), `fn64-render/src/render_ir.rs`
  (5,846), `fn64-abi/src/task_dispatch/rsp_commit.rs` (3,540),
  `fn64-abi/src/frame_census.rs` (3,614), `fn64-discover/src/host_bindings/mod.rs`
  (4,046).
- [ ] One PR each, rust-module-split skill, tests-out-first as in 4.2.
- [ ] Do not split test files; a 4,659-line `tests.rs` is fine.

### Task 4.6: Move the inert RT64 characterization modules

**Why:** 62 `rt64_*` modules (~78.6k lines) are gated behind
`cfg(any(test, feature = "rt64-port-characterization"))`, cost nothing in a
default build, and carry parity evidence. They double the crate's compile
surface under `cargo test`.

- [ ] Only after Task 1.4 makes the oracle run in CI: move them to a
  sibling crate `fn64-rt64-characterization` depending on
  `fn64-render-wgpu`'s public primitives. Rewrite the `lib.rs` test that
  asserts exactly 62 and `scripts/lint-rt64-ports-inert.py` to the new
  location. Expect several modules to reach into crate-private items;
  make those `pub(crate)` items `pub` with a doc comment or keep the
  module in place and record why.
- [ ] Do not delete them. Do not do this before 1.4.

### Task 4.7: `unsafe` audit

**Why:** 1,132 `unsafe` blocks, 168 `// SAFETY` comments, 937 sites in
`fn64-abi`.

- [ ] **Step 1:** a new `lint-unsafe-safety.py` in `scripts/`: every `unsafe {`
  or `unsafe fn` in non-generated `crates/*/src` must have a `// SAFETY:`
  comment within the three preceding lines. Start it in warn mode listing
  the count; wire into the docs job.
- [ ] **Step 2:** In `fn64-abi`, group the 937 sites by pattern (RDRAM
  pointer arithmetic, `extern "C"` call, transmute of a recompiled fn
  pointer, static mut). For each pattern, one safe wrapper with one
  `SAFETY` comment, e.g. `RdramView::slice(addr: RdramAddr, len: usize) -> &[u8]`
  with the bound check inside. Most call sites become safe.
- [ ] **Step 3:** Flip the lint to fail once the count is zero.
- [ ] Commit series: `abi: safe RDRAM view wrapper`, `lint: SAFETY comments required`.

---

## Phase 5: tests

### Task 5.1: Incremental-vs-exact raster differential

**Why:** `raster_triangle_scalar` (`crates/fn64-render-wgpu/src/targets/raw_triangle.rs:930`)
steps attributes by `plane.dx` inside a run (`previous_sample`, `:1014-1057`) and re-evaluates the
exact formula on run breaks. No test asserts the two paths agree. This is
also the loop Task 6.1 rewrites, so the test must exist first.

**Files:** `crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs`,
`crates/fn64-render-wgpu/Cargo.toml` (`proptest = "1"` under dev-dependencies).

- [ ] **Step 1:** Write the failing test:

  ```rust
  proptest! {
      #[test]
      fn incremental_stepping_matches_exact_evaluation(
          tri in arbitrary_raw_triangle(),
          shade in arbitrary_attribute_planes(),
      ) {
          let mut incremental = target_buffer();
          let mut exact = target_buffer();
          raster_for_test(&mut incremental, &tri, shade, Stepping::Incremental);
          raster_for_test(&mut exact, &tri, shade, Stepping::Exact);
          prop_assert_eq!(incremental, exact);
      }
  }
  ```

  `Stepping` is a new `#[cfg(test)]`-only parameter threaded into
  `raster_triangle_scalar` through the existing `incremental_texture_planes: bool`
  argument, which already exists at `:946`; no production signature change.
- [ ] **Step 2:** Run it; if it fails, the failure is a real finding. Do not
  weaken the property. File it as a bug with the shrunken case.
- [ ] Commit: `render-wgpu: differential test for incremental attribute stepping`.

### Task 5.2: Property tests on three fixed-point kernels

- [ ] S10.5 coordinate conversion (`triangle_span` in `raw_dpc/mod.rs`):
  roundtrip and monotonicity.
- [ ] TMEM address computation including the odd/even T-parity branch in
  `raw_triangle.rs` (near `:990`): agrees with a direct transcription of the
  hardware formula from the public RDP documentation cited in
  `docs/RDP-SILICON-VECTORS.md`.
- [ ] One `fn64-cpu-runtime` FPU emitter (pick the one with the most
  `oracle.rs` cases): emitted-body result equals the reference C body's
  result for random operands, using the existing oracle harness.
- [ ] Commit: `tests: proptest on fixed-point kernels`.

### Task 5.3: Name the audio-priority join

**Why:** the load-bearing 2026-08-31 fix (bounded VI join, skip and
re-present on timeout) has no test that names it; `vi_join|audio_priority`
matches one incidental line in `fn64-abi`.

- [ ] In `crates/fn64-abi/src/task_dispatch/tests/`: a test where the render
  worker is a channel that never replies; assert the VI edge returns within
  the budget, `VI_JOIN_SKIPS` increments by one, and the previous field is
  re-presented (same digest). A second test where the reply arrives inside
  the budget asserts zero skips and the new field.
- [ ] Commit: `abi: name the audio-priority join contract`.

### Task 5.4: Testable shell entry

**Why:** `crates/fn64-shell/src/main.rs` is 1,976 lines with zero tests.

- [ ] Task 2.2 already extracts `cli.rs` and `Knobs`. Extract the remaining
  pure pieces of `main.rs` (banner, intake-contract message, event-loop
  pacing math from the `pump_one_frame` cadence fix noted in
  `docs/plans/perf-method.md`) into modules with unit tests. Target: `main.rs`
  under 300 lines, every extracted module tested.
- [ ] Commit series: `shell: extract <module>`.

### Task 5.5: Gates fail on unusable input

**Why:** `scripts/check-reference-frame-digests.py:48-60` silently exits 0
when `FN64_SHARD_ROOT` is unset and the fixture is absent. The project rule
(memory `gates-must-fail-on-unusable-input`) is that a gate that cannot
compare must exit nonzero.

- [x] The script exits 0 only when `--allow-missing` is passed; the CI docs
  job passes it explicitly with a comment saying the digest is not
  verified in a plain clone. Local runs fail loudly.
- [x] Audit the other `if env::var(..).is_err() { return }` early exits in
  tests (the test reviewer counted these as low, but list them) and convert
  each to `#[ignore = "needs FN64_X"]` so nextest reports them as skipped
  rather than passed. 17 converted (23 skipped vs. 6 before); 6 more found
  and deliberately left unconverted (subprocess child-reentry gates,
  per-ROM sub-steps inside a corpus loop, an already-`#[ignore]`d panic).
  Full inventory in `task-5.5-report.md`.
- [x] Add a new `lint-source-pins.py` under `scripts/`: for every test that
  does `include_str!("<file>")` followed by `.find("<needle>")` or
  `.contains("<needle>")`, verify each string-literal needle still occurs
  in the named file; fails naming the test and the missing needle. Wired
  into the docs CI job with a `--self-test` step first.
- [x] Commit: `gates: fail closed on missing input`.

### Task 5.6: Black-box runtime observation harness

**Why:** `AGENTS.md` allows exactly one way to learn how the GPL reference
runtime behaves: a differential experiment against it as a black box.
`docs/plans/runtime-parity-gap.md` (line ~401) prescribes the shape: same
inputs, same schedule, compare observables. fn64 has that mechanism for the
CPU (the mupen trace) but not for libultra shim behavior, so every question
about how a game observes the reference runtime is currently answered by
guessing or by not asking. Owner ruling 2026-09-05: source-derived
descriptions of that runtime are quarantined and never cited; observations
fn64 acts on come from this harness.

**Files:** driver lives OUTSIDE fn64 in the existing GPL checkout
`~/Code/aki-recomp` (already builds the runtime for its ports), for example
`~/Code/aki-recomp/tools/shim-probe/`. fn64 gains only:
`crates/fn64-abi/tests/blackbox/` holding the input scripts and the recorded
observation files (facts, JSON), plus one test that replays the same scripts
through fn64's shims and diffs.

**Design:** an observation is a scripted sequence of shim calls with
concrete arguments and a recorded result tuple (return register, output
memory bytes, messages delivered and in what order, wall-clock class where
relevant). The driver links the reference runtime unmodified, executes the
script, and prints the tuples. fn64's test executes the same script against
`fn64-abi` and reports each tuple as `match`, `deliberate-divergence` (with
the manual citation that justifies fn64's behavior), or `unexplained`. Only
`unexplained` fails the test.

- [ ] **Step 1:** Define the script format: one JSON file per scenario,
  `{ "calls": [ { "shim": "osSendMesg", "args": [...], "expect": {...} } ] }`.
  Scenarios for the first landing, chosen because WM2000 and OoT boot
  exercise them: message queue send/jam/recv ordering under blocking and
  nonblocking flags with two waiting threads of equal priority; osSetTimer
  with zero countdown and nonzero interval; osPiStartDma / osEPiStartDma
  completion-message timing relative to return; osContGetReadData on a port
  that reports no response; osAiSetFrequency return value for a
  non-hardware rate; osSetIntMask / __osDisableInt return register.
- [ ] **Step 2:** Write the driver in the GPL tree. It is GPL-licensed by
  necessity and is never copied into fn64. Record its commit hash in the
  observation file header.
- [ ] **Step 3:** Run it; commit the observation JSON into fn64 with a
  header naming the runtime commit observed (`cdf5abbd`, 2026-08-30), the
  driver commit, the date, and the command. These files are facts about a
  black-box run and carry no runtime code.
- [ ] **Step 4:** Write the fn64 replay test. For each divergence fn64
  intends, add the manual section that justifies it; anything else fails.
- [ ] **Step 5:** Wire the replay test into the normal nextest run (it
  needs no GPL code at test time, only the recorded JSON).
- [ ] Commit series: `abi: black-box shim observation scripts and replay test`.

**Gate:** every recorded tuple is `match` or `deliberate-divergence` with a
citation; the observation files name their provenance; no file under
`crates/` references the quarantined findings document or the GPL tree's
source.

---

## Phase 6: performance (after Task 1.1 and Task 5.1)

Both tasks follow `docs/plans/perf-method.md`: interleaved pairs, within-arm
spread reported, the program's outcome measured and not the targeted counter.

**Correction (2026-09-05, same day as the review):** the first draft of this
phase proposed monomorphizing `raster_triangle_scalar`'s per-pixel dispatch.
That task is withdrawn. `docs/plans/WM2000-30HZ-OPTIMIZATION-LOOP.md` already
ran the CPU-specialization class of experiment (prepared two-cycle
combining, incremental planes, rayon cutoff, subsample caching) and recorded
its stop condition: the CPU raster specialization budget is smaller than the
reliability gap. `docs/plans/WM2000-COMPUTE-RASTER.md` then built an exact
pixel-owned compute path that is byte-identical to the CPU raster and is
live-routed by default for its admitted program keys (`FN64_RAW_DPC_TASK_COMPUTE`
defaults on). The measured frontier is transport and batch boundaries, not
scalar pixel cost: widening admission under the current per-member transport
lost twice on live kill gates (+8.81 ms per drawn frame for the many-small-member
program; p95 43.4 to 45.3 ms for the next key). The one lever both plans
agree on is the same dependency question Task 6.2 asks. Task 5.1's
differential test stays: it guards the CPU oracle the compute path is
certified against.

### Task 6.1: Withdrawn (see correction above)

### Task 6.2: Prove the first consumer boundary, then keep the target resident

**Why:** the join at `LaterGraphics`/`DmemDependency` blocks guest time until
the render worker finishes because the next SP task may read RDRAM the batch
has not written. The compute-raster plan's "retained order" item 2 needs the
same fact for a different purpose: it may keep the packed RGBA16 target
device-resident, and read back once, only up to the first real guest or VI
consumer. One instrumented answer serves both.

- [ ] **Step 1:** Instrument only: for each join, record the next task's
  DMEM/RDRAM input ranges and the in-flight batch's `SetColorImage` extent
  and any `LoadBlock`/`LoadTile` source ranges; count overlaps versus
  non-overlaps over the 3,000-pump lane. If fewer than half the joins are
  non-overlapping, stop here and record the number.
- [ ] **Step 2:** If the count justifies it, skip the join when ranges are
  disjoint, keeping the existing join otherwise. Name the interleaving this
  closes in a comment at the site, per `AGENTS.md`.
- [ ] **Step 3:** Frame tripwire 120/120 byte-identical; audio underruns 0;
  interleaved four-run table.
- [ ] Commit: `abi: skip render join for disjoint task inputs (measured: ...)`.

---

## Phase 7: contract and narrative

### Task 7.1: Validation bars that measure what they claim

**Files:** `AGENTS.md` "Validation bars".

- [ ] Replace the two bullets with:

  ```markdown
  - Deterministic bug fix: a test that failed before and passes after,
    then 10 consecutive clean runs as a flake check when a run takes under
    a minute (3 runs if longer). Repetition detects hidden nondeterminism;
    it adds no correctness evidence beyond the first run.
  - Concurrency/race fix: name the exact interleaving your fix closes, in
    a comment at the fix site, and add a test that forces that interleaving
    (a barrier, a blocking channel, or loom). Use 20+ stress runs only when
    the interleaving cannot be forced, and say so in the comment. Twenty
    clean runs rule out only races that fire on more than about one run
    in six.
  ```

- [ ] Commit: `docs: validation bars state what repetition proves`.

### Task 7.2: The renderer thesis

**Why:** `docs/ROADMAP.md` still says "RT64 as the faithful renderer, wgpu
port deferred to Phase P"; `README.md:109` calls `fn64-render-wgpu` "the
bounded M3.1 headless submission/readback lifecycle fixture." The live
WM2000 path is `fn64-render-wgpu`: an exact CPU rasterizer as the oracle
and fallback, plus a byte-identical compute-raster path for admitted
program keys, both writing guest RDRAM. RT64 is the parity oracle.

- [ ] Rewrite the README crate table row and the "Why fn64" render line to
  say: `fn64-render-wgpu` is the production renderer (exact CPU raster plus
  an exact compute path for admitted keys; the RGBA8 triangle render
  pipeline is diagnostic-only); `fn64-render-rt64` is the
  parity oracle, run nightly in CI (Task 1.4).
- [ ] Rewrite the ROADMAP "Render endgame" paragraph to match, dated.
- [ ] Commit: `docs: renderer thesis matches the code`.

### Task 7.3: Consolidate the RT64 docs

**Why:** 65 of 120 docs carry the `RT64-` prefix; most are per-slice evidence
from the port program.

- [ ] Create `docs/rt64/` and move every `RT64-*.md` and `rt64-*.json` there
  with `git mv`. `scripts/lint-docs.py` follows references, so the move is
  verified by the lint.
- [ ] Write a `README.md` inside the new `rt64/` docs directory: one line per doc, grouped as
  *authority and method* (PORT-AUTHORITY, PARITY, ENGINEERING-LOOP),
  *status* (PORT-DASHBOARD, PORT-INVENTORY, GAP-REGISTER), and *evidence*
  (everything else). Mark evidence docs as frozen with their date.
- [ ] Retire `HANDOFF.md` and `R5-HANDOFF.md` into
  a dated `docs/plans/HANDOFF-2026-08-15.md` next to the existing dated handoff.
- [ ] Commit: `docs: rt64/ directory with index; retire root handoff`.

### Task 7.4: Knob docs

- [ ] After Task 2.2, `RT64-RUNTIME-CONTROLS.md` (moved under
  the `rt64/` docs directory by Task 7.3) is replaced by the generated `docs/RUNTIME-KNOBS.md` from
  Task 2.1 plus `fn64 --help` output. Delete the hand-written one.

---

## Sequencing summary

| Order | Tasks | Parallelizable? |
|---|---|---|
| 1 | 0.1, 0.2 | 0.2 after 0.1 merges |
| 2 | 0.3, 0.4 | yes, with each other |
| 3 | 1.1, 1.2, 1.5 | yes |
| 4 | 1.3, 1.4 | yes |
| 5 | 2.1, then 2.2 | 2.3 and 2.4 parallel with 2.2 |
| 6 | 3.1 to 3.8 | one crate per agent; 3.8 before 4.2 |
| 7 | 4.1, then 4.2, then 4.3, 4.4 | 4.5 after 4.2; 4.6 after 1.4; 4.7 anytime |
| 8 | 5.1 to 5.6 | yes; 5.6 needs the `~/Code/aki-recomp` GPL tree on the executing machine |
| 9 | 6.1, 6.2 | 6.1 first |
| 10 | 7.1 to 7.4 | yes; 7.4 after 2.2 |

## Self-review against the 2026-09-05 findings

- Main is stale, perf work stranded: 0.1, 0.2, 0.3.
- No release profile: 1.1. Warnings and future-incompat: 1.2, 1.3.
- Oracle never in CI, no property tests, untested shell and join: 1.4, 5.1
  to 5.4. Silent gate skip: 5.5.
- 288 env vars, no CLI, 51 bins, god-trait: 2.1 to 2.4.
- Ten type findings: 3.1 (errors), 3.2 (RdramAddr, UnsupportedUcode), 3.3
  (PhysicalAddress, bool param), 3.4 (OtherMode), 3.5 (tuples), 3.6
  (typestate), 3.7 (BankId, RomOffset), 3.8 (indices).
- Ten RT64-port findings: 2.4 (#1), 4.1 (#2), 4.2 (#3, #10), 4.3 (#4), 3.8
  (#6), 4.6 (#7), 2.2 (#8), 4.4 (#9). #5 (`pack_*` functions) is
  deliberately omitted: the module is inert and the reviewer rated it
  low-value until wired.
- Unsafe density, no dependency policy: 4.7, 1.5.
- Raster loop dispatch and join stall: 6.1, 6.2.
- Validation bars, stale thesis, 65 RT64 docs, dirt: 7.1, 7.2, 7.3, 0.4.
