# Why the two WCW titles miss the host-binding recognizers

Status: investigated 2026-08-09. One recognizer fixed; the remaining gaps are
characterized and deliberately **not** attempted. Read the "what not to do"
section before picking this up.

The prior record (`corpus-certification-frontier.md:1810-1814`) lists both WCW
titles as `FAIL -- 0 candidates` and treats them as bounded out of the boot
lane. That is accurate as an outcome and misleading as a diagnosis: the two
titles fail for entirely different reasons, and only one of them is close.

## The probe output does not say what it looks like it says

`examples/probe_host_bindings.rs` prints `found.len()/{total}` on success but
only `{total}` on failure:

```rust
Ok(found) => println!("{label}\tOK\t{}/{total}", found.len()),
Err(error) => println!("{label}\tFAIL\t{total}\t{error:?}"),
```

Every failing ROM therefore prints `15` — it is
`WM_BLOCK_RUNTIME_HOST_SYMBOLS.len()`, a constant, not a score. It was read
once as "15 resolved, one symbol short". It carries no information about how
many resolved.

`discover_wm_block_runtime_host_bindings` is also a chain of `?`, so it aborts
at the **first** failing symbol and never evaluates the rest. A failure names
one symbol because it stopped there, not because the others passed. The two
titles naming different symbols is partly discovery order:
`osCreateMesgQueue` is resolved first inside the overlay-loader group and
`osSetEventMesg` sixth.

**If you want per-symbol truth, run each recognizer independently.** The
short-circuit makes "did not resolve" and "was never evaluated" look identical.

## Measured per-symbol matrix

Each recognizer run independently over the 1 MB boot window. Nine of the
fifteen roles are directly-callable predicates; the other six
(`osCreateThread`, `osRecvMesg`, the four RSP task roles) are found by
multi-stage call-chain logic and are not separable this way, so the
denominator here is 9, not 15.

| symbol | WM2000 | No Mercy | VPW2 | Revenge | World Tour |
|---|---|---|---|---|---|
| osCreateMesgQueue | 1 | 1 | 1 | 1 | 0 |
| osEPiStartDma | 1 | 1 | 1 | 1 | 0 |
| osGetThreadPri | 1 | 1 | 1 | 1 | 1 |
| osSendMesg | 1 | 1 | 1 | 1 | 0 |
| osSetEventMesg | 1 | 1 | 1 | **0** | 0 |
| osSetThreadPri | 1 | 1 | 1 | 1 | 0 |
| osStartThread | 1 | 1 | 1 | 1 | 0 |
| __osSiDeviceBusy | 1 | 1 | 1 | 1 | 0 |
| osSetTimer | 1 | 1 | 1 | **0** | 0 |
| **subtotal** | 9/9 | 9/9 | 9/9 | **7/9** | **1/9** |

Revenge has two genuine gaps, not one — the second was hidden behind the
short-circuit. World Tour is a different problem entirely.

## Ruled out: it is not a scan-window problem

The probe scans 1 MB at file offset `0x1000`. World Tour is a 12 MB ROM whose
boot code stops around `0x30000`, with a second large code region at
`0xa20000-0xac0000` well outside the window. That looked like the obvious
explanation and it is **wrong**:

- Scanning the whole 12 MB still yields 8/9 absent.
- Scanning `0xa00000-0xb00000` on its own yields 0/9.
- Both regions measure entropy ~6.3 with a zlib ratio ~0.53 — plain,
  uncompressed MIPS, not packed or encrypted code.
- All three headers share load address `0x80000400` and `.z64` byte order.

The code is visible and the recognizers genuinely miss it. Do not re-litigate
this; the hypothesis was raised, endorsed, and killed by measurement.

## The triage that separates the two titles

Take each routine's confirmed body from WM2000, erase **all** registers and
immediates down to an opcode-only skeleton, and search each WCW ROM allowing
~15% opcode drift. Skeleton found means the same code compiled differently
(widenable). Skeleton absent means a different instruction sequence.

| symbol | Revenge | World Tour |
|---|---|---|
| osEPiStartDma | SKELETON_FOUND | DIFFERENT_CODE |
| osSendMesg | SKELETON_FOUND | DIFFERENT_CODE |
| osSetThreadPri | SKELETON_FOUND | DIFFERENT_CODE |
| osStartThread | SKELETON_FOUND | DIFFERENT_CODE |
| __osSiDeviceBusy | SKELETON_FOUND | DIFFERENT_CODE |
| osSetEventMesg | DIFFERENT_CODE | DIFFERENT_CODE |
| osSetTimer | DIFFERENT_CODE | DIFFERENT_CODE |

A drift sweep confirms the split is real rather than a tolerance artifact.
Revenge holds at exactly 1 collapsed candidate across 15%, 25% and 35% drift.
World Tour stays at 0 through 35% and only "finds" anything at 50%, where a
6-word skeleton matches 215 places and the search is pure noise.

**Conclusion: Revenge shares WM2000's libultra build lineage. World Tour does
not.** World Tour is a 1996 launch title (`NWNE`); WM2000, No Mercy, VPW2 and
Revenge (`NW2E`) are 1998-2000. Supporting World Tour means supporting a second
libultra generation, which is a project, not a recognizer fix.

## What was fixed: osCreateMesgQueue is now register-allocation free

World Tour's `osCreateMesgQueue` **is present**, at ROM `0x126dc`, and is
behaviourally identical. The entire difference is how the sentinel is
materialized:

```
WM2000 / Revenge            World Tour
lui   $v0, hi               lui   $t6, hi
addiu $v0, $v0, lo          lui   $t7, hi
sw    $v0, 0($a0)           addiu $t6, $t6, lo
sw    $v0, 4($a0)           addiu $t7, $t7, lo
                            sw    $t6, 0($a0)
                            sw    $t7, 4($a0)
<remaining four stores and jr $ra: byte-identical in both>
```

The 1996 build loaded the same sentinel into two registers where the 1998 build
reused one. The old predicate hardcoded `$v0` and pinned the store order, which
describes *a particular compilation*, not ABI behaviour — while `mod.rs:3`
opens by promising "Addresses are outputs, never signatures. The recognizers
below describe public ABI behavior."

**This change restores the module's stated principle rather than relaxing it.**
It is correct on its own merits whether or not either WCW title ever ships. The
new predicate requires the six documented fields stored through `$a0` at their
documented offsets, order-free and register-free, plus a return — and requires
the two queue-head registers to be **proven to hold the same computed address**
by folding each `lui`/`addiu` pair and comparing. That last constraint is what
keeps it a queue-initializer predicate rather than "any six stores through
`$a0`". Every constraint is defensible from the published `OSMesgQueue` layout;
none is ROM-specific, address-based, or keyed to a magic constant.

## The latent counting bug this exposed (`collapse_overlapping_runs`)

An order-free predicate over a window wider than the routine also matches when
the window merely *contains* the routine, so one routine matches at several
adjacent start offsets. WM2000 matched at `0x32434/8/c/40` — four candidates
for one function. `unique_match` requires exactly one, so a **correct** widening
appeared to break all three passing titles.

`unique_match` now collapses runs of strictly adjacent (4-byte apart) addresses
to one candidate. Two genuinely distinct routines are never adjacent at word
granularity, so this cannot merge real ambiguity.

**The run reports its LAST address, not its first.** That is the latest start
whose window still satisfies the predicate, which is the routine's own entry;
earlier starts match only by including preceding filler. This matters because
the overlay call-chain logic resolves `jal` targets against this address —
reporting a run's first address named an instruction inside the padding and
turned three passing titles into `NonUniqueOverlayCallChain { candidates: [] }`.
That regression was caught by the before/after matrix and is exactly what the
matrix is for.

This bug was armed for the next person to widen any recognizer. It is worth
having fixed independently of the WCW work.

## Before/after, all five ROMs

`cargo run --release -p fn64-discover --example probe_host_bindings -- <rom>`

| ROM | before | after |
|---|---|---|
| WWF WrestleMania 2000 | `OK 15/15` | `OK 15/15` |
| WWF No Mercy | `OK 15/15` | `OK 15/15` |
| Virtual Pro Wrestling 2 | `OK 15/15` | `OK 15/15` |
| WCW/nWo Revenge | `FAIL OsSetEventMesg, []` | `FAIL OsSetEventMesg, []` |
| WCW vs. nWo World Tour | `FAIL OsCreateMesgQueue, []` | `FAIL OsEPiStartDma, []` |

No passing title regressed. World Tour advances past `osCreateMesgQueue`,
which is the proof the widening works; it now stops at the next
`DIFFERENT_CODE` routine, which is the expected outcome, not a new problem.

`cargo test --release -p fn64-discover`: **1079 passed, 0 failed**, including
nine new tests covering both compilations of the queue initializer, the
same-sentinel requirement, the wrong-argument and missing-field rejections, and
the run-collapse entry-address rule.

## What not to do next

**Do not describe either WCW title as unblocked or close.** Neither reaches
`OK`. The honest statement is that Revenge needs two new recognizers and World
Tour needs a second libultra generation.

- **Revenge (2 routines, plausible).** `osSetEventMesg` and `osSetTimer` are
  both `DIFFERENT_CODE`. Register generalization will not rescue them — an
  opcode skeleton with registers and immediates erased, allowing 4 mismatches
  in 19, finds nothing. These need recognizers written from the published ABI
  for a different implementation. Against a libultra that otherwise matches
  completely (7/9), this is bounded, honest work. The event system is
  definitely present: `lui $r, 0xa430` (MI interrupt mask) appears 14x in
  Revenge against 16x in WM2000.
- **World Tour (7 routines, do not start casually).** All seven are
  `DIFFERENT_CODE`. This is not widening; it is a parallel recognizer set for a
  1996 libultra, and it should be scoped as its own project with its own
  justification.
- **Never match by address, by a magic constant, or by "whatever this ROM
  happens to do."** A recognizer that does defeats the module's design and is
  worse than leaving a title blocked. If the only way to match is
  non-behavioural, that is a finding to report, not a change to land.

## Revenge passes the generic CPU-recompilation gate (2026-08-09)

With the two recognizers landed (`10a73c7`), Revenge was run through
`gate_rom_recompile` — the title-generic gate that takes one input
(`FN64_DISCOVER_ROM`) and no per-game configuration.

    internal_name              WCW / nWo  REVENGE
    normalized_rom_sha256      d8c097f8880032fc63a73a78ad2fcabac8f4b593…
    banks                      3
    pack_blocks                25057
    pack_words                 145559
    total_destinations         1749
    unsupported_destinations   0      <-- the release blocker
    dynamic_mips_destinations  8
    rustc_compiles             true
    harness_runs               true

**`unsupported == 0` is the criterion `docs/DISCOVER-PLAN.md` names as the
release blocker, and it is met.** Every discovered bank was emitted as Rust,
digest-verified, compiled by a real `rustc`, and probed at arbitrary guest PCs
— including unaligned PCs, which fault as `AddressErrorLoad` rather than
misbehaving.

**ROM digest recorded above**, per the discipline this project keeps
re-learning: a per-title result filed under a title string cannot be
reconciled later. This is the Starrcade v1.01 image.

### What this does and does not mean

**Does:** Revenge is no longer blocked at discovery. It joins WM2000, No Mercy
and VPW2 as a title whose whole ROM CPU-recompiles. The record's "0 candidates,
bounded out of the lane" is now wrong in both of its claims about this title.

**Does not:** this is a CPU-recompilation milestone, and the gate says so
itself — `not_a_booting_game=true`, because RSP audio and RDP graphics are
separate runtime subsystems the gate never consults. Revenge needs the same
remaining bring-up as No Mercy: a shard tree, a boot context, executable-image
PCs, and an input schedule.

**Four of five AKI titles now CPU-recompile.** World Tour remains the
exception, and remains a second-libultra-generation project rather than a
recognizer gap.

## VPW2 re-verified in this tree (2026-08-09)

Ran the same generic gate against VPW2 rather than inheriting the claim from a
2026-08-07 note:

    internal_name              VPW2 freem
    normalized_rom_sha256      7706ed94ebc30171186e1d96eba6b1f83f095476…
    banks                      5
    pack_blocks                49347
    total_destinations         4648
    unsupported_destinations   0
    dynamic_mips_destinations  97
    rustc_compiles             true
    harness_runs               true

**Four of five AKI titles now CPU-recompile with `unsupported == 0`, each
verified in this tree with its ROM digest recorded:**

| title | digest | banks | blocks | destinations | unsupported |
|---|---|---:|---:|---:|---:|
| WM2000 | (see byte-identity-1p5M) | 5 | 43,032 | — | 0 |
| No Mercy | `11640379…` | 6 | 57,284 | — | 0 |
| VPW2 | `7706ed94…` | 5 | 49,347 | 4,648 | **0** |
| Revenge | `d8c097f8…` | 3 | 25,057 | 1,749 | **0** |
| World Tour | — | — | — | — | **blocked at discovery** |

Note the earlier record put VPW2 at 49,329 blocks; this run measures 49,347.
An 18-block difference on a re-run is the same provenance problem as the No
Mercy topology figure — **the old number was filed without a ROM digest**, and
the Freem Edition on disk may not be the image it described. The digest above
is recorded so this one can be reconciled.

**The remaining gap for VPW2 is not discovery.** It has no answer key and no
`FN64_DISCOVER_*` entry in `.claude/local.env`, so nothing can grade a
regression against it — that is the item to fix before anyone builds on this
result, and it is bookkeeping rather than engineering.
