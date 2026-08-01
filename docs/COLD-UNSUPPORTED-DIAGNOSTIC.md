# Cold unsupported-destination diagnostic

`diagnose_cold_unsupported` runs the bounded cold-ROM measurement and emits one
JSON line per input ROM. The record contains the normalized digest, selected
discovery strategy, proven-bank count, closure scoreboard or open blocker, and
the typed provenance for every unsupported destination:

- destination address and `DestinationReason`;
- incoming bank and block extent;
- source site; and
- concrete transfer kind.

The audit is content-free: it contains addresses and classifications, never ROM
bytes or instruction words. It is deliberately outside the sealed
`fn64.cold-rom-receipt.v2` measurement, so adding diagnostic provenance does not
change historical receipt identity.

Run it on one or more uncompressed ROM images:

```sh
cargo run -p fn64-discover --bin diagnose_cold_unsupported -- ROM [ROM ...]
```

For a batch, rejected inputs are reported on standard error without suppressing
later inputs. The process exits nonzero after the batch if any input failed.

This diagnostic identifies the concrete unsupported edge; it does not grant a
mapping or execution authority. One run is evidence only. Apply the
`AGENTS.md` repetition bar before making a determinism claim.
