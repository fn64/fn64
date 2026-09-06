# Recompiler parity method

Status: live verification contract. This document covers the `FN64_RECOMP=c`
versus `FN64_RECOMP=rs` comparison. It does not cover `DESIGN.md` §4's separate
runtime-provider swap over one byte-identical generated-C archive.

## Authority comes before output

Two boot lanes producing equal framebuffers is a semantic differential only
when both generated programs contain the same callable behavior. The legacy
OoT C corpus does not meet that precondition: its bootstrap stub policy emits
callable functions with no translated instructions, while the Rust driver
recovers many of those functions from the same ROM.

`scripts/lane-parity.sh` therefore audits the generated artifacts before it
builds either boot binary. `crates/fn64-cpu-runtime/tests/lane_authority.rs`:

1. extracts every `RECOMP_FUNC` C body and every emitted Rust function body;
2. classifies a generated body by the instruction PCs recorded by each
   recompiler in its output;
3. rejects authority if a callable empty C body has a nonempty Rust
   counterpart; and
4. compares the exact unique instruction-PC set of every shared nonempty
   function.

The check is name- and instruction-address-based, not a source-text or binary
hash comparison: C and Rust deliberately spell the same operation differently.
An equal PC set proves that both code generators cover the same source
instructions for that function. It does not prove each emitted operation has
the same semantics; `fn64-cpu-runtime`'s instruction/oracle suites own that
separate layer.

With the current private OoT NTSC 1.0 artifacts, the audit measures 13,203 C
functions and 13,324 Rust functions. There are 13,047 shared nonempty bodies,
with zero unequal unique instruction-PC sets, but 116 callable empty C bodies
have nonempty Rust counterparts. Those counts are mechanically reproduced by
the command below; no game-derived input or generated output is committed.

## Commands and claims

```sh
# No-ROM contract checks and path-free phase plan. Neither invokes Cargo.
scripts/lane-parity.sh --selftest
scripts/lane-parity.sh --dry-run --observe 60

# Authoritative mode (default): currently exits 2 before boot because the
# generated callable-body sets are not aligned.
FN64_GAME_DIR=/path/to/private/game-workspace scripts/lane-parity.sh 60

# Observation mode: runs both lanes despite the admitted C coverage defect.
FN64_GAME_DIR=/path/to/private/game-workspace scripts/lane-parity.sh --observe 60
```

Default mode can say `OK` only after the body audit is aligned and framebuffer
SHAs match. `--observe` prints `NON-AUTHORITATIVE OBSERVATION` before building
and can report only `OBSERVED MATCH` or `OBSERVED DIVERGENCE`. A matching
observation is a useful end-to-end regression signal for the exercised output;
it is not evidence that an empty C body was unreachable or semantically
irrelevant.

The native emitter build, emitter execution, callable-body test, C build, C
run, Rust build, and Rust run are sequential phases. Each phase owns a fresh
exact process group under the common 2048 MiB/40%-free memory guard; every Cargo
compiler command is `-j1`, and the authority test also fixes one libtest thread.
The C and Rust target paths remain distinct and unchanged. Compiler/linker
children spawned by either Cargo build inherit that phase's process group, so a
threshold crossing terminates the complete phase without crossing into the
other lane. A native-emitter failure is an authority error, not a content skip.

The current measured observation matches through swap 60 under shared
guest-quiescence timing. A historical deeper observation first differed at
framebuffer 234 after graphics task 232. Neither number is an authority
horizon: the audit rejects C arbitration from swap zero because no executable
reachability proof excludes the 116 missing bodies before either point.

## Independent evidence and residual limit

The usable evidence stack is deliberately split:

- The whole-corpus PC-set audit proves structural instruction coverage for
  13,047 shared generated functions.
- `oracle.rs`, `fpu_oracle.rs`, `dword.rs`, `mixed_isa.rs`, and related tests
  execute emitted Rust against independent MIT-generated-C or ISA-semantic
  oracles for their named instruction families.
- `lane-parity.sh --observe` compares end-to-end framebuffer bytes while
  labeling the legacy lane's missing-body defect.
- Schema-v34 fixed-cycle device/framebuffer/audio/memory digests,
  boundary-owned observations, the compiled unsupported-instrumentation
  identity, and the bound zero-unsupported journal provide the release
  authority mechanism. Representative private NTSC reference and RT64
  LLE/post-VI exact-ten schema-v22 series completed and were independently
  reverified on 2026-07-22 with zero unsupported events; these are historical
  and require schema-v34 regeneration. The public synthetic identified-native
  XBUS scenario has a historical schema-v28 macOS arm64 exact-ten gate whose
  sole repository acceptance anchor was a complete target-named semantic
  fingerprint including both build-produced archive hashes. It passed 10/10
  consecutive parent invocations (100 fresh children) on 2026-07-24, but now
  requires schema-v34 regeneration. Compiler, SDK, or target drift also fails
  closed pending a separately reviewed golden. Combined with the
  retained historical public synthetic exact-ten series, the
  previous three-scenario matrix credited 12 of 162 requirements and retained the
  other 150 explicitly. The public series adds mechanism coverage, not another
  representative full-ROM parity claim.

The residual limitation is exact: structural PC equality cannot detect a
wrong translation of a covered instruction, the focused semantic oracles do
not exhaust every instruction context, and the framebuffer observation shares
runtime and renderer blind spots. The legacy C lane becomes authoritative
again only if its callable-body set is mechanically aligned, or if a separate
independent oracle proves the behavior under test without relying on that
lane.

Provenance: generated C is N64Recomp's MIT output; generated Rust and this
method are fn64-owned. No GPL runtime implementation is read or required.
