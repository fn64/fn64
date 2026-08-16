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
