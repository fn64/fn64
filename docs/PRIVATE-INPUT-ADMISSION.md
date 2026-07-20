# Private-input admission

Status: local admission mechanism complete; no private game content, path,
length, or content hash is tracked by this repository.

`tools/private_input_admission.py` validates private inputs before an
Extended-GBI fixture or full-ROM release run consumes them. It never copies
the inputs. The populated manifest and emitted readiness report must remain in
`/private/tmp` or another path outside the repository (a repository-local path
is accepted only when git itself confirms it is ignored).

This is an admission check, not behavioral evidence. In particular,
`ready_for_runtime_recognition` means the text/data pair matched its local
manifest and may be submitted to RT64's existing recognition gate. It does not
claim that pinned RT64 recognized the pair, that Extended GBI executed, or
that a full-ROM release matrix closed.

## Local-only manifest

The schema is `fn64.private-input-admission.v3`. A populated manifest contains
private paths, exact lengths, and SHA-256 values and must never enter git. Use
this shape, replacing every angle-bracket placeholder locally. Quoted length
placeholders must become JSON integers, not strings:

```json
{
  "schema": "fn64.private-input-admission.v3",
  "purpose": "combined",
  "intent": {
    "wire_family": "f3dex2_extended_gbi_v1",
    "report_scenario": "<release-report-scenario>",
    "recognition": "runtime_must_confirm_rt64_known_pair",
    "program_evidence_lane": "typed_block_program",
    "extended_gbi_cases": [
      "activation",
      "disabled-negative-control",
      "hook-control",
      "interpolation",
      "vertex-z",
      "widescreen"
    ]
  },
  "release_matrix": {
    "platform": "macos_arm64",
    "controllers": ["standard_controller"],
    "save": "no_cartridge_save",
    "renderers": ["rt64_lle_accuracy", "rt64_post_vi_capture"],
    "repeat_bar": 10
  },
  "artifacts": {
    "microcode_text": {
      "path": "/private/tmp/<text-file>",
      "length": 4096,
      "sha256": "<lowercase-sha256>",
      "provenance": "user_owned_rom_derived",
      "git_identity": "excluded"
    },
    "microcode_data": {
      "path": "/private/tmp/<data-file>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "provenance": "user_owned_rom_derived",
      "git_identity": "excluded"
    },
    "rom": {
      "path": "/private/tmp/<owned-rom>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "provenance": "user_owned_rom",
      "git_identity": "excluded"
    },
    "recompiled": {
      "path": "/private/tmp/<recompiled-artifact>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "provenance": "user_generated_from_owned_rom",
      "git_identity": "excluded"
    }
  }
}
```

`microcode_text` and `microcode_data` are always required. The text length is
exactly 4096 bytes because the fixture must install the complete admitted IMEM
image. `rom` and `recompiled` may be `null` for `extended_gbi`; both are
required for `full_rom` and `combined`.

`report_scenario` is a content-free release label: 1–128 lowercase letters,
digits, dots, underscores, or hyphens, beginning with a letter or digit. A raw
64-character content hash is rejected as a scenario label.

`program_evidence_lane` is the pre-run executable-authority contract. A
`full_rom` or `combined` run must select `typed_observed_function`,
`typed_block_program`, or `identified_native_archive`; the resulting v16
report must carry that exact execution-destination source.
`typed_observed_function` asserts that the host installed the generated
artifact's `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` marker and that the
committed-boundary stream is authoritative. The stale name `typed_function`
is rejected because it makes no such assertion, and
`unidentified_native` is rejected because no stable archive identity is
bound. Use `no_program_fixture` only for an `extended_gbi`-only fixture that
executes no full-ROM program. These failures occur during admission, before a
ten-run series spends time booting the game.

The release policy mirrors the typed release-matrix vocabulary:

- one platform: `macos_arm64`, `linux_x86_64`, or `windows_x86_64`;
- one cartridge-save mode and one or more declared controller features;
- `reference_lle_accuracy` alone, or `rt64_lle_accuracy` with optional RT64
  capabilities;
- Extended GBI specifically requires `rt64_lle_accuracy` plus
  `rt64_post_vi_capture`;
- the deterministic repeat bar is exactly ten.

These values are pre-run admission policy only. Matrix v5 does not copy them
into a certification denominator or trust them as scenario coverage: it derives
platform, controller, save, renderer, program-lane, and committed RSP/RDP-
mechanism coverage from each validated committed-boundary report. A backend-
recognized microcode family is diagnostic/optimization evidence only.
Public-microcode credit requires independent exact digest-to-family
adjudication by the immutable project-owned catalog v1, which is currently
empty pending allowed-source digest provenance; v13 therefore cannot yet
satisfy any public-microcode requirement. Schema v16's coarse host platform
still cannot satisfy an exact platform/API target. A readiness receipt cannot
satisfy ROM-class, TV-region, public-microcode, platform-case, or blocker
requirements by itself.

## Filesystem and identity rules

Admission rejects:

- relative paths or `..` traversal;
- a symlink in any path component;
- directories, sockets, devices, FIFOs, and all other non-regular files;
- repository-tracked or repository-local non-ignored inputs;
- missing/extra manifest fields, unsupported provenance labels, zero or
  excessive lengths, length drift, and SHA-256 drift;
- a reduced six-case Extended-GBI denominator;
- full-ROM/combined admission without both ROM and recompiled artifacts;
- full-ROM admission with stale `typed_function`, `unidentified_native`, or
  `no_program_fixture` program evidence;
- reference-renderer and RT64 capabilities mixed in one release scenario.

The manifest itself is subject to the same local-only, regular-file, no-symlink
policy. Output refuses to overwrite an existing report.

## Content-free readiness report

The emitted and revalidated schema is `fn64.private-input-readiness.v3`.

Run admission with explicit absolute paths:

```sh
python3 tools/private_input_admission.py \
  --manifest /private/tmp/fn64-private-input.json \
  --report /private/tmp/fn64-private-readiness.json

python3 tools/private_input_admission.py \
  --verify-readiness /private/tmp/fn64-private-readiness.json
```

The readiness report contains policy labels, the selected program-evidence
lane, admitted role names, and
ready/not-supplied states only. It contains no input path, filename, byte
length, SHA-256, manifest identity, or private byte-derived digest. It is safe
as a content-free handoff, but it is not a substitute for retaining the local
manifest during the run.

## Synthetic CI gate

The CI-friendly gate creates deterministic non-game bytes in a temporary
external directory, proves a valid admission, then proves rejection of hash
drift, a shrunken case denominator, missing combined inputs, non-regular files,
tracked repository files, symlinks, and report overwrite:

```sh
python3 tools/private_input_admission.py --check
python3 tools/private_input_admission.py --selftest
```

No private fixture is required for these checks, and none of the synthetic
paths, lengths, or hashes is written into a tracked file.

This admission self-test is separate from the live non-default
`extended-gbi-evidence` renderer fixture. That fixture substitutes RT64's
built-in F3DEX2 dialect only after proving normal `process_task` still rejects
the synthetic state; it never claims that its hand-authored bytes form a
recognized text/data pair. Neither synthetic mechanism can satisfy
`ready_for_runtime_recognition` or close the private recognized-microcode
denominator above.
