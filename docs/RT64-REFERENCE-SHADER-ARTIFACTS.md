# RT64 reference shader artifacts

Status: **additive mechanism integrated; v2 source-build receipt verified as historical evidence; scalar-layout validation contract pending review.**

This is M2.5a's additive evidence path for the exact RT64 HLSL denominator. It
produces reference-valid SPIR-V without weakening or replacing the accepted
wgpu-ingestion path in `tools/rt64_shader_artifacts.py`. The existing artifact
producer, its policy, and its wgpu 30 validator remain byte-identical. This
tool imports their qualified source staging, DXC receipt verification, and
three explicit dependency/preprocess/compile phases, and every new receipt
binds both producer identities.

The claim is conditional reference validity under the typed scalar-block-layout
device contract below. It is not evidence that an adapter exposes the contract,
that Naga can parse a module, that wgpu or a pipeline can create it, that a
backend/runtime can run it, that its output has parity with RT64, or that it
meets any performance target. In particular, the preserved v3 attempt failed
closed on row one because Naga 30 rejects the `ShaderNonUniform` capability
deliberately emitted for RT64's non-uniform texture/TMEM binding array accesses.

## Direct consumers and ownership

`docs/rt64-reference-shader-artifact-schema.json` is consumed directly only
by:

- `tools/rt64_reference_shader_artifacts.py`
- `tools/test_rt64_reference_shader_artifacts.py`

Generated compilers, receipts, preprocessed HLSL, and SPIR-V stay outside the
repository. No runtime crate consumes this schema or these artifacts. The
canonical port status/dashboard remains owned by the orchestration/status
lane; this additive branch only supplies the status handoff below.

## Frozen authority and validator build

The build accepts only the complete clean official DXC checkout at
`0d3ee6b551b8fa768fbf825300ebab81047ef6a8`, including its exact initialized
SPIRV-Tools gitlink `b707790a898e44038547df54580022fc1cf89c3d`
and SPIRV-Headers gitlink
`29981f65241605e08b0ede4cfeb999fe3b723c6a`. It configures the official
SPIRV-Tools project directly and builds only the `spirv-val` Ninja target,
statically linking project code. Tests, fuzzers, compression, mimalloc,
timers, shared libraries, and installation are disabled explicitly.

The receipt repeats the complete parent/gitlink byte audit before and after
the build. It binds the exact CMake flags and controlled environment, tool and
version identities, configure/build/target-command transcripts, CMake and
Ninja graphs, the fresh target's executed translation-unit closure, the six
generated table/version authorities, the retained executable, and its exact
Darwin loader denominator. Darwin admits only `/usr/lib/libc++.1.dylib` and
`/usr/lib/libSystem.B.dylib`; any project dylib, `@rpath` edge, unknown system
library, duplicate load, or changed load descriptor fails closed. Linux and
Windows require separately reviewed loader policies.

The six generated authorities have an exact ordered path denominator:
`build/core_tables_body.inc`, `build/core_tables_header.inc`,
`build/DebugInfo.h`, `build/OpenCLDebugInfo100.h`, `build/generators.inc`, and
`build/build-version.inc`. These paths come from SPIRV-Tools' explicit
`${spirv-tools_BINARY_DIR}` CMake outputs under the directly configured build
root; the mechanism neither searches by basename nor infers paths from the
generator.

The controlled build environment has an exact ordered denominator: `PATH`,
`LC_ALL`, `LANG`, `CC`, `CXX`, `GIT_CONFIG_GLOBAL`,
`GIT_CONFIG_NOSYSTEM`, `GIT_NO_REPLACE_OBJECTS`, `GIT_OPTIONAL_LOCKS`,
`GIT_TERMINAL_PROMPT`, `FORCED_BUILD_VERSION_DESCRIPTION`, and
`SOURCE_DATE_EPOCH`. No optional host variable is inherited.

The source authority also binds all 16 unified SPIR-V grammar inputs used by
the generated tables and `spir-v.xml`, in their official order and at their
exact hashes. The core grammar additionally drives the corpus capability,
extension, and `NonUniform` inventory.

Standalone `spirv-val` is separately source-built, receipted, staged, and
invoked from DXC's in-process validation. It is not an independent validator
implementation: both validations use the same pinned SPIRV-Tools source. The
second invocation is still useful process evidence because it consumes the
retained artifact bytes through a distinct executable boundary.

## Typed Vulkan device contract

Validation has one exact argv denominator:
`spirv-val --target-env vulkan1.0 --scalar-block-layout -`. No omitted scalar
mode, reordered or extra option, `--relax-block-layout`, mixed
scalar-plus-relaxed mode, or skip option is admitted. Receipts bind the typed
device requirements alongside that validator mode:

- extension `VK_EXT_scalar_block_layout` must be enabled; and
- feature `scalarBlockLayout` must equal `VK_TRUE`.

DXC is intentionally invoked with `-fvk-use-dx-layout`. That layout contract
requires scalar block layout; relaxed block layout is not a sufficient or
interchangeable device contract. The preserved v2 first-row diagnostic made
the distinction executable: under standard Vulkan 1.0 layout,
`RDPParams.keyScale` (member 7) at offset 92 failed the required 16-byte
alignment. The v2 validator build receipt remains valid historical build
evidence, but its failed smoke emitted no receipt and established no shader
qualification claim.

## Corpus production

For every one of the existing 56 denominator rows, the additive producer:

1. verifies the accepted DXC build receipt and privately stages its exact
   compiler closure;
2. privately stages the exact RT64 source set;
3. reuses the accepted dependency-only `-M/-MF`, preprocess-only `-P/-Fi`, and
   retained-preprocessed compile helpers without changing them;
4. requires that `-Vd` is absent, so successful compilation retains DXC's
   built-in SPIR-V validation;
5. sends the descriptor-stably read artifact bytes to the private
   `spirv-val --target-env vulkan1.0 --scalar-block-layout -` process through
   stdin, with no relaxed, skipped, reordered, mixed, or extra validation
   flags;
6. requires zero exit, empty stdout/stderr, no file-set change, and identical
   artifact bytes after validation; and
7. parses the same retained bytes with the receipt-bound core grammar to emit
   ordered capability, extension, and direct `NonUniform` decoration rows with
   absolute module word offsets.

SPIR-V decoration groups fail closed. The inventory does not guess through
`OpDecorationGroup`, `OpGroupDecorate`, or `OpGroupMemberDecorate`; support
requires a separately reviewed group-expansion implementation. Unknown or
malformed inventory enumerants, strings, word counts, member-level
`NonUniform`, and instruction extents also fail closed.

Verification revalidates both source-build receipts, reparses the retained raw
DXC dependency file, checks the normalized dependency closure, checks exact
file/link/size/digest denominators, reruns `spirv-val` from a fresh private
0700 staging directory outside the corpus tree, and reproduces the
grammar-bound semantic inventory. The staging operation therefore cannot add
files beneath the exact corpus denominator it is checking. The verifier
resolves the created root and rejects canonical ancestry overlap before it
changes permissions or stages any validator byte.

## Commands

All output paths must be new and outside fn64.

```sh
python3 tools/rt64_reference_shader_artifacts.py build-spirv-val \
  --dxc-dir /absolute/path/to/DirectXShaderCompiler \
  --output-dir /outside/repo/spirv-val-build

python3 tools/rt64_reference_shader_artifacts.py verify-spirv-val-build \
  --dxc-dir /absolute/path/to/DirectXShaderCompiler \
  --build-dir /outside/repo/spirv-val-build

python3 tools/rt64_reference_shader_artifacts.py smoke-spirv-val \
  --dxc-dir /absolute/path/to/DirectXShaderCompiler \
  --build-dir /outside/repo/spirv-val-build \
  --artifact /outside/repo/one-retained-witness.spv \
  --require-shader-nonuniform

python3 tools/rt64_reference_shader_artifacts.py produce \
  --port-dir /absolute/path/to/rt64-port \
  --oracle-dir /absolute/path/to/rt64-oracle \
  --dxc-dir /absolute/path/to/DirectXShaderCompiler \
  --dxc-build-dir /outside/repo/qualified-dxc-build \
  --spirv-val-build-dir /outside/repo/spirv-val-build \
  --output-dir /outside/repo/reference-corpus

python3 tools/rt64_reference_shader_artifacts.py verify \
  --port-dir /absolute/path/to/rt64-port \
  --oracle-dir /absolute/path/to/rt64-oracle \
  --dxc-dir /absolute/path/to/DirectXShaderCompiler \
  --dxc-build-dir /outside/repo/qualified-dxc-build \
  --spirv-val-build-dir /outside/repo/spirv-val-build \
  --artifact-dir /outside/repo/reference-corpus
```

The first executable gate after mechanism review is one retained
`ShaderNonUniform` witness through the source-built validator and inventory,
before paying for all 56 rows. This is the continuous-improvement loop learned
from the earlier depfile and Naga failures: prove external tool semantics with
one representative artifact, record the finding, then expand the denominator.
`smoke-spirv-val` first verifies the complete validator build receipt, then
descriptor-stably reads one explicit outside-repository regular file with one
link, qualifies its canonical parent tree, privately stages the validator and
grammar outside both fn64 and that artifact tree, validates the exact bytes
through stdin, and emits a path-free receipt containing the semantic
inventory. Containment is bound to parent-chain device/inode identities as
well as canonical paths, so renaming the qualified directory and redirecting
the temporary root to that same directory object fails closed before staging.
The smoke inventory is bounded at 65,536 semantic rows before JSON
amplification, and the final canonical pretty-JSON receipt must fit the policy's
8 MiB receipt limit. `--require-shader-nonuniform` additionally requires both the
`ShaderNonUniform` capability and at least one direct `NonUniform` decoration.
Because the input is an arbitrary explicit outside-repository file, this
smoke receipt proves neither its DXC provenance nor corpus membership. Its
claim is limited to one qualified-validator reference-valid result and
inventory conditional on the typed scalar-layout device contract; it makes no
artifact-provenance, corpus, adapter, wgpu, pipeline, runtime, parity, or
performance claim.

## M2.5 status handoff

- **M2.5a — reference-valid corpus:** the additive mechanism is integrated.
  The v2 source-build receipt verified successfully and is retained historical
  evidence. Its first-row smoke correctly failed receipt-less because the old
  standard-layout validator policy did not model DXC's scalar-layout device
  requirement. This v2 policy/receipt-contract correction requires review
  before a fresh v3 build. No corpus qualification claim exists yet.
- **M2.5b — wgpu-ingestible owned shaders:** separately port the required
  shaders to checked WGSL/Naga IR with exact feature/limit gates. M2.5a cannot
  close this ticket.
- **M2.5c — portability/capability strategy:** qualify the non-uniform binding
  fast path and a typed fallback for adapters lacking the feature or binding
  limit. Unsafe passthrough, `strict_capabilities=false`, and stripping
  `NonUniform` remain excluded.

The umbrella M2.5 is complete only when the canonical plan's individually
stated a/b/c gates are satisfied. Reference-corpus completion alone is not
runtime admission or parity.
