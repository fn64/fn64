# M2 DXC artifact validator

This standalone workspace is the independent Rust/wgpu half of M2.4's shader
artifact qualification. It pins wgpu 30.0.0 and uses its deterministic noop
backend, so validation does not depend on a machine's graphics adapter or
driver. The noop backend does not itself surface shader errors, so the probe
explicitly executes the same pinned naga parser, all-flags validator, and
wgpu-naga-bridge feature-capability mapping used by wgpu-core 30 before calling
`Device::create_shader_module` with checked runtime checks. The selected closed
profile supplies the exact same feature and limit contract to both validation
paths. A small independent SPIR-V instruction scan
also proves that the requested entry point and execution model exist; it is an
identity check, not a substitute for wgpu validation.

The validator is not a runtime dependency and this workspace is deliberately
excluded from fn64's root Cargo workspace. Build it only through the artifact
tool, which descriptor-stably stages these reviewed files outside fn64's Cargo
configuration ancestry, invokes Cargo from the configuration-checked filesystem
root, uses a new controlled Cargo home and target directory with direct toolchain binaries,
remaps that isolated root to a stable virtual source path, passes `--locked`,
and emits a source/tool/dependency/binary receipt:

```sh
python3 tools/rt64_shader_artifacts.py build-validator \
  --output-dir /outside/repo/wgpu-validator
```

The stable protocol is:

```sh
fn64-wgpu-shader-validator --fn64-version
fn64-wgpu-shader-validator --profile baseline --shader module.spv --stage compute --entry CSMain
fn64-wgpu-shader-validator --profile immediates-8 --shader module.spv --stage fragment --entry PSMain
```

The only admitted profiles are `baseline` and `immediates-{4,8,16,20,24,32,40,56}`. Immediate size is Naga's alignment-rounded push-constant struct span, not merely the unrounded end of the last member.
The validator derives the module's minimum Immediate byte span and rejects both
underprivileged and overprivileged selections. Immediate profiles enable exactly
wgpu `IMMEDIATES` and set only `max_immediate_size` above its default. Raw feature
or limit arguments are not part of the protocol.

Both commands emit one path-free JSON object. Validation returns 0 only when
the module is well-formed, the selected entry/stage pair exists, the explicit
wgpu 30 parser/validator-equivalent path passes, and the wgpu API error scope
is empty. Noop validation is not evidence that a native adapter supports the
selected feature or limit. Invalid input returns 2. Receipts and Cargo
target output are machine evidence and must not enter git.
