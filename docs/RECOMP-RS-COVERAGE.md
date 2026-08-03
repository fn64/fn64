# fn64-recomp-rs whole-ROM coverage baseline

This is a living, reproducible measurement of how much of a real N64 ROM the
all-Rust recompiler `fn64-recomp-rs` recompiles today, and — more importantly —
what the *remaining* gap actually is. It answers "how done is the runtime
replacement" with numbers from live ROM data, not prose.

The measurement is produced by the whole-ROM driver
`crates/fn64-recomp-rs-codegen/src/bin/recompile_rom.rs`, which runs the recompiler
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

The arbitrary-PC block/interpreter lane is where these cases belong: its AOT
emitter renders `break`/`syscall`/traps as architectural exceptions and `eret`
as a typed control transfer (see `crates/fn64-recomp-rs/src/execution/mod.rs`,
`crates/fn64-recomp-rs-codegen/src/emit/mod.rs`, and the `ISA-COVERAGE.md` C/P/T/R audit).
Promoting that lane to the default whole-ROM execution path — not retrofitting
an exception-return ABI into the whole-function lane — is the work that turns
"recompiles OoT" into "runs arbitrary N64 ROMs."

That promotion has several pieces, being landed incrementally:

- **Driver exception vectoring (done).** The block-lane driver
  `run_block_program` (`crates/fn64-abi/src/recompiled/mod.rs`) now vectors a
  `BlockExit::Fault` carrying an architectural exception through the installed
  handler, exactly like the executable-write boundary; previously only
  executable-write faults were vectored and a mid-function `break` panicked the
  driver.
- **Whole-ROM snapshot composition (done).** Composition accepted only
  physically-resident banks, so OoT composed one bank and its 468 DMA-loaded
  (VROM) overlays were filtered out. Byte-verification now runs through
  `materialize_rom_range`, and a load-time `.bss` tail is accepted by composing
  the ROM-backed prefix. OoT now composes the resident bank plus all 468
  overlay banks. The old `unsupported=8` observation predated the retained
  whole-ROM closure run and is withdrawn; that later historical run measured
  568, and snapshot schema v2 now requires another ROM-bearing regeneration
  before any count is attributed to current HEAD.
- **Interpreter trap arms (done).** The interpreter
  (`crates/fn64-recomp-rs/src/interp.rs`) now raises `break`/`syscall` and the
  twelve conditional traps as architectural exceptions, matching the AOT lane
  word-for-word. This stopped being deferrable once a whole-ROM compose showed
  ~12k destinations (`dynamic_mips`, 96% of it `proven_code_no_owner`) reaching the
  interpreter fallback. COP2 words remain a deliberate loud fault: COP2 is the
  RSP vector unit, not CPU-accessible on N64, and the AOT lane traps it too.
- **Whole-ROM `BlockProgram` emitter (partially proven).** The WM2000 gate now
  emits, compiles, and mechanically probes all five discovered banks, so the
  former single-bank/synthetic-only claim and globally unmeasured rustc claim
  are obsolete. OoT and SM64 still lack the corresponding emitted-pack boot
  evidence; WM's successful five-bank compile does not close their default
  wiring or source/writer frontiers.
- **CPU instruction-store selected-build audit (structural only).** The canonical ABI
  owner can now arm a process-unique, move-only fresh epoch and retain ordered
  post-commit CPU RDRAM store ranges. A successful take requires an exercised
  store path, a quiescent mutation owner, exact live watched bytes, the
  production-AOT feature lane, and ABI-issued host-catalog authority. This
  prerequisite now feeds a selected-build exact-ten runtime series. The
  verifier-owned writer-audit bundle can atomically project that CPU authority,
  alongside its represented Bootstrap/HostAbi/PI/RDP/RSP/SI/SP authorities, into the
  fixed writer denominator. No private run has been performed, so the CPU row
  remains open in production evidence.
- **PI DMA selected-build audit (structural only).** The canonical ABI owner's
  fresh, process-unique PI epoch now feeds the verifier-owned exact-ten
  selected-binary protocol. Each report binds the balanced PI lifecycle,
  device-to-RDRAM commit, watched-byte/journal state, catalog receipt, program
  model, and production-AOT build; the one-build bundle retains that series as
  its PI bit and can atomically project it into the fixed writer denominator.
  A private ten-run series remains open, so this does not close the PI row in
  production evidence.
- **Host ABI selected-build audit (structural only).** The canonical ABI owner
  now retains declared host-ABI executable writes in a fresh, model-bound
  runtime epoch. Its exact-ten selected-build series can join the verifier-owned
  writer-audit bundle and atomically project the represented HostAbi row into
  the fixed denominator. No private ten-run series has been performed, so the
  HostAbi row remains open in production evidence.
- **RDP renderer selected-build audit (structural only).** The selected runner
  arms the ABI-local renderer epoch immediately before guest/device scheduling.
  Only a backend-committed publication with at least one canonical
  `RdpRenderer` executable-journal entry can produce its strict nonce-bound
  report; `NeedsLle`, a framebuffer-only publication, or a malformed mutation
  trace cannot. Ten distinct, nonce-excluded-identical reports mint the
  move-only selected-build series, which can join the one-build writer-audit
  bundle and atomically project its represented RDP row into the fixed
  denominator. No private ten-run series has been performed, so the RDP row
  remains open in production evidence.
- **RSP execution/HLE selected-build audit (structural only).** The selected
  runner chooses graphics LLE, arms the ABI-local RSP epoch immediately before
  scheduling, and accepts only explicit pending-owner/no-writeback states as
  transient. Its strict nonce-bound report recomputes the receipt, build,
  program model, catalog, journal, watched image, and ordered writeback trace.
  Ten distinct nonce-excluded-identical reports mint the move-only RSP series
  and bundle bit, which can atomically project its represented RSP row into the
  fixed denominator. A data-only interpreter writeback is a valid typed-path
  exercise even when the executable-clipped journal count is zero. No private
  ten-run series has been performed, so the RSP row remains open in production
  evidence.
- **Default wiring (open).** `FN64_RS_EXECUTION=block` remains opt-in until the
  above land and both OoT and SM64 boot through the block lane with zero
  gap-panics.

## C shell-out retirement

The former `n64recomp` adapter in `fn64-recomp` (which serialized configs to
N64Recomp/RSPRecomp TOML and shelled out to the pinned fork's binaries) has been
removed: once `fn64-recomp-rs` became the in-tree recompiler, the adapter had no
in-tree consumer. The pre-generated-C CI oracle lane compiled by the
`fn64-boot-harness`/`fn64-shell` build scripts is a separate mechanism and is
retained as an independent cross-check until the block lane is proven to run
whole ROMs.
