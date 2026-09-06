# Profile-guided release workflow

fn64's retained WM2000 experiment found an 8.86% lower mean field time and a
2.96 ms lower p99 on its two trained routes. That is evidence for those frozen
binaries and routes, not a universal PGO claim. The reusable mechanism is
[`scripts/pgo-release.py`](../scripts/pgo-release.py): it builds, trains, and
consumes a profile only through an explicit game-owned manifest.

The ordinary release remains the default. Nothing in the workspace's Cargo
profiles silently enables a local `.profdata` file.

## What the workflow binds

The manifest names all inputs the generic fn64 repository cannot infer:

- exact Cargo and rustc commands and the target triple;
- one release build command and its executable inside a workflow-owned target;
- base Rust flags shared by the instrumentation and profile-use builds;
- every training command, working directory, and environment value;
- every ambient environment variable a build or route may inherit;
- explicit identity files that bind the source/build graph.

Use a project-owned source receipt as an identity file when it already hashes
the complete generated-source graph. Otherwise list the relevant manifests,
lockfile, build scripts, generated-source receipts, configuration, and source
files individually. A short list containing only `Cargo.lock` does not bind a
build's source. Directories are rejected because their membership can change
without changing a directory inode.

The successful training receipt additionally binds the canonical manifest,
exact Cargo/rustc launchers, their version output, the `llvm-profdata` binary,
declared identity-file digests, inherited-environment value digests, instrumented
artifact, every raw profile, merged `.profdata`, and ordered route identifiers.
The profile-use build rechecks the manifest, compiler, source denominator,
build environment, and merged profile before invoking Cargo. A mismatch fails
rather than falling back to an unverified profile.

This is a local integrity wire, not a signature, trusted-build attestation, or
performance evidence. The manifest author must make the identity denominator
complete. Keep manifests, logs, profiles, ROM paths, and game-derived artifacts
outside fn64 unless they contain no private or game-derived content and are
intentionally suitable for git.

## Manifest

Paths may be absolute or relative to the manifest. Commands are JSON argument
arrays and never pass through a shell. `{artifact}`, `{target}`, `{target_dir}`,
`{output_dir}`, `{profile_dir}`, and `{merged_profile}` are the only
placeholders; `{artifact}` is available only to training commands.

```json
{
  "schema": "fn64.pgo-training-manifest.v1",
  "schema_version": 1,
  "profile_id": "my-game-representative-v1",
  "target": "aarch64-apple-darwin",
  "toolchain": {
    "cargo": ["rustup", "run", "stable", "cargo"],
    "rustc": ["rustup", "run", "stable", "rustc"]
  },
  "build": {
    "arguments": [
      "build",
      "--release",
      "--locked",
      "--target",
      "{target}",
      "--package",
      "my-game"
    ],
    "cwd": "/absolute/path/to/game-workspace",
    "artifact": "{target_dir}/{target}/release/my-game",
    "rustflags": ["-Ccodegen-units=1"],
    "environment": {},
    "inherit_environment": ["PATH", "HOME", "TMPDIR"]
  },
  "training": [
    {
      "id": "entrance-route",
      "command": [
        "{artifact}",
        "--headless-training-route",
        "entrance",
        "--steps",
        "1500000",
        "--rom",
        "/private/path/to/user-owned.rom"
      ],
      "cwd": "/absolute/path/to/game-workspace",
      "environment": {},
      "inherit_environment": ["PATH", "HOME", "TMPDIR"]
    },
    {
      "id": "shell-route",
      "command": [
        "{artifact}",
        "--headless-training-route",
        "shell",
        "--fields",
        "1200",
        "--rom",
        "/private/path/to/user-owned.rom"
      ],
      "cwd": "/absolute/path/to/game-workspace",
      "environment": {},
      "inherit_environment": ["PATH", "HOME", "TMPDIR"]
    }
  ],
  "identity_files": [
    {"id": "workspace-lock", "path": "/absolute/path/to/game-workspace/Cargo.lock"},
    {"id": "source-receipt", "path": "/private/path/to/source-receipt.json"}
  ]
}
```

The two route shapes illustrate the retained experiment's corpus: one complete
1.5-million-step entrance run and one 1,200-field shell run. Their command-line
syntax is illustrative. No such command, game path, ROM variable, or private
checkout is built into fn64.

`build.arguments` must use `--release`, `--locked` or `--frozen`, and an
explicit `--target`. The target prevents PGO flags from instrumenting Cargo
build scripts, following the Rust compiler's documented Cargo workflow.
`build.artifact` must resolve inside the isolated target directory. The
workflow owns `CARGO_TARGET_DIR`, `CARGO_INCREMENTAL`, `RUSTFLAGS`,
`CARGO_ENCODED_RUSTFLAGS`, and `LLVM_PROFILE_FILE`; ambient or manifest
attempts to override them fail loudly.

Environment inheritance is allowlisted rather than ambient. Add platform
requirements such as `SystemRoot`, `SDKROOT`, `DEVELOPER_DIR`, `CARGO_HOME`,
or `RUSTUP_HOME` explicitly when needed. Do not put credentials in a manifest;
prefer an authenticated local dependency cache or a separate narrow mechanism.

## Commands

Use `llvm-profdata` from the compiler's `llvm-tools-preview` component when
possible. It is not normally added to `PATH`; pass its exact path. The tool
used for a successful merge is retained in the profile receipt.

Train and build in one invocation:

```sh
python3 scripts/pgo-release.py all \
  --manifest /private/path/pgo.json \
  --output-dir /private/path/pgo-output-v1 \
  --llvm-profdata /absolute/path/to/llvm-profdata
```

Training and consumption can be separated. `optimize` and `verify-profile` do
not need `llvm-profdata`; they verify the compiler and merged-profile identity.

```sh
python3 scripts/pgo-release.py train \
  --manifest /private/path/pgo.json \
  --output-dir /private/path/pgo-output-v1 \
  --llvm-profdata /absolute/path/to/llvm-profdata

python3 scripts/pgo-release.py verify-profile \
  --manifest /private/path/pgo.json \
  --output-dir /private/path/pgo-output-v1

python3 scripts/pgo-release.py optimize \
  --manifest /private/path/pgo.json \
  --output-dir /private/path/pgo-output-v1
```

An ordinary release uses the same declared build and identity denominator but
does not require LLVM profiling tools or a trained profile:

```sh
python3 scripts/pgo-release.py ordinary \
  --manifest /private/path/pgo.json \
  --output-dir /private/path/ordinary-output-v1
```

Training requires a new empty output directory. Profile-use and ordinary
target directories must not exist. The script never deletes an old profile or
target and requires the output root to remain outside the fn64 repository.

## Promotion bar

Producing a build receipt does not promote that binary. For each game and
representative corpus:

1. freeze the ordinary and profile-use executables before timing;
2. run three or more interleaved full-route pairs on a quiet machine;
3. prove the established framebuffer, audio, simulation-time, and endpoint
   identities on every run;
4. retain PGO only when a pre-registered threshold is met with non-overlapping
   run ranges;
5. record binary-size growth and untrained-function warnings beside the result.

The original experiment required at least a 1% mean improvement and cleared it
with 8.86%. New games and materially changed corpora need their own A/B. A
profile receipt proves compatibility with declared inputs; it cannot prove a
speedup.
