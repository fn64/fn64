# RT64 wgpu shader ingestion assessment

M2.5b measures the accepted 56-row M2.5a SPIR-V corpus against the pinned,
source-built wgpu 30/Naga 30 validator. It is a typed ingestion assessment,
not a runtime shader implementation. The machine-readable contract is
[`rt64-wgpu-shader-assessment-schema.json`](rt64-wgpu-shader-assessment-schema.json).

This ticket name supersedes older M2.5a handoff prose that used “M2.5b” for
owned WGSL production. Owned runtime shaders and native adapter execution are
M2.5c work; they are deliberately absent here.

## Outcome taxonomy

Every accepted reference row first receives one validator profile derived from
its exact authenticated SPIR-V bytes. No `PushConstant` (`StorageClass` 9)
selects `baseline`. One `PushConstant` global must point directly through a
matching storage-class pointer to one `Block` struct whose named members have
one exact `Offset` each and are bounded 32-bit scalars or vectors. The raw
content extent is `max(offset + size)`. Its struct span is rounded up to the
largest Naga member alignment: 4 bytes for scalars, 8 for two-component vectors,
and 16 for three- or four-component vectors. That rounded span must select
exactly one of `immediates-{4,8,16,20,24,32,40,56}`. In particular, the corpus's
raw 20-, 36-, and 52-byte vector-bearing shapes select 24, 40, and 56; profiles
36 and 52 do not exist. The receipt retains the IDs, names, ordered member
types, offsets, sizes, rounded required span, and selected profile. Multiple
globals, group decorations, missing or duplicate offsets, arrays, matrices,
recursive/nested/unsized types, unsupported widths or vector shapes, overlap,
content or round-up overflow, and any other rounded span fail closed.
The accepted denominator is also closed by profile count: baseline 13;
immediates-4 6; immediates-8 3; immediates-16 9; immediates-20 1;
immediates-24 5; immediates-32 13; immediates-40 5; immediates-56 1.

Every row then receives exactly one outcome, in reference-receipt order:

- `ingestible`: the exact staged wgpu validator exits 0, writes no stderr, and
  writes the exact canonical pass record for the row's stage, entry point, and
  byte length.
- `blocked-known`: the validator exits 2 with empty stdout and matches one exact
  typed blocker class:
  - `ShaderNonUniform`: the exact pinned unsupported-capability diagnostic plus
    authenticated capability `ShaderNonUniform` (5301), extension
    `SPV_EXT_descriptor_indexing`, and a direct `NonUniform` decoration.
  - scalar storage layout: the exact pinned `instanceRDPParams` Naga validation
    diagnostic plus a witness derived from the authenticated SPIR-V bytes. The
    witness requires legacy `Uniform` + `BufferBlock`, descriptor set 0/binding
    2, a 128-byte runtime array of `RDPParams`, and member 7 `keyScale` as
    `float3` at offset 92. Naga's standard layout requires 16-byte alignment, so
    the offset is deliberately recorded as unaligned.
  - `SampledBuffer`: the exact pinned unsupported-capability diagnostic plus
    authenticated capability `SampledBuffer` (46), declared exactly once at its
    exact word offset.
  - fragment direct blend-source-index output: the exact pinned `PSMain` entry-
    point-invalid Naga validation diagnostic plus a witness derived from the
    authenticated SPIR-V bytes. The witness requires a `fragment`-stage
    `PSMain` entry point whose interface directly (not through a struct member)
    lists an `Output`-storage-class `float4` variable named
    `out.var.SV_TARGET0`, decorated with exact `Location` 0 and `Index` 0.

An unexpected success for any blocker witness is contract drift, not an
ingestible result. The former row-11 baseline-limit diagnostic is deliberately
not a blocker class: its derived immediate profile must pass or match another
independently preserved exact blocker. Any near-match, other diagnostic, exit
code, output, profile mismatch, missing or changed witness, row mutation, or
incomplete denominator aborts without a complete receipt. Each blocked-known
row carries exactly one matching witness among the four closed classes; an
ingestible row carries none. Relaxed capabilities, disabling Naga
`STRUCT_LAYOUTS`, SPIR-V passthrough, decoration stripping, skip flags, and
unchecked shader-module creation are not admitted alternatives.

## Commands

The accepted M2.5a corpus and current validator build paths are explicit. M2.5b
does not rerun M2.5a against whichever producer checkouts happen to exist now:
it authenticates the independently accepted historical pretty-JSON receipt by
its exact file and receipt identities, audits all 225 retained files for exact
canonical paths, lengths, SHA-256 values, and regular/single-link/unique-object
identity, reconstructs the artifact set and row order, and rejects any extra
file. It then verifies the pinned validator build, copies the validator and each
module into a fresh private staging root, and executes only those retained bytes.
The policy records `frozen` with the independently reviewed v2 build receipt,
binary, source, lock, and dependency identities. The build receipt pin is its
internal canonical receipt identity, not the SHA-256 of the pretty-JSON receipt
file; no v1 digest is relabelled.

```sh
python3 tools/rt64_wgpu_shader_assessment.py selftest

python3 tools/rt64_wgpu_shader_assessment.py assess \
  --reference-artifact-dir /absolute/accepted-reference-corpus \
  --wgpu-validator-build-dir /absolute/wgpu-validator-build \
  --output-dir /absolute/new-assessment

python3 tools/rt64_wgpu_shader_assessment.py verify \
  [the same corpus/validator-build arguments] \
  --assessment-dir /absolute/existing-assessment

python3 tools/rt64_wgpu_shader_assessment.py runtime-ready \
  [the same corpus/validator-build arguments] \
  --assessment-dir /absolute/existing-assessment

python3 tools/rt64_wgpu_shader_assessment.py diagnostic-census \
  --reference-artifact-dir /absolute/accepted-reference-corpus \
  --wgpu-validator-build-dir /absolute/wgpu-validator-build
```

The validator invocation is exactly `--profile <derived-profile> --shader
<private-staged-spv> --stage <stage> --entry <entry>`. Its v2 success record
must bind the same closed profile object, stage, entry, and module length.

`assess` creates only `assessment-receipt.json`, and only after all 56 rows are
classified. `verify` repeats the complete external verification and assessment
without writing. `runtime-ready` also performs that full verification, prints a
path-free readiness record, and exits 78 because schema v3 cannot establish
runtime readiness.

`diagnostic-census` is a bounded, explicitly non-authoritative way to inspect
all 56 rows before the fail-fast assessment. It authenticates the same corpus
and validator, derives the same profile and immediate witness, uses private
snapshots and an inherited-environment-cleared child with only the four pinned
environment entries, and continues after arbitrary validator exit codes. Each
stream is hard-limited to 4 KiB, each row to 8 KiB, all child output to 448 KiB,
optional UTF-8 path-free failure text to 1 KiB per stream, and the emitted JSON
to 4 MiB. Each row records exact exit/timeout state, byte lengths, SHA-256
digests, profile, and witness. The command writes no assessment artifact and
emits no receipt identity. Its distinct diagnostic schema, authority label,
hard-false `runtime_ready`, and absent assessment path mean `verify` and
`runtime-ready` cannot consume it. It is a discovery accelerator, not evidence.

## Runtime boundary

`runtime_ready` is unconditionally false in schema v3. A blocked row adds
`blocked-known-ingestion-row`; the following gaps always remain:

1. `native-adapter-contract-not-recorded`
2. `native-shader-module-not-executed`
3. `pipeline-and-semantic-evidence-not-recorded`

Feature names or synthetic limit values cannot promote this assessment. A later
native receipt must bind a real adapter/device contract, checked module creation,
pipeline execution, and semantic evidence. This receipt makes no claim about an
adapter, device, WGSL production, pipeline behavior, runtime integration, parity,
or performance.

The scalar-layout class is a bounded wgpu 30/Naga 30 ingestion gap. wgpu's
shader-module path invokes Naga with all validation flags, while Naga's standard
storage-layout model has no scalar-block-layout mode. M2.5a's Vulkan reference
contract remains conditional on `VK_EXT_scalar_block_layout` and
`scalarBlockLayout == VK_TRUE`; recognizing the exact Naga rejection does not
weaken or replace either contract.

## Process improvement

Before another full external corpus pass, run the tool's self-test and focused
hostile suite once, then obtain independent mechanism review. This keeps cheap
policy/CLI/classifier failures ahead of the expensive 56-row loop and prevents
rebuilding or consuming evidence under a producer identity that review will
invalidate.
