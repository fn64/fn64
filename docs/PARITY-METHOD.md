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
builds either boot binary. `crates/fn64-recomp-rs/tests/lane_authority.rs`:

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
the same semantics; `fn64-recomp-rs`'s instruction/oracle suites own that
separate layer.

With the current private OoT NTSC 1.0 artifacts, the audit measures 13,203 C
functions and 13,324 Rust functions. There are 13,047 shared nonempty bodies,
with zero unequal unique instruction-PC sets, but 116 callable empty C bodies
have nonempty Rust counterparts. Those counts are mechanically reproduced by
the command below; no game-derived input or generated output is committed.

## Commands and claims

```sh
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
- Schema-v6 fixed-cycle device/framebuffer/audio/memory digests and the bound
  zero-unsupported journal provide the release authority mechanism. No
  representative full-ROM matrix has yet been populated, so no full-ROM
  zero-unsupported claim has been made.

The residual limitation is exact: structural PC equality cannot detect a
wrong translation of a covered instruction, the focused semantic oracles do
not exhaust every instruction context, and the framebuffer observation shares
runtime and renderer blind spots. The legacy C lane becomes authoritative
again only if its callable-body set is mechanically aligned, or if a separate
independent oracle proves the behavior under test without relying on that
lane.

Provenance: generated C is N64Recomp's MIT output; generated Rust and this
method are fn64-owned. No GPL runtime implementation is read or required.
