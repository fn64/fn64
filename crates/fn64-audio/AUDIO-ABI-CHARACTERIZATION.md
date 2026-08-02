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

For an already characterized compact identity, the companion verifier runs
the compact HLE executor and the clean-room LLE authority from the same
post-rspboot snapshot:

```sh
cargo run -p fn64-audio --bin audio_compact_verify -- request.json
```

It accepts request v2 but requires every trial to contain exactly one phase;
cross-task persistence is rejected. Its content-free v1 report gives the
command and decoded-command counts, exact terminal-DMEM equivalence and hashed
difference ranges, and canonical RDRAM-patch equivalence for every trial. It
does not serialize packets, memory bytes, paths, or caller-controlled case
labels. This verifier establishes command-list memory effects only. It does
not compare terminal scalar/vector/SP state, completion work, live scheduling,
or commit policy and therefore cannot authorize live HLE selection by itself.

Every private file has a required exact SHA-256. The harness refuses a
mismatch before execution. Paths and bytes never enter the report. Standard
output is one canonical compact JSON value containing only fixture revision,
fixed experiment-axis names, verified/captured digests, terminal and
instruction counts, diagnostic DMA records, phase-separated mutation ranges
with digests, same-baseline comparison locations, and selected
whole-image/sentinel digests. It never serializes
microcode, ROM, PCM, RDRAM, DMEM, or IMEM bytes. Caller-controlled case IDs,
packet words, layouts, detailed axis values, paths, and sentinel parameters are
also omitted rather than being mislabeled share-safe.

## Request v2: same-baseline matrices

The following is shape documentation; digest and path placeholders are not a
fixture or an admitted private identity.

```json
{
  "schema": "fn64.audio-abi-characterization-request.v2",
  "fixture_revision": 2,
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
      "trials": [
        {
          "phases": [
            { "packets": [{ "word0": 33554432, "word1": 0 }] }
          ]
        },
        {
          "phases": [
            { "packets": [{ "word0": 33554432, "word1": 8 }] }
          ]
        }
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
`task_count >= 2`, supplies exactly that many phases in every trial, and
carries the prior phase's RDRAM, RSP memory, and complete architectural
scalar/vector state into the next rspboot. Every non-persistence trial must
contain exactly one phase; state carry cannot occur under a label that claims
a single-task experiment.

Every case has at least two trials. Trial zero is the declared control and
each later trial is compared with it. All trials begin from deep copies of one
pre-rspboot baseline: the same exact microcode inputs, zeroed physical RDRAM,
sentinel initialization, empty RSP memory/state, and layout. Packet phases are
installed only after that baseline is cloned. The report's
`common_baseline_sha256` content-binds the omitted baseline, so a comparison
cannot accidentally join independently initialized tasks. Trials must have
equal phase counts and corresponding phases must have equal packet counts, so
the `OSTask` header geometry is identical. For the first phase, the harness
also hashes the complete post-rspboot entry after masking only the declared
packet input range and rejects the case unless every trial matches. A packet
that influences rspboot outside its declared input bytes therefore cannot be
mistaken for a same-snapshot ucode result. This separates two distinct
questions that request v1 conflated: independent control/probe execution and
deliberate cross-task state persistence.

The report binds the omitted inputs with domain-separated SHA-256 values. A
request digest covers the fixed schema/revision, observed input identities,
layout, and ordered case digests. Each case digest covers its hidden ID, typed
axis values, sentinels, and ordered trial digests. Each trial digest covers its
ordered phase digests, and each phase digest covers its ordered packet words.
These are content addresses, not admission signatures;
they make two reports joinable to the same private request without echoing its
caller-controlled strings or words.

The digest wire is host-independent: strings and lists carry an unsigned
64-bit big-endian byte/item count; strings then carry UTF-8 bytes; numeric
fields use their declared unsigned width in big-endian order; enum variants
begin with the fixed ASCII axis name and fixed one-byte subvariant tags.
Nested phase, trial, and case values enter their parent as exactly 32 digest
bytes. The v2 domain strings are versioned in `characterize.rs`; changing any
shape requires a new domain and report schema.

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

## Canonical comparison result

For every trial-zero/candidate pair, corresponding phases are compared in
this stable order: rspboot RDRAM write coverage and bytes; rspboot DMA journal;
rspboot IMEM replacement journal; post-rspboot entry identity; terminal
reason; ucode RDRAM write coverage and bytes; complete DMEM; complete IMEM and
its generation; SP PC; every scalar, vector, accumulator, flag, divider,
SP/DMA/DPC register; the separately retained deferred-DPC submissions;
ucode diagnostic DMA journal; ucode IMEM replacement journal; rspboot work;
ucode work. The first mismatch is emitted as a fixed domain plus an entry
index or logical byte address where one exists. Guest byte values are never
emitted.

Boot write coverage is retained independently from final bytes, so a
same-valued boot DMA cannot disappear from equivalence. Every phase, including
later persistence phases, retains its boot patches, DMA/replacement journals,
and entry digest. Only the first phase is required to have an equal entry
digest: later differences are the persistence evidence and appear at their
exact phase rather than being discarded.

The report also emits every contiguous differing range in the union of both
lanes' boot-phase and ucode-phase RDRAM write coverage, plus complete DMEM and
IMEM differences. Each range carries only its address, length, and whole-range
digest from each lane.
The RDRAM comparison deliberately excludes untouched request storage, so
differing packet words do not become a false result divergence. Exact write
coverage remains observable even when a write restores its entry value.
Complete architectural states have a canonical field-by-field digest whose
wire includes all scalar/VU/SP/DMA/DPC state. The LLE runner drains deferred
DPC work before taking that machine snapshot, so the report retains the
drained list separately and content-binds its ordered source/range/command
identities with another canonical digest. Diagnostic step counters are
compared separately.

Execution failures cross the private-input boundary through exhaustive static
mappings for RSP-memory installation, rspboot, nested entry-snapshot, nested
microcode-identity, and speculative-LLE errors. Variant context—including
addresses, PCs, offsets, counts, digests, XBUS command words, and their
indices—is never formatted into the report or stderr error returned by this
tool.

This machinery can answer the Standard ABI memory-command questions once a
private admitted image and hand-authored control/probe packets are supplied:
SETBUFF versus `A_AUX` disposition; LOAD/SAVE count and address rounding;
CLEAR and DMEMMOVE coverage, wrap, overlap, and zero-count behavior; SEGMENT
translation and task persistence; and LOADADPCM/SETLOOP state persistence.
The tool does not infer those answers from a single digest or promote them
into HLE code.

## Evidence boundary

A report characterizes exactly the supplied request and hashes. It does not
identify a named ABI family, establish expected semantics, certify HLE, or
authorize release fallback. Expected behavior must be established separately
through a predeclared experiment matrix and repeated observations. Private
reports and inputs stay outside git; only hand-authored public fixtures in this
crate are repository content. One public fixture drives a complete synthetic
ucode through `run_trial`, submits an XBUS DPC range, and proves that the
post-drain submission reaches the execution capture, content-free report, and
trial comparison without embedding private input.
