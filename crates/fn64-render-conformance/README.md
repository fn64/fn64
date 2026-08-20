# fn64-render-conformance

This crate owns bounded renderer fixtures and the opaque evidence shapes used
by the RT64-to-Rust migration ladder. It does not define another render IR:
every fixture retains an exact `fn64-render-ir` record plus the owned streams
needed for identity-checked replay.

There is deliberately no public pass issuer. `BackendProducedObservable` and
`SealedExecutionEvidence` have private fields and no public constructors. A
concrete backend runner must add a reviewed private issuer coupled to its real
completion path; fixture expectations cannot be promoted into observations.
Guest-visible in-memory evidence retains an actual `GuestCommittedTicket`, not
an editable digest standing in for one.

No such RT64 or Rust-port runner is registered yet, so every backend row stays
open. For a future closed row, the manifest checker—not evidence JSON—launches
the exact retained runner ten times. It generates an unpredictable challenge
for each child, captures stdout itself, checks the child PID, and derives every
fixture, semantic, process, run, and series identity from stable artifacts and
typed results. A fixture's expected bytes, arbitrary build bytes, a backend
label, caller nonces, prose, and hand-authored JSON cannot issue a pass.

See `docs/RT64-PORT-PARITY.md` and run:

```sh
cargo test -p fn64-render-conformance
python3 tools/check_rt64_port_parity.py
python3 tools/check_rt64_port_parity.py --progress # expected red today
```

## The wgpu runner and the backend differential

`fn64-render-conformance-wgpu-runner` (feature `wgpu-runner`) is the first
adapter for the backend fn64 actually ships. It replays the *identical*
fixture the reference runner replays -- same `ROW_ID`, same eight-command
display list, same 8x4 RGBA16 native target, same seeded bytes and region
guards -- through `fn64-render-wgpu`'s raw-DPC seam. Two runners answering one
row against one hand-derived answer key is what makes this a differential
rather than two unrelated tests.

**Adapterless.** No GPU device is requested. `ConformanceSession::try_new`
records the host-configured extent and tolerates `WgpuCreateError::NoAdapter`,
and the observation is `ColorTargetRegistry`'s `device_bytes`, a CPU
`Vec<u8>`. It therefore runs in the default suite.

```sh
cargo build -p fn64-render-conformance --features wgpu-runner \
  --bin fn64-render-conformance-wgpu-runner
fn64-render-conformance-wgpu-runner diff   # one row, per-pixel
fn64-render-conformance-wgpu-runner sweep  # both backends, a family of rows
```

**`device_bytes` are not guest bytes.** They are flat big-endian device bytes;
guest RDRAM is stored in native words under the `^3` byte-lane mapping. The
runner copies them back through `RdramViewMut::write_logical_bytes`, the exact
call `fn64-abi`'s `copy_committed_guest_writes` makes. A raw `copy_from_slice`
reports all 32 pixels as byte-swapped -- a runner defect that reads exactly
like a renderer defect.

### Mutation evidence

The instrument's ability to *detect* a disagreement was mutation-tested, not
assumed. All three mutants were killed:

| Mutant | Expected kill | Observed |
|---|---|---|
| M1: copy back with a raw slice copy instead of `write_logical_bytes` | `diff` flips `agrees` -> `diverges` | 32/32 pixels differ |
| M2: `wgpu_bytes` returns the reference backend's bytes | the scissor row flips `disagree` -> `agree` | flipped; 4 -> 3 disagreeing cases |
| M3: invert the scissor case's hand-derived key | the key blesses the other backend, backend-vs-backend verdict unchanged | `reference_matches_key` true -> false, `wgpu_matches_key` false -> true, still `disagree` |

M3 is the one that matters most: it proves the key is an independent third
authority rather than something derived from either backend.
