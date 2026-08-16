# RT64 wgpu shader ingestion assessment

M2.5b measures the accepted 56-row M2.5a SPIR-V corpus against the pinned,
source-built wgpu 30/Naga 30 validator. It is a typed ingestion assessment,
not a runtime shader implementation. The machine-readable contract is
[`rt64-wgpu-shader-assessment-schema.json`](rt64-wgpu-shader-assessment-schema.json).

This ticket name supersedes older M2.5a handoff prose that used “M2.5b” for
owned WGSL production. Owned runtime shaders and native adapter execution are
M2.5c work; they are deliberately absent here.

## Outcome taxonomy

Every accepted reference row receives exactly one outcome, in reference-receipt
order:

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

An unexpected success for either witness is contract drift, not an ingestible
result. Any near-match, other diagnostic, exit code, output, missing or changed
set/binding/member/type/offset/alignment witness, row mutation, or incomplete
denominator aborts without a complete receipt. Relaxed capabilities, disabling
Naga `STRUCT_LAYOUTS`, SPIR-V passthrough, decoration stripping, skip flags, and
unchecked shader-module creation are not admitted alternatives.

## Commands

All source/build/corpus paths are explicit. The assessment tool first reruns the
complete M2.5a reference verifier, then verifies the pinned wgpu validator build,
copies the validator and each module into a fresh private staging root, and
executes only those retained bytes.

```sh
python3 tools/rt64_wgpu_shader_assessment.py selftest

python3 tools/rt64_wgpu_shader_assessment.py assess \
  --port-dir /absolute/rt64-port \
  --oracle-dir /absolute/rt64-oracle \
  --dxc-dir /absolute/dxc-source \
  --dxc-build-dir /absolute/dxc-build \
  --spirv-val-build-dir /absolute/spirv-val-build \
  --reference-artifact-dir /absolute/accepted-reference-corpus \
  --wgpu-validator-build-dir /absolute/wgpu-validator-build \
  --output-dir /absolute/new-assessment

python3 tools/rt64_wgpu_shader_assessment.py verify \
  [the same source/build/corpus arguments] \
  --assessment-dir /absolute/existing-assessment

python3 tools/rt64_wgpu_shader_assessment.py runtime-ready \
  [the same source/build/corpus arguments] \
  --assessment-dir /absolute/existing-assessment
```

`assess` creates only `assessment-receipt.json`, and only after all 56 rows are
classified. `verify` repeats the complete external verification and assessment
without writing. `runtime-ready` also performs that full verification, prints a
path-free readiness record, and exits 78 because schema v2 cannot establish
runtime readiness.

## Runtime boundary

`runtime_ready` is unconditionally false in schema v2. A blocked row adds
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
