# M2 DXC artifact validator

This standalone workspace is the independent Rust/wgpu half of M2.4's shader
artifact qualification. It pins wgpu 30.0.0 and uses its deterministic noop
backend, so validation does not depend on a machine's graphics adapter or
driver. The noop backend does not itself surface shader errors, so the probe
explicitly executes the same pinned naga parser, all-flags validator, and
wgpu-naga-bridge baseline feature capability mapping used by wgpu-core 30 before calling
`Device::create_shader_module` with checked runtime checks. A small independent SPIR-V instruction scan
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
fn64-wgpu-shader-validator --shader module.spv --stage compute --entry CSMain
```

Both commands emit one path-free JSON object. Validation returns 0 only when
the module is well-formed, the selected entry/stage pair exists, the explicit
wgpu 30 parser/validator-equivalent path passes, and the wgpu API error scope
is empty. Invalid input returns 2. Receipts and Cargo
target output are machine evidence and must not enter git.
