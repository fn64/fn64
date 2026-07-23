# Private audio ABI characterization

`audio_abi_characterize` executes privately supplied RSP audio images through
fn64's owned rspboot and speculative LLE kernels. It is a black-box evidence
tool, not an HLE-family detector. Its source model is the public libultra
`OSTask`/audio command-list contract and the SGI *Nintendo 64 RSP Programmer's
Guide* SP DMA model. No GPL runtime implementation is an input.

The tool reads one JSON request:

```sh
cargo run -p fn64-audio --bin audio_abi_characterize -- request.json
```

Every private file has a required exact SHA-256. The harness refuses a
mismatch before execution. Paths and bytes never enter the report. Standard
output is one canonical compact JSON value containing only fixture revision,
fixed experiment-axis names, verified/captured digests, terminal and
instruction counts, diagnostic DMA records, phase-separated mutation ranges
with digests, and selected whole-image/sentinel digests. It never serializes
microcode, ROM, PCM, RDRAM, DMEM, or IMEM bytes. Caller-controlled case IDs,
packet words, layouts, detailed axis values, paths, and sentinel parameters are
also omitted rather than being mislabeled share-safe.

## Request v1

The following is shape documentation; digest and path placeholders are not a
fixture or an admitted private identity.

```json
{
  "schema": "fn64.audio-abi-characterization-request.v1",
  "fixture_revision": 1,
  "microcode": {
    "rspboot_path": "/private/input/rspboot.bin",
    "rspboot_sha256": "<64 hexadecimal characters>",
    "text_path": "/private/input/text.bin",
    "text_sha256": "<64 hexadecimal characters>",
    "data_path": "/private/input/data.bin",
    "data_sha256": "<64 hexadecimal characters>"
  },
  "layout": {
    "task_address": 64,
    "rspboot_address": 256,
    "text_address": 4096,
    "data_address": 12288,
    "command_address": 16384
  },
  "cases": [
    {
      "id": "count-boundary-8",
      "parameters": { "kind": "count", "opcode": 2, "count": 8 },
      "sentinels": [
        { "start": 32768, "byte_len": 64, "pattern_seed": 90 }
      ],
      "phases": [
        { "packets": [{ "word0": 33554432, "word1": 0 }] }
      ]
    }
  ]
}
```

Packet words are exact hand-authored inputs. The adjacent typed
`parameters` value labels the question being asked; it does not change or
reinterpret those words. The supported question shapes cover address and
alignment, selector, count, DMEMMOVE overlap direction, `A_AUX` buffer fields,
reserved-bit masks, and cross-task persistence. A persistence case declares
`task_count >= 2`, supplies exactly that many phases, and carries the prior
task's RDRAM, RSP memory, and complete architectural scalar/vector state into
the next rspboot. Every non-persistence case must contain exactly one phase;
state carry cannot occur under a label that claims a single-task experiment.

The report binds the omitted inputs with domain-separated SHA-256 values. A
request digest covers the fixed schema/revision, observed input identities,
layout, and ordered case digests. Each case digest covers its hidden ID, typed
axis values, sentinels, and ordered phase digests. Each phase digest covers its
ordered packet words. These are content addresses, not admission signatures;
they make two reports joinable to the same private request without echoing its
caller-controlled strings or words.

The digest wire is host-independent: strings and lists carry an unsigned
64-bit big-endian byte/item count; strings then carry UTF-8 bytes; numeric
fields use their declared unsigned width in big-endian order; enum variants
begin with the fixed ASCII axis name and fixed one-byte subvariant tags.
Nested phase and case values enter their parent as exactly 32 digest bytes.
The domain strings are versioned in `characterize.rs`; changing any shape
requires a new domain and report schema.

Case IDs are restricted to ASCII letters, digits, `.`, `_`, and `-`, keeping
the canonical wire independent of Unicode normalization. The harness
constructs a public 64-byte audio `OSTask`, installs the exact
packet list and deterministic sentinels, runs rspboot to the owned task-entry
boundary, then runs the captured image to BREAK. Inputs and sentinels must be
disjoint within physical 8 MiB RDRAM. Task and command addresses must be
8-byte aligned. An empty command phase is rejected rather than becoming a
silent no-op.

## Diagnostic DMA journal

Each accepted SP DMA records, in execution order:

- direction (`read` or `write`);
- effective DRAM address after hardware low-bit masking;
- SP memory address including its DMEM/IMEM bank selector;
- raw length/count/skip descriptor written by the microcode.

The event is appended only after the complete rectangular DMA passes physical
range and admission checks and before any transfer mutation. A rejected DMA
therefore produces no event. The journal is not part of `RspMachineState`, an
audio task outcome, or any verified commit token. Consuming a speculative LLE
result into effects drops the journal; diagnostics cannot become execution
authority accidentally.

While the harness executes, a thread-local content-safe diagnostic guard also
suppresses ambient instruction, register, DMEM, CP0, and DMA-word traces.
Unknown scalar words, vector operation names, and delay-slot instructions are
redacted in both stderr and the typed unsupported-event context.

## Evidence boundary

A report characterizes exactly the supplied request and hashes. It does not
identify a named ABI family, establish expected semantics, certify HLE, or
authorize release fallback. Expected behavior must be established separately
through a predeclared experiment matrix and repeated observations. Private
reports and inputs stay outside git; only the hand-authored public smoke
fixture in this crate is repository content.
