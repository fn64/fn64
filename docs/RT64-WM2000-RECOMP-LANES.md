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

**The `rs` lane cannot run WM2000 today.** The blocker is not the recompiler —
the recompiler is fine, measured below. It is that no rs-lane *harness* exists
for this title. Three things are missing, and each is a named artifact rather
than a research question.

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
