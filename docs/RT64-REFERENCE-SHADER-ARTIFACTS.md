# RT64 reference shader artifacts

Status: **additive mechanism implemented; source build, 56-row corpus, and qualification not yet run.**

This is M2.5a's additive evidence path for the exact RT64 HLSL denominator. It
produces reference-valid SPIR-V without weakening or replacing the accepted
wgpu-ingestion path in `tools/rt64_shader_artifacts.py`. The existing artifact
producer, its policy, and its wgpu 30 validator remain byte-identical. This
tool imports their qualified source staging, DXC receipt verification, and
three explicit dependency/preprocess/compile phases, and every new receipt
binds both producer identities.

The only claim is `reference-valid-only-not-wgpu-runtime-or-parity`.
Reference validity does not imply that Naga can parse a module, that wgpu can
create it, that a backend can run it, or that its output is visually or
behaviorally equal to RT64. In particular, the preserved v3 attempt failed
closed on row one because Naga 30 rejects the valid `ShaderNonUniform`
capability deliberately emitted for RT64's non-uniform texture/TMEM binding
array accesses.

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
   `spirv-val --target-env vulkan1.0 -` process through stdin, with no relaxed
   validation flags;
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

## M2.5 status handoff

- **M2.5a — reference-valid corpus:** mechanism implemented in this additive
  branch; focused hostiles and independent mechanism review are next. No
  spirv-val source-build or corpus qualification claim exists yet.
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
