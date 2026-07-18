# Ghidra T3 conformance spike

This directory contains a bounded, synthetic-only headless Ghidra experiment.
It does not load a ROM, use N64LoaderWV as a mapping authority, or establish
function ownership. It proves that the production adapter shape proposed in
`docs/DISCOVER-TOOLCHAIN.md` is executable:

- each bank is imported into a separate disposable project with raw
  `MIPS:BE:64:64-32addr:o32` bytes at an fn64-supplied VA;
- banks A and B deliberately occupy the same VA but call different functions;
- `unseeded` receives only the synthetic proven entry, while `seeded` receives
  an additional snapshot-derived entry and records that parent lineage;
- the post-script emits only `function_entry` and `function_extent` candidates
  through `fn64.tool-adapter` schema v1; and
- `gate_tool_jsonl` passes the result through the real Rust parser and verifies
  completion and the canonical claim digest.

Run it with:

```sh
tools/ghidra/run-conformance.sh
```

Override `GHIDRA_HEADLESS`, `GHIDRA_JAVA_HOME`, or `FN64_GHIDRA_WORK` for
another installation. The work directory must remain outside the repository.
The checked-in `.hex` inputs are handwritten MIPS instructions; all binaries,
projects, logs, and JSONL are materialized under `/private/tmp` by default.

This is not the full Decomp Pack exporter. The next bounded expansion should
export bank-qualified basic blocks, direct and indirect xrefs, switch records,
data objects, prototypes, decompiler types, and stack-frame candidates into the
canonical graph. Those artifacts are valuable inputs to a Decomp Pack even
when they cannot prove exact ownership. Exact owner derivation and recompiler
admission remain separate Rust proof stages; seeded Ghidra descendants cannot
corroborate their own seeds.

## Measured conformance

On 2026-07-17, Ghidra 12.1.2 and OpenJDK 21 completed ten consecutive clean
runs. Each run created three isolated projects and passed the Rust JSONL gate.
Each stream had exactly one SHA-256 value across all ten runs:

- bank A seeded: `193ce1641402c9f4c436a7abca70030970859dcb12895535661f863a7fa45e0f`
- bank A unseeded: `953c0a01d81dbdd4fb05c9c02d6664c09610254da22970b4246ee9a55892b807`
- bank B unseeded: `8d3fdcc8e222d598ea815703296d403a693f192e98639d1ee4657dfa5c5e8e31`

One measured three-project run took 8.89 seconds wall time. In the restricted
agent sandbox, Ghidra logged non-fatal permission warnings for its optional
`/var/tmp` caches; the raw imports, analyses, post-scripts, strict ingestion,
and project deletion all completed successfully.
