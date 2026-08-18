# WM2000's recompiler lanes: which one every measurement came from

fn64 has two CPU-recompiler lanes. Every WM2000 measurement this project holds
— the opcode census, the cycle-mode probe, the 366-command captured packet, the
4,454-VI-swap window, and therefore the three-way 0-differing-pixel result —
was produced by **one** of them. This doc says which, measures what the other
lane would need, and records the stream diff that could not be produced.

Companion docs: [`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md),
[`RT64-WM2000-THREE-WAY.md`](RT64-WM2000-THREE-WAY.md),
[`DECOUPLING.md`](DECOUPLING.md), [`PARITY-METHOD.md`](PARITY-METHOD.md).

---

## 1. Headline: every WM2000 measurement is C-lane; the rs lane has never run this title

**The `rs` lane cannot run WM2000 today** — but the reason changed on
2026-08-18, and §6 supersedes this section's diagnosis. The three missing
harness artifacts named below have been built, the rs lane now compiles and
*runs* WM2000, and it fails at a different, deeper place: a stubbed function
the C lane papers over with an empty body. §2.1's "the recompiler is not the
blocker" remains true as a codegen statement and remains insufficient as a boot
statement.

**The C lane is therefore the only lane that has produced WM2000 evidence, and
the shipping path is unvalidated on this title.**

---

## 2. The two lanes, and what each needs

`crates/fn64-shell/build.rs:53` is where the lanes fork.

| | `c` lane (default) | `rs` lane (`FN64_RECOMP=rs`) |
|---|---|---|
| Game source | N64Recomp-generated `RecompiledFuncs/*.c` | emitted typed-Rust whole-ROM crate |
| Env intake | `RECOMPILED_DIR` + `RECOMP_H_DIR` + `ROM` | `RECOMP_RS_DIR` (symlinked to `rs/recompiled`) + `ROM` |
| Selected by | absence of `FN64_RECOMP=rs` | `build.rs:53`, sets `fn64_cpu_runtime` + `fn64_game_linked` |
| Section bridge | `register_linked_sections()` over C FFI | `recompiled::RECOMPILED_SECTION_GEOMETRY` |
| Host binding | N64Recomp's own symbol list | a hand-written per-game vram table |

`examples/wm2000-census/build.rs` has **no `FN64_RECOMP` branch at all** — it
reads `RECOMPILED_DIR`/`RECOMP_H_DIR`/`ROM` unconditionally. The census harness
is structurally incapable of producing an rs-lane capture; there is no env var
that would switch it.

### 2.1 What the rs lane needs for WM2000, measured

**(a) The recompiler is NOT the blocker.** Run on this machine against
WM2000's own config and ROM:

```sh
recompile_rom --config aki-recomp/games/NWXE/wm2000.toml \
              --rom    aki-recomp/games/NWXE/wm2000.z64 --out <scratch>
```

```
total functions: 2442
  clean              2414  (98.85%)
  runtime-trap          3  ( 0.12%)
  stubbed              25  ( 1.02%)
linkable (recompiled + host-bound): 2417 (98.98%)
```

**Zero unknown-opcode functions and zero ROM-range failures** — the gap report's
"genuine gaps in OUR recompiler: 0". The three runtime-traps are
`osAiSetFrequency`/`osRecvMesg`/`osSendMesg`, all libultra entries that bind to
`fn64-abi` shims, the same host-bound class OoT shows. This matches
[`CPU-RUNTIME-COVERAGE.md`](CPU-RUNTIME-COVERAGE.md)'s finding on OoT and SM64:
the remaining gap is architectural, not instruction coverage.

The emit is **deterministic**: two full runs produced byte-identical `src/`
trees (`diff -r` clean; 64 parts, 41,924,390 bytes aggregate).

**(b) No rs-lane harness exists for WM2000.** The game harnesses were extracted
out of this repo by `269f5415` into `~/Code/recomps/wm2000/packages`. Of the ten
packages there, **only `oot-boot` has an `rs/` manifest**. `wm2000-boot` and
`wm2000-block-boot` contain no `FN64_RECOMP`, no `fn64_cpu_runtime`, and no
`RECOMP_RS_DIR` — measured by grep over both `build.rs` files.

**(c) `fn64-shell`'s rs lane is hardcoded to OoT.** Three bindings, none
game-neutral:

- `crates/fn64-shell/src/main.rs:126` — `use oot_recompiled as recompiled;`
- `crates/fn64-shell/rs/Cargo.toml:53` — `oot-recompiled = { path = "recompiled" }`
- `crates/fn64-shell/src/main.rs:108-110` — `#[path = "../../../examples/oot-boot/src/host_lookup.rs"] mod host_lookup;`

That third path **does not exist in this repo**: `examples/` contains only
`wm2000-census`. The file lives at
`recomps/wm2000/packages/oot-boot/src/host_lookup.rs` (107 lines). So
`FN64_RECOMP=rs` cannot compile in this tree at all, for any title.

`host_lookup.rs` is the substantive missing artifact, not the include path. It
is a hand-written table of **68 OoT vram → `fn64-abi` adapter bindings**
derived from OoT's decomp symbol dump. WM2000 would need its own: its
`syms/dump.toml` names **34 libultra symbols** (28 `os*` plus six `__os*`/`__ll*`),
each needing a vram→adapter row. The rs lane resolves host functions by
*address*, so this table cannot be inherited or inferred from the C lane.

**(d) The extracted harness repo is pinned to a pre-rename fn64.**
`recomps/wm2000` depends on `fn64-recomp-rs` / `fn64-recomp-rs-codegen`, which
`20c3f7c3` renamed to `fn64-cpu-runtime`. That rename is an ancestor of this
commit but **not** of the main checkout's `HEAD` (`f2549cbc`), so the harness
repo resolves against `/Users/jer/Code/fn64` and would not build against this
worktree without a rename sweep.

### 2.2 The ROM question is NOT a blocker

`build.rs:54-57` warns the rs lane's `ROM` must be "the decomp's OWN
decompressed build-output z64 — NOT the retail compressed cartridge image."
That warning is about OoT's decomp workflow. It does not bind WM2000: the rs
recompiler consumed `wm2000.z64` (33,554,432 bytes — the same retail image the
C-lane census uses) directly and emitted 98.85% clean. WM2000's resident image
is an affine boot bank, not a compressed one.

---

## 3. The stream diff: not produced, and why

**No rs-lane RDP command stream exists for WM2000, so there is no diff.** This
section states that plainly so nobody later reads §4's numbers as a comparison.
That is still true after the 2026-08-18 harness work: the lane now runs, but it
traps before any RDP command is emitted (§6).

What was ruled out as the cause: the recompiler (§2.1a, 98.85% clean, 0 gaps),
the ROM format (§2.2), and instruction coverage. What actually blocks it is
harness plumbing — a WM2000 `host_lookup.rs`, an rs manifest for a WM2000
harness, and an `FN64_RECOMP` branch in the census harness.

**UNKNOWN: whether the two lanes emit identical RDP streams.** Nothing here
measures that, in either direction. `scripts/lane-parity.sh` exists and does
exactly this comparison — but only for OoT, and its own header records that
default mode *rejects* the legacy C corpus as an arbiter from swap zero because
callable empty C bodies have nonempty Rust counterparts.

---

## 4. The two N64Recomp repairs are C-lane-only — measured

Both recent WM2000 fixes repair defects in N64Recomp's *generated C text*. They
are structurally unreachable from the rs lane.

**The epilogue mender.** `prepare_recompiled_cxx_sources_with_proven_fallthrough_repair`
is called at `crates/fn64-shell/build.rs:129`, which is **after** the
`FN64_RECOMP=rs` early `return` at `build.rs:61`. The rs lane never calls it.
It operates by scanning `RECOMPILED_DIR/*.c`; with no generated C, there is
nothing to mend.

**The 40 `static_<section>_<vram>` registrations.** The receiving callback
`fn64_register_section_local_func` (`crates/fn64-boot-harness/src/lib.rs:1571`)
is `extern "C"` behind `#[cfg(feature = "c-bridge")]`, as is
`register_linked_sections`. The rs lane registers sections from
`recompiled::RECOMPILED_SECTION_GEOMETRY` instead
(`crates/fn64-shell/src/main.rs:328`).

**Measured, not asserted — the rs lane does not have this defect.** WM2000's
generated C defines 2,449 `RECOMP_FUNC` bodies, of which **40** are
`static_<section>_<vram>`. The rs emit contains **zero** symbols of that shape.
All 40 of those vrams are nonetheless present in the emitted Rust — checked
one by one, **0 of 40 missing**. They appear as ordinary branch targets inside
their enclosing functions (e.g. `0x8011FFA4`: 5 references) rather than as
separate callable bodies, because the rs recompiler derives functions from the
symbol dump rather than from N64Recomp's file-local symbol splitting.

So the defect the repair fixed — a body carrying the entry observer while
appearing in no `FuncEntry` table — has no rs-lane analogue to fix.

---

## 5. Which lane every existing measurement came from

**All of them: the `c` lane.** Every WM2000 figure in `docs/` traces to
`examples/wm2000-census` (or the extracted `wm2000-boot`), whose documented
invocation sets `RECOMPILED_DIR="$HOME/Code/aki-recomp/games/NWXE/RecompiledFuncs"`.

| Measurement | Doc | Lane |
|---|---|---|
| 142,606 commands; 84.0% admitted; 0 GBI-lane | `RT64-WM2000-CENSUS.md` | **c** |
| Cycle-mode probe | `RT64-WM2000-CYCLE-MODES.md` | **c** |
| 366-command decode entry 0 packet | `RT64-WM2000-REPLAY.md` | **c** |
| 4,454 VI swaps; 5,406,193 RDP commands | commit `a22762f3` | **c** |
| 0 of 115,200 pixels differ, three ways | `RT64-WM2000-THREE-WAY.md` | **c** (consumes the packet above) |

The three-way pixel result compares three *renderers* against one captured
command stream. That stream is a C-lane artifact. **The result therefore does
not transfer to the shipping rs lane** — not because it is wrong, but because
the transfer premise (identical streams) is the unmeasured question in §3.

---

## Nonclaims

- **No claim that the streams differ.** No rs-lane WM2000 stream exists; §3 is
  a missing measurement, not a negative result.
- **No claim that the streams are identical.** The renderer ports are not shown
  to be validated on the shipping lane, nor shown to be invalid.
- **No claim the rs lane would boot WM2000** if the three artifacts in §2.1
  were supplied. 98.85% clean recompilation is a codegen result; it is not a
  boot, and the host-bound trio and 25 config stubs still need host bindings.
- **No claim about the block lane.** `wm2000-block-boot` is a third
  configuration (dense AOT shards over `fn64-discover`); it was not exercised
  here and its relationship to the C/rs split is unmeasured.
- **No renderer, residency, or pixel claim.** Nothing here re-measures any
  census figure; §5's table cites existing docs rather than re-deriving them.


---

## 6. 2026-08-18: the harness was built, and the rs lane does not boot WM2000

The three artifacts §2.1 named as missing now exist. The rs lane compiles,
links, and executes WM2000 — and then traps, deterministically, on a function
the C lane silently no-ops. **That trap, not the harness, is now the blocker.**

### 6.1 What was built

| Artifact | Where | Note |
|---|---|---|
| WM2000 `host_lookup.rs` | `recomps/wm2000/packages/wm2000-boot/src/` | 31 rows + 7 guard tests |
| `wm2000-boot` rs sibling | `recomps/wm2000/packages/wm2000-boot/rs/` | `FN64_RECOMP=rs` branch in `build.rs`, rs wiring in `main.rs` |
| Title-neutral shell rs lane | `crates/fn64-shell/{build.rs,src/main.rs}` | replaced the hardcoded OoT `#[path]` include |

`crates/fn64-shell/src/main.rs`'s `#[path = "../../../examples/oot-boot/src/host_lookup.rs"]`
named a file that does not exist in this repo, so `FN64_RECOMP=rs` could not
compile here for **any** title. It is now `include!(env!("FN64_HOST_LOOKUP_PATH"))`,
resolved by `build.rs` from `RECOMP_RS_HOST_LOOKUP`. **No default is baked in**:
the rs lane binds host functions by *address*, so silently defaulting to another
title's table would resolve a wrong-but-plausible set and produce wrong
behaviour with no error. The path dependency `oot-recompiled` was likewise
renamed to `game-recompiled`.

### 6.2 The host_lookup mapping — 31 rows, mechanically derived

WM2000's `syms/dump.toml` names **34** libultra symbols. The mapping was
generated by joining that dump against `fn64-abi`'s adapter surface
(`crates/fn64-abi/src/recompiled/runners.rs`) on identical libultra name, not
typed by hand: **31** have an identically-named adapter, **3** do not.

The three unmapped — `__osSiRawReadIo` (`0x800377F0`), `__osSiRawWriteIo`
(`0x80037840`), `osUnmapTLBAll` (`0x800379D0`) — are **not** aliased onto a
plausible neighbour. Two measured facts say leaving them to their emitted guest
bodies is correct rather than a gap: the C lane does not host-bind them either
(its `recomp_overlays.inl` has *no* `FuncEntry` at those three vrams, versus 28
`*_recomp` entries for the rest), and their emitted bodies reach real modelled
hardware — the two SI ones load/store through `0xA0000000|addr`, which
`Rdram::load_w` routes to `fn64-abi`'s PIF window model (an unmodelled device
address traps loudly), and `osUnmapTLBAll`'s `mtc0`/`tlbwi` land in
`RecompContext::tlbwi_record`, a real TLB write.

An address with no adapter returns `None`, so the emitted dispatcher owns it and
an genuinely-unknown vram hits `trap_unsupported` by name.

**Mutation-tested, 4 mutants, all killed.** The first attempt caught only 3: a
row pointing at the *wrong* adapter (`os_vi_swap_buffer` → `os_vi_black`)
survived, which is exactly the silent-mismapping danger. A test that compares
resolved function pointers against the dump-named adapter closes it; re-run
under mutation, it now fails with "resolved to the WRONG adapter".

### 6.3 The boot result — a precise blocker

```
[wm2000-boot] registered 6 recompiled section geometries; marked 0/1 resident
[wm2000-boot] FN64_RECOMP=rs: linked emitted crate + host-first adapters active;
              generated_source_sha256=<emitted-artifact digest, printed at boot>
[wm2000-boot] thread 0 (recomp_entrypoint) returned at step 3 -- expected
panicked: lookup: no recompiled function or host shim at vram 0x80022540
```

Byte-identical across two consecutive runs.

`0x80022540` is `func_80022540`, identified in `games/NWXE/profile.toml` as
**`osDriveRomInit`** — a 64DD drive probe whose return value the boot caller
discards. It is in `wm2000.toml`'s `[patches].stubs`, so fn64's recompiler
deliberately omits its body and a call to it traps by name.

**This is a lane-disposition divergence, not a codegen defect.** Measured over
the 60 effective config stubs: the C lane emits **57 empty callable bodies** —
functions that are callable, do nothing, and return silently — where fn64's
recompiler emits nothing and refuses. The C lane "boots past" `osDriveRomInit`
because its stub is a silent no-op. This is the same hazard
`scripts/lane-parity.sh` already refuses the legacy C corpus over.

**Scope: 25 trap sites, not one.** 24 effective stubs are direct
`call_host_or_recompiled` targets in the rs emit (one, `func_80140570`, at 27
call sites); `func_80022540` is reached through `lookup(0x80022540)`. Fixing the
first address would surface the next. `fn64-abi` has **no `osDriveRomInit`
adapter** — grep over `crates/` finds the name only in discovery testdata and
comments.

Whether the right resolution is host adapters for the reachable stubs, or
force-recompiling those with benign bodies (the mechanism `profile.toml`'s
`force_recompile` already uses for 96 functions), is **not decided here**.

### 6.4 The C-lane capture stays the pinned artifact

The owner's standing direction is that the rs lane becomes the standard and the
C lane is retained only as a stored capture that a one-time wire-level
regression gate consults. **That gate is not constructible for WM2000 yet**:
the rs lane emits no RDP command before it traps, so there is no second stream
to diff. The C-lane capture therefore remains the *only* WM2000 stream, and
every WM2000 number in `docs/` (§5's table) is still C-lane. Pinning it as a
stored artifact is a real next step; it is not done here, and nothing in this
card produced an rs-lane stream to pin beside it.

## Nonclaims (2026-08-18 addendum)

- **No claim the rs lane boots WM2000.** It does not. It reaches
  `osDriveRomInit` and refuses.
- **No claim the C lane is right and the rs lane wrong.** The C lane proceeds
  because its stub is an empty callable body; that is a silent no-op, which
  AGENTS.md forbids. Which disposition yields *correct* behaviour at each of
  the 25 sites is unmeasured.
- **No RDP stream, no wire diff, no pixel result on the rs lane.** The lane
  traps before emitting a command, so steps 5 and 6 of this card were not
  reachable. The C-lane capture remains the only WM2000 stream artifact.
- **No claim about the 3 unmapped libultra symbols at runtime.** Their
  dispositions are argued from the C lane's table and from what their emitted
  bodies call; neither was observed executing, because boot stops earlier.
- **No claim the shell's rs lane runs.** `fn64-shell`'s rs manifest now
  *compiles* against WM2000's table (measured — the first time it has compiled
  for any title in this tree, since its `#[path]` target was a deleted file).
  It was not booted, and it would hit §6.3's trap if it were.
- **No block-lane claim.** `FN64_RS_EXECUTION=block` is refused by name for
  WM2000 (it needs a `block_program_pack` and an `FN64_BOOT_CONTEXT` capture).
