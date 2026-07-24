# fn64-recomp-rs whole-ROM coverage baseline

This is a living, reproducible measurement of how much of a real N64 ROM the
all-Rust recompiler `fn64-recomp-rs` recompiles today, and — more importantly —
what the *remaining* gap actually is. It answers "how done is the runtime
replacement" with numbers from live ROM data, not prose.

The measurement is produced by the whole-ROM driver
`crates/fn64-recomp-rs/src/bin/recompile_rom.rs`, which runs the recompiler
over EVERY function in a ROM without bailing on the first bad one, classifies
each function, emits the clean bodies as a standalone Rust crate, and writes a
`gap-report.md`. ROMs and their N64Recomp configs are game-derived content and
live out of tree; the driver only ever reads their paths from args/env.

## Reproduce

```text
cargo build --release -p fn64-recomp-rs --bin recompile_rom
FN64_CONFIG=<game>.toml FN64_ROM=<game>.z64 FN64_OUT=<dir> \
  ./target/release/recompile_rom
# gap report: <dir>/gap-report.md
```

## Baseline (2026-07-24)

| ROM | Functions | Clean | Host-bound (`break`/`eret`) | Config stubs | Decoder/emitter gaps we own |
|---|---:|---:|---:|---:|---:|
| Ocarina of Time (NTSC 1.0) | 13358 | 13335 (99.83%) | 14 | 9 | **0** |
| Super Mario 64 (USA) | 3893 | 3844 (98.74%) | 40 | 9 | **0** |

"Clean" means the function fully recompiled to typed Rust and linked. "Stubs"
are functions the N64Recomp config deliberately excludes. "Decoder/emitter gaps
we own" counts functions that fail because our recompiler cannot decode or emit
some instruction — the number that would represent genuinely missing ISA
coverage.

## The one honest conclusion

**The remaining gap is not instruction coverage — it is a single architectural
limitation.** Both ROMs show zero decoder/emitter gaps we own: the MIPS III /
VR4300 instruction surface the recompiler emits is complete for real code. The
non-clean functions are all "host-bound" — the whole-function lane emits a
`panic!` for a mid-function privileged or trapping instruction (`break`,
`eret`) because a callable `fn` cannot return a control transfer, so the whole
body is omitted and routed through the host resolver instead.

- On OoT the host-bound set is almost entirely libultra/OS entries
  (`osSendMesg`, `osRecvMesg`, the `__ll_*`/`__ull_*` math helpers,
  `__osException`), which are correctly bound to `fn64-abi` shims — plus one
  `eret` function, `__osDispatchThread`.
- On SM64 the same limitation hits **39 ordinary gameplay functions**
  (`move_into_c_up`, `approach_s16_asymptotic`, `next_lakitu_state`,
  `render_painting`, `spawn_sparkle_particles`, …) that contain a
  compiler-emitted assertion `break`, plus the same one `eret`
  (`__osDispatchThread`). These are real game logic, not OS shims.

So a recompiler that is "99.83% clean on OoT" is not a general N64 runtime yet:
the moment a ROM's game code contains mid-function `break`/`eret` (SM64 does,
heavily), the whole-function lane omits real code.

## What closes it

The arbitrary-PC block/interpreter lane already handles exactly these cases:
`break` becomes `CpuException::Breakpoint` and `eret` becomes a typed control
transfer, vectored through the installed handler rather than panicking (see
`crates/fn64-recomp-rs/src/execution.rs`, `crates/fn64-recomp-rs/src/interp.rs`,
and the `ISA-COVERAGE.md` C/P/T/R audit). That lane is currently opt-in
(`FN64_RS_EXECUTION=block`). Promoting it to the default whole-ROM execution
path — not retrofitting an exception-return ABI into the whole-function lane —
is the work that turns "recompiles OoT" into "runs arbitrary N64 ROMs." It is
tracked separately from this baseline.

## C shell-out retirement

The former `n64recomp` adapter in `fn64-recomp` (which serialized configs to
N64Recomp/RSPRecomp TOML and shelled out to the pinned fork's binaries) has been
removed: once `fn64-recomp-rs` became the in-tree recompiler, the adapter had no
in-tree consumer. The pre-generated-C CI oracle lane compiled by the
`fn64-boot-harness`/`fn64-shell` build scripts is a separate mechanism and is
retained as an independent cross-check until the block lane is proven to run
whole ROMs.
