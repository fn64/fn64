# Private-input admission

Status: local admission, typed program/microcode-kickoff identity checks, and
the trusted series mechanism are wired. Representative private NTSC full-ROM
reference and RT64 LLE/post-VI exact-ten series completed under schema v22 and
were reverified on 2026-07-22. Their private content remains outside the
repository. Combined with the retained public synthetic identified-native
XBUS series, the matrix assessment accepted 3 scenarios and 30 reports,
satisfied 12 of 162 requirements, and retained the other 150 as explicit
gaps. The public series receives no private-input or ROM-class authority.

`tools/private_input_admission.py` is the manifest/readiness producer and a
differential oracle for the typed Rust policy; it is not loader authority.
Extended-GBI, full-ROM, and F3DZEX2 consumers revalidate admission in process
before consuming private bytes. The Python producer never copies the inputs.
The populated manifest and emitted readiness report must remain in
`/private/tmp` or another path outside the repository (a repository-local path
is accepted only when git itself confirms it is ignored).

This is an admission check, not behavioral evidence. In particular,
`ready_for_runtime_recognition` means the text/data pair matched its local
manifest and may be submitted to RT64's existing recognition gate. It does not
claim that pinned RT64 recognized the pair, that Extended GBI executed, or
that a full-ROM release matrix closed.

## Local-only manifest

The current schema is `fn64.private-input-admission.v7`. A populated manifest contains
private paths, exact lengths, and SHA-256 values and must never enter git. Use
this shape, replacing every angle-bracket placeholder locally. Quoted integer
placeholders (`length` and `release_gate_cycle`) must become JSON integers,
not strings:

```json
{
  "schema": "fn64.private-input-admission.v7",
  "purpose": "combined",
  "intent": {
    "wire_family": "f3dex2_extended_gbi_v1",
    "report_scenario": "<release-report-scenario>",
    "recognition": "runtime_must_confirm_backend_known_pair",
    "program_evidence_lane": "typed_block_program",
    "rom_class": "retail_cartridge",
    "characterization_suite": null,
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
    "microcode_text_raw_window": null,
    "microcode_data_raw_window": null,
    "rom": {
      "path": "/private/tmp/<owned-rom>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "provenance": "user_owned_retail_cartridge_dump",
      "git_identity": "excluded"
    },
    "recompiled": {
      "path": "/private/tmp/<recompiled-artifact>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "provenance": "user_generated_from_owned_rom",
      "git_identity": "excluded"
    }
  },
  "runner": {
    "executable": {
      "path": "/private/tmp/<prebuilt-native-host>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "git_identity": "excluded"
    },
    "working_directory": "/private/tmp/<run-directory>",
    "argv": [],
    "env": {
      "OOT_MAX_STEPS": "<fixed-value>"
    },
    "release_gate_cycle": "<nonnegative-u64-integer>",
    "execution_source": {
      "kind": "typed_block_program",
      "program_sha256": "<lowercase-sha256>",
      "dispatch_artifact_sha256": "<lowercase-sha256>"
    },
    "program_build_receipt": {
      "path": "/private/tmp/<program-build-receipt.json>",
      "length": "<exact-positive-integer>",
      "sha256": "<lowercase-sha256>",
      "git_identity": "excluded"
    }
  }
}
```

`microcode_text` and `microcode_data` are required for `extended_gbi`,
`full_rom`, and `combined`. The text length is exactly 4096 bytes because the
fixture must install the complete admitted IMEM image. The two raw-window
roles must be `null` for those purposes. `rom` and `recompiled` may be `null`
for `extended_gbi`; both are required for `full_rom` and `combined`.

### F3DZEX2 characterization input

Purpose `f3dzex2_characterization` admits native RDRAM-storage recognition
windows without treating those storage bytes as logical N64 byte order. Its
intent is fixed to `wire_family: "f3dzex2"`, an empty
`extended_gbi_cases` array, `program_evidence_lane: "no_program_fixture"`,
`rom_class: "not_applicable"`, and
`characterization_suite: "fn64.f3dzex2-point-light.v1"`. Every other v7
purpose must set `characterization_suite` to JSON `null`. Retained v6
manifests keep their exact older intent object and do not contain this field.
The suite identifier selects only the repository-owned vector denominator;
the manifest cannot provide commands, expected results, a microcode variant,
or additional cases. Characterization requires RT64 LLE and post-VI capture
in the release policy. Its artifact object has this exclusive shape:

```json
{
  "microcode_text": null,
  "microcode_data": null,
  "microcode_text_raw_window": {
    "path": "/private/tmp/<raw-text-window>",
    "length": 6352,
    "sha256": "<lowercase-sha256>",
    "provenance": "user_owned_rom_derived",
    "git_identity": "excluded"
  },
  "microcode_data_raw_window": {
    "path": "/private/tmp/<raw-data-window>",
    "length": 4032,
    "sha256": "<lowercase-sha256>",
    "provenance": "user_owned_rom_derived",
    "git_identity": "excluded"
  },
  "rom": null,
  "recompiled": null
}
```

The lengths are exactly `0x18d0` text-storage bytes and `0x0fc0`
data-storage bytes, matching the native RT64 adapter recognition boundary.
After admission, a characterization consumer copies those bytes directly to
non-overlapping scratch RDRAM ranges. It derives the logical 4096-byte IMEM
image and logical `0x0fc0` task-data image through the normal RDRAM byte-lane
view, then loads/hashes those derived logical images. It must retain the
original storage ranges unchanged for raw recognition. No independently
supplied logical artifact may override or contradict that derivation.

The characterization runner uses the boot harness's narrow typed Rust loader.
That loader revalidates the current v7 characterization scope, derives the
canonical content-free readiness bytes and requires the supplied readiness to
exact-match them, then returns only the two fixed-size raw windows. Each raw
window is read and hashed through one no-follow descriptor or Windows handle;
the returned bytes are the capture from that same stable handle, not a later
pathname reopen. Detailed failures stay inside the harness and the public
error is content-free. Python remains only the producer/oracle for this path.

This purpose is characterization intake only. Readiness does not admit
F3DZEX2 HLE, convert its diagnostic family into public-microcode credit, or
authorize relabeling it as F3DEX2. A production catalog remains closed until
the separate behavioral gates are met.

`rom_class` is `retail_cartridge` or `public_homebrew` for a full-ROM run,
and exactly `not_applicable` for an Extended-GBI-only fixture. The ROM
descriptor must carry the matching class-specific provenance:
`user_owned_retail_cartridge_dump` or
`publicly_distributed_homebrew_rom`. The header cannot distinguish these
classes. This is a contract-bound local provenance attestation, not a claim
that header bytes prove retail or homebrew origin; independently transferable
public-homebrew provenance would require a project-owned digest/build-receipt
catalog or an external attestation.

`report_scenario` is a content-free release label: 1–128 lowercase letters,
digits, dots, underscores, or hyphens, beginning with a letter or digit. A raw
64-character content hash is rejected as a scenario label.

`program_evidence_lane` is the pre-run executable-authority contract. A
`full_rom` or `combined` run must select `typed_observed_function`,
`typed_block_program`, or `identified_native_archive`; the resulting v22
report must carry that exact execution-destination source.
`typed_observed_function` asserts that the host installed the generated
artifact's `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` marker and that the
committed-boundary stream is authoritative. The stale name `typed_function`
is rejected because it makes no such assertion, and
`unidentified_native` is rejected because no stable archive identity is
bound. Use `no_program_fixture` only for an `extended_gbi`-only fixture that
executes no full-ROM program. These failures occur during admission, before a
ten-run series spends time booting the game.

The v7 `runner` section retains the v6 contract: it binds the exact native entry image, program-build
receipt, working directory, argument vector, fixed child environment, gate
cycle, and expected v22 execution source. The executable and receipt must be
built before admission. The trusted runner clears the ambient environment,
launches the executable directly, and owns `ROM`,
`FN64_RELEASE_ROM_CLASS`, the gate/report/event variables, and the staged
microcode paths; manifests cannot override them. ELF, Mach-O, and PE entry
images are accepted by the Rust runner;
scripts and interpreter-mediated launchers are rejected. Loader, interpreter,
plugin, shader/ICD, and search-path override variables are also rejected,
including the `LD_*`, `DYLD_*`, `PYTHON*`, `VK_*`, and analogous families.
This constrains entry-image provenance; it does not pretend that one executable
SHA-256 attests every system framework or GPU driver loaded by the process.

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
empty pending allowed-source digest provenance; matrix v18 therefore cannot yet
satisfy any public-microcode requirement. Schema v22 binds RT64's resolved
graphics API independently of the requested settings and derives
`macos-metal` or `linux-vulkan` target credit only from an authoritative
matching RT64 post-VI report. Windows D3D12 and Vulkan are distinguished, and
an exact native workstation build/UBR derives Windows 10 versus 11; no positive
Windows report has been retained. A readiness
receipt, scenario label, reference renderer, or coarse host platform cannot
satisfy ROM-class, TV-region, public-microcode, platform-case, or blocker
requirements by itself. TV-region credit is derived from normalized header,
device, and renderer agreement. ROM-class credit additionally requires the
opaque verified-series authority built from the v3 contract, exact-ten receipt,
retained output files, and exact runner image; report-only matrix verification
cannot promote a host-authored class.

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
- a missing/unknown ROM class, ambiguous legacy `user_owned_rom` provenance,
  or a retail/public-homebrew class and provenance mismatch;
- an authoritative program lane without a private
  `fn64.release-program-build-receipt.v1`, or with a receipt whose bytes,
  child, lane, recompiled input, canonical digest, or recomputed execution
  source drift;
- full-ROM admission with stale `typed_function`, `unidentified_native`, or
  `no_program_fixture` program evidence;
- reference-renderer and RT64 capabilities mixed in one release scenario;
- case-variant paths that resolve to tracked repository files on a
  case-insensitive filesystem;
- reserved or code-injecting runner environment variables.

The manifest itself is subject to the same local-only, regular-file, no-symlink
policy. Input measurement uses no-follow descriptors and detects identity,
length, or timestamp changes during hashing. Output is staged, flushed, and
published without replacement; rejected output leaves no final file. A writer
that can concurrently rename external directory ancestors or restore file
metadata remains outside this local single-owner admission guarantee.

## Content-free readiness report

The emitted and revalidated current schema is
`fn64.private-input-readiness.v6`.

Run admission with explicit absolute paths:

```sh
python3 tools/private_input_admission.py \
  --manifest /private/tmp/fn64-private-input.json \
  --report /private/tmp/fn64-private-readiness.json

python3 tools/private_input_admission.py \
  --verify-readiness /private/tmp/fn64-private-readiness.json
```

The readiness report contains policy labels, the selected program-evidence
lane, the content-free ROM-class attestation, a
`verified`/`not_applicable` program-build-receipt state, admitted role
names, and ready/not-supplied states only. It contains no input path, filename,
byte length, SHA-256, manifest identity, or private byte-derived digest. It is
safe as a content-free handoff, but it is not a substitute for retaining the
local manifest during the run.

Characterization readiness names only the purpose and the two admitted raw
role labels. It also derives these fixed, content-free values from the suite
identifier rather than accepting them from the manifest:

- `characterization_fixture: "ready_for_controlled_native_evidence"`;
- `characterization_suite: "fn64.f3dzex2-point-light.v1"`;
- `characterization_vector_source: "repository_generated"`;
- `required_characterization_cases`, in exact sorted order:
  `directional-light-control`, `lighting-disabled-control`,
  `point-light-far-distance`, `point-light-near-distance`,
  `point-light-negative-axis`, `point-light-positive-axis`,
  `point-light-record-boundary`, and `point-light-zero-distance`.

Its Extended-GBI and full-ROM states are respectively `not_requested` and
`not_supplied`. Current readiness for every other purpose uses `not_requested`
for the three characterization labels and an empty characterization-case
array. No readiness field contains a private path, filename, length, digest,
derived logical identity, command payload, expected result, or variant choice.
The typed characterization loader compares the complete supplied readiness
document byte-for-byte with the canonical serialization derived from the
revalidated manifest; semantically similar or noncanonical JSON is rejected.

For retained evidence, contract and readiness verification continue to accept
the exact v6 manifest/v5 readiness vocabulary. That compatibility branch is
read-only: a new `--manifest` admission must use v7. It does not accept
`f3dzex2_characterization`, the `f3dzex2` wire-family label, the raw-window
roles, `characterization_suite`, the current readiness characterization
fields, or any other v7/v6-current field shape. New admission is emitted as
v7/v6. This
preserves revalidation of retained v3 contracts without silently broadening
their older schema.

## Private run contract and trusted series

For `full_rom` or `combined`, admission can also emit the content-bearing
`fn64.private-release-run-contract.v3` file:

```sh
python3 tools/private_input_admission.py \
  --manifest /private/tmp/fn64-private-input.json \
  --report /private/tmp/fn64-private-readiness.json \
  --emit-private-run-contract /private/tmp/fn64-private-run.json

python3 tools/private_input_admission.py \
  --verify-private-run-contract /private/tmp/fn64-private-run.json
```

The contract binds the manifest, readiness, and program-build-receipt bytes;
purpose; ROM class; scenario; cycle; exact ROM and admitted artifacts; execution source;
native child image; working directory; arguments; and sorted environment. Its
canonical SHA-256 wire uses domain
`fn64.private-release-run-contract-digest.v3\0`, u64
big-endian lengths/counts, raw 32-byte hashes, and typed execution-source
tags. This is integrity plus policy equivalence, not an authority signature.

The Rust loader therefore does not accept a deserialized contract directly.
It returns an opaque verified-contract type only after the in-process Rust
policy revalidates the full v7/v6 manifest/readiness/receipt/v3 contract
mapping, including the strict retained v6/v5 branch. The contract and every
referenced file are opened without following links; hashing, native-image
inspection, and Unix execute-mode inspection use the retained descriptor or
Windows handle, and object/path-chain identity is checked after the read.
`tools/private_input_admission.py` remains the current create-new producer and
a differential oracle, but the production loader neither embeds nor launches
it. Repository-owned synthetic mechanism tests
use a separate constructor that accepts only the exact fixed non-game
manifest/readiness/input bytes, scenario, cycle, empty admitted-artifact set,
`NoProgram` source, and current test executable. Caller-labelled synthetic
input cannot mint authority.

The in-process loader and its Windows handle implementation compile for
`x86_64-pc-windows-msvc`; that removes the former `/usr/bin/python3` layout
blocker but is not native Windows execution evidence. Retained verification also
requires the exact runner image named by the receipt, so the certifying binary
must be preserved alongside the receipt and evidence. The CLI's
`--print-contract-sha256` mode recomputes integrity only; its output is never
admission authority.

`run-private-release-series` owns a random series nonce and launches exactly
ten children sequentially with an empty ambient environment. It copies the
verified child bytes to a create-new executable beside the original and runs
only that read-only isolated stage. Production runs likewise copy the admitted
microcode text and data into separate create-new, read-only stages and inject
their paths through runner-owned `FN64_RELEASE_MICROCODE_TEXT_PATH` and
`FN64_RELEASE_MICROCODE_DATA_PATH` variables.
The OoT host reads and shape-checks those exact staged bytes but does not use
them to choose a family. At the live task boundary the ABI independently
classifies the larger raw RDRAM text/data prefixes through pinned MIT RT64's
F3DZEX2 XXH3 rows; the report event must then match the staged logical pair.
This binds recognition without admitting F3DZEX2 HLE or minting
public-microcode credit. Before every launch and after the series
the runner rehashes the contract-bound files, child stage, and both pair
stages. The source contract itself is consumed from one stable captured
descriptor; a later pathname replacement cannot change the bytes parsed by
Rust. This is a local single-owner execution guarantee: a malicious same-UID
process able to discover, chmod, and replace child or microcode stages between
verification and operating-system open/spawn is outside scope. Each child gets a distinct derived event
identity and new report/journal/log paths.
The runner verifies each terminal v3 journal, exact v22
scenario/cycle/input/source, the five fixed-cycle artifacts, live-minimum
closure, zero reached unsupported events, and the admitted microcode pair
before starting the next child. For a
production report, at least one individual recognized microcode event must contain the
admitted text SHA-256 and admitted data length/SHA-256 together with a
recognized family; matches split across events are rejected. A create-new,
flushed, and file-synced `fn64.private-release-series-receipt.v1` binds the
contract, runner and child entry-image hashes, nonce, ten event/file/report
identities, and common semantic report SHA-256.

For production ROM evidence, the runner also requires the report class to
equal contract v3, the raw report input SHA-256 and byte length to equal the
admitted ROM, and independently rereads those exact admitted bytes to
recompute the normalized z64/n64/v64 identity, destination code, decoded TV
region, and configured-TV agreement. A child cannot satisfy this check by
copying the class label while fabricating different header evidence.

Matrix verification keeps that authority boundary explicit. Generic report
verification derives TV-region and other runtime/render coverage but always
leaves ROM-class coverage empty. The private-series path accepts only opaque
capabilities created by jointly revalidating the admitted contract, exact-ten
receipt, retained reports/journals, raw ROM, runner image, bound files, and
program-build receipt. It requires exact semantic-report and ordered run-event
agreement with the matrix evidence and retains
`fn64.verified-rom-class-authority.v1` in matrix v18. That record's digest
detects later drift; it is not a signature and does not replace external
attestation when transferable provenance is required.

That receipt is deliberately an integrity record, not a signature. An
observed runner invocation enforces the direct sequential launches, while
later receipt verification proves only that the retained evidence still
matches the recorded series. Transferable process-provenance claims require
an external trusted CI/code-signing attestation over the receipt and exact
runner image; fn64 does not currently manufacture such an attestation.

### Program-input and runtime task-start identity binding

The private `fn64.release-program-build-receipt.v1` is itself content-bearing
and must remain outside git. It binds the exact child executable and one of
three typed build lanes:

- `native_archives` lists exact archive files in strictly sorted, unique
  canonical-label order. Verification checks each declared length and digest
  in the same streaming pass that recomputes the domain-separated
  linked-archive aggregate used by the live native source.
- `typed_observed_function` binds the exact canonical generated-source
  identity wire; its raw file SHA-256 is the observed-function artifact
  identity.
- `typed_block` binds the exact block pack, whose raw SHA-256 is the dispatch
  artifact identity, plus the expected live program SHA-256.

All lanes bind the exact child path/length/SHA-256, declare the typed expected
execution source, and carry a canonical receipt SHA-256. Admission and Rust
independently reread the files and require the declared source, recomputed
source, contract source, and eventual report source to agree. Exactly one lane
input must equal the admitted `recompiled` descriptor. The manifest binds the
receipt file's own exact identity, and contract v3 binds that descriptor.

Create the receipt from measured files; do not hand-author its JSON. The
materializer sorts native labels, derives the execution source, publishes only
with create-new semantics, file-syncs the result, and reloads it through the
same verifier used by admission:

```sh
cargo run -p fn64-boot-harness \
  --bin materialize-release-program-build-receipt -- \
  native-archives \
  --output /private/tmp/program-build-receipt.json \
  --child /private/tmp/oot-boot \
  --archive generated-code /private/tmp/librecompiled.a \
  --archive section-bridge /private/tmp/libsection_bridge.a

cargo run -p fn64-boot-harness \
  --bin materialize-release-program-build-receipt -- \
  typed-block \
  --output /private/tmp/program-build-receipt.json \
  --child /private/tmp/oot-boot \
  --pack /private/tmp/program.pack \
  --expected-program-sha256 LOWERCASE_LIVE_PROGRAM_SHA256
```

For the OoT typed-observed-function lane, the host build embeds the same
path-independent source wire used by its execution-destination identity. Its
private writer publishes those exact bytes, after checking their SHA-256
against the child build identity, for the generic materializer. Invoke it with
the same private `FN64_GAME_DIR` (or explicit `ROM`/`RECOMPILED_DIR`) and
regenerated `RECOMP_RS_DIR` used to build the exact child:

```sh
FN64_GAME_DIR=/absolute/private/game-workspace \
RECOMP_RS_DIR=/absolute/private/generated-rust-crate \
FN64_RECOMP=rs FN64_RS_EXECUTION=function \
  examples/oot-boot/oot identity-wire \
  /private/tmp/oot-function-identity.wire

cargo run -p fn64-boot-harness \
  --bin materialize-release-program-build-receipt -- \
  typed-observed-function \
  --output /private/tmp/program-build-receipt.json \
  --child /private/tmp/oot-boot \
  --identity-wire /private/tmp/oot-function-identity.wire
```

This is identity co-binding, not proof that the child was compiled or linked
from the lane input. That stronger build-provenance claim requires a trusted
linker/build record, embedded signed note, or external CI/code-signing
attestation connecting those identities.

The build receipt does not claim microcode-data consumption. At graphics-task
start, the ABI hashes the exact logical RDRAM bytes at the original task
microcode-data address and length. Report schema `fn64.release-gate.v22`
records those fields in
the same recognition event as the live 4 KiB IMEM SHA-256 and recognized
family, using `fn64.rsp-rdp-observations.v2`. Pinned raw-window classification
has priority; an exact backend pair catalog may fill an otherwise unknown
identity but cannot contradict the classifier. Text-only HLE recognition is
insufficient. Replacement IMEM generations
retain the original data identity; yielded resume state cannot masquerade as
the admitted data. A typed one-way lifecycle retires ordinary completion and
consumes each public resume authorization at the next yielded Load. The trusted runner requires every production report to
contain a single event matching the admitted text SHA-256, data length, and
data SHA-256 with a recognized family.

This proves which pair the task named at the authoritative kickoff boundary;
it does not independently trace every later RSP read of the data image.

These checks make a valid production contract launchable. Representative
private reference and RT64 LLE/post-VI exact-ten series completed under v22 on
2026-07-22 and were independently reverified from their retained contracts,
runner, receipts, reports, and journals. The retained private receipts carry
the exact semantic-report and series identities. The matrix path accepted
those 20 private reports plus 10 retained public XBUS reports and emitted a
canonical incomplete-v6 assessment: 12 of 162 FullParityV1 requirements are
satisfied and 150 remain explicit. The public series does not receive the
private-series capability.
The series receipt remains a self-hashed integrity record, not external process
attestation; transferable provenance still requires a trusted CI/code-signing
root over the receipt and runner.

## Synthetic CI gate

The CI-friendly gate creates deterministic non-game bytes in a temporary
external directory, proves a valid admission, then proves rejection of hash
drift, a shrunken case denominator, missing combined inputs, non-regular files,
tracked repository files, symlinks, and report overwrite:

```sh
python3 tools/private_input_admission.py --check
python3 tools/private_input_admission.py --selftest
python3 tools/private_input_admission.py --corpus-snapshot
cargo test -p fn64-boot-harness shared_content_free_corpus_matches_rust_policy
cargo test -p fn64-boot-harness typed_characterization_loader
cargo test -p fn64-render-rt64 --test private_release_runner
```

The content-free `fn64.private-admission-rejection-corpus.v1` recipes add
duplicate-object, path-replacement, case-alias, environment-injection,
receipt/contract-tamper, and retained-schema cases without storing paths,
hashes, or content identities. The Python producer executes every host-capable
recipe and emits only its ordered recipe ID plus `accept`, `reject`, or
capability `skip`. Rust strictly parses the same recipe wire, forces every
`capability=all` recipe to execute, verifies an operation-specific native
rejection cause so unrelated setup failures cannot satisfy a reject recipe,
and produces the same content-free snapshot. On Unix the test suite executes
both implementations and requires their ordered snapshots to match exactly.

The Rust integration test makes the trusted runner launch ten fresh test
processes. Every child executes the real executor, PI/SI/AI device fabric,
graphics-task RSP path, raw RDP submission, reference renderer, and committed
VI boundary before producing its five-channel report and journal. No private
fixture is required for these checks, and none of the synthetic paths,
lengths, or hashes is written into a tracked file.

This admission self-test is separate from the live non-default
`extended-gbi-evidence` renderer fixture. That fixture substitutes RT64's
built-in F3DEX2 dialect only after proving normal `process_task` still rejects
the synthetic state; it never claims that its hand-authored bytes form a
recognized text/data pair. Neither synthetic mechanism can satisfy
`ready_for_runtime_recognition` or close the private recognized-microcode
denominator above.
