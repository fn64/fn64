# Discovery toolchain adapters

Status: proposed design  
Date: 2026-07-17

This document defines where external reverse-engineering and emulation tools
fit around `fn64-discover`. It complements
[`DISCOVER-DESIGN.md`](DISCOVER-DESIGN.md),
[`DISCOVER-PLAN.md`](DISCOVER-PLAN.md), and
[`DISCOVER-STORAGE.md`](DISCOVER-STORAGE.md). The short answer is:

> Rust owns identity, mappings, proof rules, the canonical graph, and the
> final packs. External tools propose typed claims, consume derived views, or
> reject bad hypotheses. No external tool owns truth.

The first adapters should be spimdisasm, Splat, and m2c because they match the
N64 workflow directly and have short feedback loops. Ghidra is valuable as a
second, structurally different analyzer and interactive UI, but it should
receive fn64's bank model rather than invent one from a flat ROM. Dynamic
tracing should begin with a documented, scriptable debugger interface such as
MAME's and add other emulators as independent black boxes. Expensive general
binary-analysis tools such as angr belong at the unresolved frontier, not in
the default whole-ROM pass.

## Clean-room and license boundary

This design uses project documentation and public command/API contracts. It
does not use any emulator implementation as a behavioral source. In
particular, GPL runtime internals remain prohibited by [`AGENTS.md`](../AGENTS.md).
A reference emulator may be executed and observed as a black box; its source
may not be read to answer an N64 behavior question.

Tool execution and source incorporation are different decisions:

- Ghidra is Apache-2.0 and has documented headless scripting. An fn64-owned
  export script may use that public API.
- spimdisasm, Splat, Rabbitizer, and n64sym are MIT-licensed. They may be
  pinned external dependencies, but Rust remains the canonical engine.
- angr is BSD-2-Clause and is suitable for bounded experiments.
- m2c is GPL-3.0. It may be an optional user-installed subprocess and produce
  a user's out-of-tree decompilation artifact. It must not be linked into,
  vendored into, or copied into fn64.
- N64LoaderWV's repository describes its Ghidra loader but does not present a
  license in the repository metadata reviewed for this design. Do not vendor
  or copy it without a separate license check. It is not required: fn64 can
  materialize each bank and import it through Ghidra's public raw-binary API.
- MAME, ares, CEN64, and any other emulator run out of process. Their exact
  binary, version, settings, and output digest are provenance. fn64 does not
  link an emulator core.
- A GPL emulator or runtime supplied by the user is black-box-only. Do not
  implement an adapter by reading or modifying its internals. Prefer a public
  command, debugger, GDB, or trace interface. If no such boundary exists, the
  tool is not eligible.

ROMs, materialized banks, traces, Ghidra projects, matching assembly, m2c
output, caches, and tool logs are game-derived and stay in a user-selected
out-of-tree workspace. A tool adapter must refuse an output path inside the
fn64 repository unless the output is a synthetic test fixture.

## Adapter roles

Every adapter declares exactly one primary role. A tool may have several
adapters, but one run cannot blur the roles.

| Role | Direction | Permitted result |
|---|---|---|
| Candidate provider | tool -> fn64 | typed candidate claims and diagnostics |
| Consumer | fn64 -> tool | assembly, config, context, or UI project; no facts return automatically |
| Validator | hypothesis -> tool -> fn64 | rejection/conflict on mismatch; success normally does not promote |
| Differential oracle | fn64/tool A <-> tool B | test evidence about a named behavior and scope |
| Runtime observer | emulator/hardware -> fn64 | ordered, scenario-bound observed events |

This distinction prevents circular evidence. For example:

1. fn64 proposes a function extent.
2. Splat emits assembly using that extent.
3. m2c consumes the assembly successfully.

That is one hypothesis surviving two consumers. It is not three independent
boundary claims. Likewise, a Ghidra analysis seeded with an fn64 function
entry cannot corroborate that same entry; the seed is a parent source of all
descendant Ghidra claims.

## Common adapter contract

### Run identity

Every tool run gets a stable `SourceId` computed from canonical fields:

```text
ToolRunSource
  adapter_schema_version
  adapter_id and adapter_algorithm_version
  role
  tool name and exact version
  executable or environment digest
  normalized ROM digest
  input snapshot digest
  selected BankIds and BlobSlice digests
  canonical parameter digest
  parent SourceIds
  output digest
  license disposition
```

Absolute paths, timestamps, process IDs, thread counts, and hostnames are
recorded only in a non-canonical diagnostic log. They never enter `SourceId`.
Python tools additionally record the lockfile or installed-wheel digest, not
only `python --version`. Ghidra runs record the Ghidra release, Java runtime,
script digest, processor-language identifier, analysis-option digest, and any
extension digest.

`parent SourceIds` are mandatory. They identify common lineage and prevent
double voting:

```text
Rabbitizer decode
  -> spimdisasm analysis
     -> Splat split/assembly
        -> m2c pseudocode
```

Claims from this chain share one decode lineage unless a later stage adds an
independent observation. Running Splat and spimdisasm separately over the
same bytes is useful as a reproducibility check, but it is not independent
corroboration when Splat delegates MIPS analysis to spimdisasm.

### Bank input

Adapters never receive an undifferentiated ROM when fn64 already has mappings.
The runner materializes a bank-local input in a disposable workspace:

```text
ToolBankInput
  RomId
  BankId
  bank alias for display only
  blob digest and temporary path
  source ROM/VROM span
  runtime VA start
  candidate/proven executable intervals
  seed entries with state and SourceIds
  known data intervals
  byte order = canonical big-endian
```

Overlays sharing a runtime VA are separate inputs. A tool that cannot model
them simultaneously runs once per bank. Results are translated back through
the exact `ToolBankInput`; a raw address without the input `BankId` is invalid.

The temporary path is not evidence. The blob digest, mapping, and source
snapshot are.

### Canonical output

Adapters emit versioned JSON Lines for inspection and a typed in-memory form
for ingestion. The first row is a header, followed by claims and diagnostics,
then one summary. Representative claim kinds are:

```text
FunctionEntryCandidate(bank, VA, basis)
FunctionExtentCandidate(bank, start, end, basis)
BlockCandidate(bank, start, end, terminator)
ControlEdgeCandidate(bank, site, target, kind)
DataObjectCandidate(bank, start, end, kind)
ReferenceCandidate(bank, site, target, kind)
RegionBoundaryCandidate(bank, VA, left_kind, right_kind)
LibraryIdentityCandidate(bank, start, reference_id)
PrototypeCandidate(bank, entry, canonical_type)
ValidationMismatch(subject, expected_digest, actual_digest)
ObservedPc(trace, sequence, active_bank, PC)
ObservedTransfer(trace, sequence, active_bank, site, target, kind)
ObservedPiDma(trace, sequence, source, destination, length, phase)
ObservedWrite(trace, sequence, active_bank, address, size, value_digest)
```

Every claim carries its `SourceId`, parent claim IDs when applicable, the
tool's typed basis, and any raw score as a diagnostic. Addresses and spans use
the typed spaces from `DISCOVER-STORAGE.md`. A free-form tool symbol is an
alias payload, never identity.

The parser rejects, rather than repairs:

- unknown required schema fields or claim kinds;
- unqualified virtual addresses;
- addresses outside the supplied bank mapping;
- inverted, overflowing, or unaligned instruction spans;
- duplicate rows with incompatible payloads;
- a claimed input or output digest mismatch;
- non-finite scores or locale-dependent numbers;
- tool failure, timeout, truncated output, or missing summary.

Claims are sorted by canonical typed key and deduplicated before ingestion.
Tool output order never affects a snapshot.

The current v1 implementation serializes external results as a separate
`ToolClaimSetV1`, bound to the exact `ProgramSnapshotV1` digest. Its validator
recomputes bank identity, source and claim digests, role/range constraints,
canonical ordering, and provider observations. It never inserts these claims
into `FactDb`; that physical/type boundary prevents a generic native
conclusion from being promoted by tool output. Removing a source removes only
its sidecar claims. The current schema carries function entries/extents,
executable/data ranges, and aliases; the broader graph kinds above remain the
target schema.

### Sandboxing and resource bounds

ROMs are untrusted input to large third-party parsers. Each adapter runs with:

- no network access by default;
- a fresh content-addressed working directory;
- explicit wall-clock, CPU, memory, file-count, and output-byte limits;
- stdout and stderr captured separately;
- no inherited credentials beyond the minimum environment;
- a process-group kill on timeout;
- output parsing only after a successful exit.

A resource limit produces an `open` frontier item. It never produces a
partial accepted result unless the tool's schema has an authenticated,
complete checkpoint and the adapter explicitly supports it.

## Evidence, independence, and promotion

External tools do not receive a special confidence shortcut. Their scores are
kept for ranking and evaluation, but conclusions use the existing discrete
states.

| External result | Maximum state before fn64 proof rules | Reason |
|---|---|---|
| One tool's function start or extent | `candidate` | heuristic partition |
| Agreement from independent analyzer families | `candidate` in the current sidecar | corroboration is not boundary proof; a future native rule must consume it explicitly |
| Splat plus spimdisasm agreement | `candidate` | shared implementation lineage |
| m2c accepts an assembly function | no promotion | consumer success does not prove the split |
| Ghidra decompiles a region | no promotion | many incorrect partitions still decompile |
| Matching assembly/relink equality | no promotion | validates bytes/placement, not ownership |
| Round-trip mismatch | `rejected` or `conflict` | concrete contradiction |
| Unique full-body library signature | `supported` identity only | extent and callability remain separate |
| Emulator executes a bank-qualified PC | proven observation | existence in that named run, not universal reachability |
| Emulator observes an indirect target | proven observation | target occurred; target set is not exhaustive |
| Completed, resolved PI DMA event | proven observation and mapping geometry | does not imply executable content |
| Screenshot/audio match | runtime differential only | no static ownership claim follows |

Two providers are independent only if all of these hold:

- they do not share a decoder/analysis backend for the relevant claim;
- neither consumed the other's output or the claim being corroborated;
- they did not receive the same candidate as a seed;
- their signature/reference corpora have distinct source identities;
- wrapper names do not hide a shared emulator core;
- the derivation graph has no common non-ROM parent for the claim.

Agreement between fn64's MIPS decoder and Ghidra's SLEIGH decoder is a useful
cross-implementation differential. It still does not make a heuristic
function boundary proven. Agreement between three frontends using Rabbitizer
is one decoder-family result.

An external candidate may eventually become `proven` only when a named fn64
rule consumes it with sufficient non-heuristic evidence. Examples include a
candidate call target reached by a direct call from proven code, or a
candidate table whose bounded consumers establish every record field and
whose runtime DMA events validate its mapping. The proof cites both the tool
claim and the fn64 derivation; it is never labeled "tool says so".

Signature corpora require their own manifest:

```text
SignatureCorpus
  corpus digest and schema
  source/license citation per object family
  compiler/library variant metadata
  transformation algorithm and version
  whether bytes may be persisted
```

Do not accept n64sym built-ins, Ghidra FID databases, or a loose directory of
objects as authoritative without this provenance. A signature name may be
wrong even when bytes match a related library build. Full-body uniqueness,
relocation compatibility, bank compatibility, and a runner-up margin are
required before `supported` identity.

## Static tools

### Rabbitizer: decoder differential, not a second discovery engine

[Rabbitizer](https://github.com/Decompollaborate/rabbitizer) provides
allocation-free per-word MIPS decoding, instruction validation, VR4300
coprocessor names, RSP decoding, and Rust bindings. Its highest-value initial
use is a test adapter:

- decode every synthetic MIPS-III and delay-slot fixture with fn64 and
  Rabbitizer;
- compare opcode class, registers, immediates, branch/jump target, and
  control-flow category;
- retain disagreements as decoder test cases;
- sample corpus words by opcode and region class, then expand to full proven
  executable intervals when cheap.

Rabbitizer does not infer banks, function ownership, or loader semantics. A
Rabbitizer/spimdisasm/Splat agreement shares lineage and gets one vote.
Keeping fn64's small decoder is still useful: proof behavior remains visible
and type-checked in Rust, while Rabbitizer catches ISA coverage gaps.

### spimdisasm: first candidate provider and assembly emitter

[spimdisasm](https://github.com/Decompollaborate/spimdisasm) is the most
immediately useful external analyzer. Its documented features include MIPS
I-IV decoding, matching assembly, automatic function/pointer/symbol
detection, HI/LO pairing, handwritten-function detection, section and simple
file-boundary detection, and experimental same-VRAM overlays.

The adapter should not parse human-oriented assembly text to rediscover its
analysis. Pin a compatible release and maintain a small adapter-owned Python
exporter that calls its public package and writes fn64's JSONL schema. Because
the backend documentation is currently sparse, pin the exact wheel hash and
cover every consumed field with a synthetic conformance test. A changed
spimdisasm version is a new adapter algorithm input.

Run it once per fn64 bank with the bank's VA and known executable/data spans.
Import:

- proposed function entries and extents;
- block starts and direct references;
- HI/LO symbol/reference pairs;
- pointer, string, float, and data-object candidates;
- text/rodata/file boundary candidates;
- tool diagnostics about invalid or overlapping analysis.

Do not let spimdisasm's experimental overlay identity replace `BankId`. Do
not import automatically generated names as identities. Matching assembly is
a consumer/validator output, separate from candidate detection.

The first measurement is marginal value over each native provider:

```text
new exact entries found only by spimdisasm
new false entries found only by spimdisasm
extent exact/coarse/wrong counts
new HI/LO and data references
new executable bytes reached after native proof
time, peak memory, and cache bytes per bank
```

The first local measurement (2026-07-17) pinned spimdisasm 1.42.2 and
Rabbitizer 1.16.2, disabled built-in symbol corpora, and analyzed the unseeded
NWXE resident text interval supplied by the external evidence manifest. It
proposed 899 entries against 847 known: 827 exact starts, 72 false starts, and
20 misses (91.9911% precision, 97.6387% recall). Among the 827 common starts,
666 extents were exact, 153 too long, and eight too short. The result supports
T1 immediately for entry/reference candidates while confirming that extent
claims must remain candidates until ownership rules close them.

The first production normalization slice is implemented in
`spimdisasm_adapter`: it consumes a completed `--function-info` CSV without
launching a process, requires the pinned tool identity and exact bank input,
checks each row's VROM/VA geometry and non-overlapping word-aligned extent,
and exports function entries/extents through the strict tool-adapter JSONL.
The canonical provider-output digest ignores row order and CSV quoting while
retaining every decoded field; absolute, drive-qualified, URI-like, backslash,
and traversing `file` values are rejected so disposable workspace paths cannot
enter that digest. Configuration and output digests are explicit lineage.
Generated names are deliberately not imported as identities. The out-of-
process runner and block/reference/region export remain T1 work.

### Splat: pack consumer and partition experiment

[Splat](https://github.com/ethteck/splat) is a binary splitting tool for N64
and other MIPS platforms. It is the natural Decomp Pack consumer:

- fn64 emits sections, mappings, symbols, and candidate/proven boundaries;
- Splat creates the conventional assembly/data/linker workspace;
- spimdisasm emits matching assembly beneath Splat;
- assembly/relink results return through a validator adapter.

Splat configuration is an input declaration, not evidence. If fn64 writes a
function address into the config and Splat reproduces it, no new claim was
created. Splat-specific file/section boundary heuristics may return as
candidates, but their source lineage includes spimdisasm where applicable.

Emit two visibly separate configurations:

- `proven`: only authoritative bank/owner facts; failure is a gate;
- `experimental`: candidate partitions for measurement; never merge its
  generated symbols into the proven pack.

Splat and its Python environment are optional toolchain dependencies. The
core discovery artifact must remain usable without them.

### m2c: downstream pseudocode and type-hint producer

[m2c](https://github.com/matt-kempster/m2c) consumes GNU `as`-style assembly,
including spimdisasm output, and can use a context file for declarations and
cross-function type inference. This makes it useful after an owner view
exists, not for finding that view.

The adapter supplies:

- one exact or explicitly experimental function/owner assembly unit;
- bank-qualified symbol aliases rewritten to valid C identifiers;
- known prototypes, structs, globals, and section information in a generated
  context;
- the target/compiler mode as an explicit parameter.

Returned pseudocode, prototypes, struct fields, stack variables, and inferred
global types are decompilation hints. They never establish code/data class,
entry, or extent. A parse failure is a diagnostic about the current pack, not
proof that the ROM is malformed.

m2c documents an internal context cache. fn64 must still key the enclosing
adapter result by m2c version, assembly digest, full context digest, and
parameters. The tool's cache is acceleration, never evidence. m2c remains an
external GPL subprocess; its source or algorithms are not copied into fn64.

### Ghidra: independent graph/type candidates and human UI

[Ghidra](https://github.com/NationalSecurityAgency/ghidra) supports automated
analysis and scripting. Its official
[`HeadlessAnalyzer`](https://ghidra.re/ghidra_docs/api/ghidra/app/util/headless/HeadlessAnalyzer.html)
runs pre-scripts, analysis, and post-scripts; the API also states that one
analyzer instance is not thread-safe. Use one isolated process/project per
bank, parallelized at the process level.

Do not make a flat N64 ROM loader authoritative. The public
[N64LoaderWV](https://github.com/zeroKilo/N64LoaderWV) description says it
normalizes byte order and loads ROM, RAM, and boot regions, which is useful
for interactive first contact but insufficient for arbitrary mutually
exclusive overlays. fn64 has already normalized bytes and established bank
identity. The production adapter should therefore:

1. create a disposable Ghidra project;
2. import one materialized bank as raw big-endian MIPS using a pinned,
   validated processor-language identifier;
3. create memory blocks at fn64-supplied VAs and permissions;
4. seed only facts selected by the experiment, retaining their SourceIds;
5. run a fixed analyzer option set and timeout;
6. use an fn64-owned post-script to export canonical JSONL;
7. delete or cache the project outside the repository.

Export candidates for functions, blocks, flows, switches, references,
strings, data objects, prototypes, stack frames, and decompiler-derived types.
Ghidra function starts and decompiler output remain candidates. A Ghidra
control-flow disagreement with fn64 is a high-priority differential because
the decoder/lifter family is independent.

Use two experiment modes:

- `unseeded`: only bank mapping, permissions, and a hardware/proven entry;
  useful for genuine independent discovery;
- `seeded`: all current entries and data ranges; useful for types, xrefs, and
  human work, but not corroboration of the seeds.

The first bounded T3 conformance spike now runs stock Ghidra 12.1.2 over two
handwritten 64-byte MIPS fixtures at the same VA. Each bank uses a separate
disposable raw-binary project with
`MIPS:BE:64:64-32addr:o32`; the installed N64 loader is not selected. Unseeded
analysis found each bank's distinct direct-call target, seeded analysis added
only its declared entry with discovery-snapshot lineage, and every result
passed the Rust candidate-only adapter. Ten consecutive three-project runs
produced one output digest per mode/bank. The reproducible command and full
digests live in [`tools/ghidra/README.md`](../tools/ghidra/README.md).

Ghidra's GUI is an excellent review surface. Export fn64 proof state and
provenance as bookmarks/comments/colors, and import analyst changes only as a
separate external evidence manifest. A click in a GUI is not silently written
back as proof.

### n64sym: explicitly sourced library identity candidates

[n64sym](https://github.com/shygoo/n64sym) scans a ROM or RAM dump against
signature, object, or library inputs. Its own documentation warns that direct
ROM scanning may be inaccurate and recommends a RAM dump. fn64 can provide a
better input: one exactly materialized, mapped bank.

Use `n64sig` only with locally supplied objects whose source, license, build
variant, and digest are declared in a `SignatureCorpus`. Do not rely on a
flat header displacement to infer overlay mapping. Run thorough byte scanning
only over executable candidates and validate every match in Rust with:

- exact or relocation-masked full-body comparison;
- independently established bank and extent compatibility;
- uniqueness within the corpus and target bank;
- a recorded second-best match;
- call/reference neighborhood compatibility where available.

The result is a supported library-identity alias, not proof of a function
boundary. Porting the useful normalized fingerprint index into Rust is likely
faster for corpus-scale operation; n64sym remains a differential and corpus
import/export tool.

### angr: expensive local frontier solver

[angr](https://github.com/angr/angr) provides lifting, CFG recovery, symbolic
execution, value-set/data-dependency analysis, and decompilation. Its CFG
documentation distinguishes fast static recovery from emulation-based
recovery. This overlaps fn64's Rust graph and should not be in the default
whole-ROM loop.

Use angr only for bounded experiments selected by expected information gain:

- an open indirect call with a small predecessor slice;
- a suspected callback table mutation;
- one local ownership ambiguity region;
- a loader call whose source/destination/length slice is unresolved.

Provide a bank-local blob, explicit architecture/base address, initial roots,
and modeled external calls. Impose hard state, path, memory, and time limits.
Export target sets, path predicates, data dependencies, and blocks as
candidates. A timeout or path explosion stays `open`. Never accept a symbolic
target merely because it is satisfiable; it must map, align, decode, and pass
the same bounded/exhaustive rules as the Rust resolver.

angr earns a permanent adapter only if it closes materially more frontier per
CPU-minute than extending the Rust value-set engine.

### Recompiler, assembler, and compiler diagnostics

[N64Recomp](https://github.com/N64Recomp/N64Recomp) documents its current
model as a binary plus function symbol/metadata list, with each input
function emitted separately. fn64's existing C and Rust emitters are therefore
the most important consumers of proven owner views.

Assembler, linker, compiler, N64Recomp, fn64-recomp, asm-differ, and
decomp-permuter results are validators or downstream work products:

- a byte/relocation mismatch rejects a hypothesis;
- an overlap, missing target, or codegen trap adds a typed frontier item;
- byte-identical assembly validates decoding, relocation reconstruction, and
  placement;
- successful compilation validates syntax and declared interfaces;
- none of these successes proves that a source-like function boundary is
  historically or semantically correct.

Compiler diagnostics should be parsed into stable categories and locations,
not retained only as free-form logs. Automatic source patches based on a
diagnostic are outside discovery and may not alter proof state.

## Dynamic observers

### Trace contract

Dynamic tools consume an fn64-generated `ProbePlan` and emit a versioned,
ordered trace. The minimum trace header is:

```text
TraceHeader
  schema and producer versions
  RomId
  emulator/core executable digest
  settings and plugin digests
  scenario ID and input-timeline digest
  initial save-state digest, if any
  reset type and region
  event/instruction/emulated-time limits
```

Events have a monotonically increasing sequence number and emulated time.
Required kinds are executed PC/block, taken transfer, PI DMA start/completion,
bank activation/replacement, watched memory write, exception/interrupt entry,
frame digest, and audio-block digest. Large instruction traces may be stored
as a compressed content-addressed stream plus a canonical decompressed digest;
the ingested claims remain sorted typed rows.

Bank attribution is mandatory for semantic use. Resolve overlapping VAs by
the ordered load/activation history and, when necessary, the digest of bytes
resident at the executed PC. If several banks remain possible, store an
unqualified raw-VA observation and a conflict set. Never choose the most
recent or first-listed bank silently.

Dynamic facts are scoped carefully:

- `ObservedPc` proves execution in that run, not all possible runs.
- `ObservedIndirectTarget` proves one target, never exhaustiveness.
- absence from a trace proves nothing unless the probe itself establishes a
  bounded exhaustive universe.
- a PI transfer becomes completed only on a completion/interrupt event, not
  when source/destination/length registers are merely written.
- frame and audio digests validate runtime behavior, not static ownership.

### MAME first

MAME's public debugger documentation exposes instruction tracing, PC history,
memory-write tracking, watchpoints, screenshots, trace logging, and command
files through `source`. Its command-line documentation also provides
`-debugscript` for executing a debugger command file at startup and a watchdog
timeout. That is enough to prototype an unattended out-of-process adapter
without reading emulator code:

- generate a debugger command file from a `ProbePlan`;
- trace only named PC/range predicates rather than the entire boot when
  possible;
- watch public N64 PI registers and suspected table ranges;
- log sequence, PC, relevant registers, and memory values in a parseable
  prefix format;
- stop at an event/time budget and capture a screenshot;
- normalize the textual log into the trace schema.

Whether a particular pinned MAME build can do this with no display server is
a conformance result, not an assumption. An adapter may be unattended before
it is fully displayless. Frame-capture scenarios necessarily retain a video
output path.

Machine/device tags and debugger syntax are adapter-version parameters. They
do not enter generic discovery logic. Validate them with a synthetic N64 test
ROM before using a release.

The emulator-neutral half of this boundary now exists in
`fn64_discover::headless`. It exports a canonical run bundle containing the
validated probe plan, ROM and input-timeline identities, all three budgets,
the adapter/emulator identity, executable/settings digests, typed region and
reset kind, and a mandatory state digest for state-restore starts. A black-box
wrapper returns a strictly sequenced JSONL stream whose header must match the
run-bundle digest. Every observation names the probe that admitted it; Rust
checks its range, bank, direction, width, and budget before translating it to
the canonical trace schema. The only accepted PI record is explicitly a
completed DMA, so debugger-visible register writes cannot silently become a
transfer observation.

This bridge deliberately emits no exhaustiveness claim yet. Trace schema v1's
coverage domains do not retain a probe's range and bank filters, so translating
"all events in this filtered range" into a whole-domain claim would broaden
the evidence. The exact next external dependency is a pinned MAME binary with
the Nintendo 64 machine and public debugger command interface enabled. Its
machine/device tags, command syntax, reset behavior, and PI completion signal
must be established against a synthetic N64 test ROM; that result becomes a
versioned MAME wrapper which emits the neutral JSONL, not generic discovery
logic.

### ares and CEN64 as independent black boxes

[ares](https://github.com/ares-emulator/ares) documents N64 support,
command-line launch, and debugger trace facilities; its public site describes
trace logging for original-software development. A GDB or debugger adapter
can provide PCs, breakpoints, and memory reads when the pinned build exposes
them. [CEN64](https://github.com/n64dev/cen64) is BSD-licensed, targets
hardware-level accuracy, and includes public command-line usage; its debugger
and GDB automation need a conformance spike before adoption.

Use these initially for independent execution coverage, checkpoint memory,
frame/audio capture, and disagreement localization. Do not assume two emulator
frontends are independent without recording the core digest. Cross-emulator
agreement supports confidence in an observation; disagreement becomes an
explicit runtime frontier, not a majority vote.

### Other reference runtimes

A user-supplied reference runtime may participate only through a documented
black-box interface. For a GPL runtime, fn64 must not inspect or modify its
implementation and must not link its core. A console process, public debugger
protocol, or externally produced trace can be ingested if it satisfies the
same schema and provenance rules. If the only path requires reading runtime
internals or adding private instrumentation, stop at that frontier.

Hardware traces are the strongest independent runtime observation when
available. They use the same schema with a hardware/flashcart/logic-trace
producer ID and explicit limitations.

## Staged adoption

Each stage is useful alone and preserves the ROM-only Rust path.

### T0: adapter protocol and fake tools

- [x] Implement typed tool/bank identity, canonical JSONL, strict parsing,
  lineage, and resource limits.
- [x] Add synthetic fake-provider coverage for malformed, partial, stale, and
  conflicting results.
- [x] Add an overlay fixture with two banks at the same VA.
- [x] Keep the interchange candidate-only at the type boundary, so shared
  Splat/spimdisasm lineage cannot create an authoritative conclusion.
- [x] Freeze accepted runs into a snapshot-bound canonical sidecar, validate
  it after deserialization, and prove source removal cannot mutate native
  facts.

Exit gate: malformed, stale, unqualified, circular, and partial results fail
loudly; canonical output is identical across ten runs.

### T1: decoder differential and spimdisasm provider

- Add Rabbitizer as an optional test oracle.
- [x] Add the pinned spimdisasm function-info JSON normalizer.
- [x] Run the function-info adapter through the sidecar for entries and
  extents; references and region candidates remain open.
- Measure marginal precision/recall, exact/coarse owner bytes, new references,
  and cost per bank.

Exit gate: no external claim is authoritative by itself; disabling the
adapter removes only its claims and descendant derivations; a holdout ROM
shows positive marginal entry or reference value.

### T2: Splat and m2c consumers

- Emit proven and experimental Splat packs separately.
- Run matching-assembly/relink validators.
- Generate m2c assembly/context for exact owners and selected experimental
  coarse owners.
- Parse validation failures and m2c diagnostics into frontier categories.

Exit gate: proven pack round trips where expected; an experimental pack can
never leak symbols or boundaries into the proven pack without a new native
proof derivation.

### T3: Ghidra headless provider and review project

- [x] Prove per-bank raw import and candidate export on synthetic same-VA
  banks using official Ghidra APIs.
- [x] Run unseeded and seeded modes with distinct configuration and lineage.
- Differentially compare blocks, direct edges, switch candidates, and data
  references.
- Export fn64 states/provenance for interactive review.

Exit gate: projects with different import order or process parallelism yield
the same canonical claims; same-VA overlays never merge; tool disagreement is
reported at exact words/edges.

### T4: signature and corpus providers

- Define `SignatureCorpus` and require source/license/digest metadata.
- Add n64sym as a differential provider over materialized banks.
- Feed exact and relocation-masked matching into the Rust corpus index from
  `DISCOVER-STORAGE.md`.
- Evaluate cross-ROM transfer with leave-one-ROM-out tests.

Exit gate: no non-unique or prefix-only match is promoted; runner-up distance,
full-body validation, and bank compatibility are reported.

### T5: targeted dynamic probes

- Define `ProbePlan`, the trace schema, and the digest-bound neutral headless
  run/observation bridge. (Implemented.)
- Implement MAME debugger command generation and ingestion first.
- Add ares/CEN64 only after a small conformance suite proves stable PCs,
  memory reads, reset behavior, and event ordering.
- Schedule open indirects, loader/DMA sites, code-data conflicts, and table
  writes by expected information gain.

Exit gate: trace ingestion is byte-deterministic; every event is scenario- and
producer-bound; observed targets remain non-exhaustive; active-bank ambiguity
never resolves by guess.

### T6: expensive local solvers

- Add angr only for bounded frontier slices.
- Compare its marginal closures and cost against extending Rust analysis.
- Keep a per-query state/path/time budget in the cache key.

Exit gate: no timeout or partial solver result becomes a target; the adapter
demonstrates useful frontier closures on a holdout set.

## Deterministic evaluation gates

Every adapter reports both correctness and cost. Entry precision/recall alone
is insufficient.

### Conformance

- exact tool/environment and adapter digests recorded;
- strict schema round trip and unknown-field behavior tested;
- big-endian word, signed immediate, pseudo-direct jump, and delay-slot
  fixtures agree with expected facts;
- two same-VA banks remain separate through export and import;
- tool crash, timeout, malformed row, range overflow, and truncated summary
  each produce a named rejection/open result;
- one-thread and many-process outputs canonicalize identically;
- ten clean static runs produce one claim-artifact digest;
- concurrency-sensitive runner/cache tests pass twenty consecutive runs.

### Discovery value

Report per tool, bank, and corpus:

- candidate entry precision and recall;
- marginal true/false entries not already emitted by another lineage;
- exact, coarse, ambiguous, wrong, and open owner counts;
- exact-owner, coarse-owner, reachable-unowned, and candidate executable
  bytes;
- new direct references, HI/LO pairs, tables, and data objects;
- exhaustive, bounded, observed-only, and open indirect sites;
- physical ROM classification, logical image coverage, executable coverage,
  and recompiler-accepted coverage;
- authoritative conclusions changed, with full derivation IDs.

Train/tune and holdout ROMs are fixed before an experiment. AKI-family
similarity is useful for homology but can hide overfitting, so at least one
non-AKI holdout remains in every release gate.

### Cost and iteration speed

Report cold and warm:

- wall and CPU time;
- peak resident memory;
- bytes materialized, read, written, and cached;
- claims and genuinely new accepted facts per CPU-second;
- invalidated downstream artifact count;
- per-bank parallel scaling;
- time from one changed candidate/evidence item to updated grade and pack.

The scheduler runs cheap native passes first, then spimdisasm, then Ghidra,
dynamic probes, and local symbolic analysis. A tool is selected for a frontier
item by measured expected information gain divided by cost. Whole-ROM Ghidra
or angr runs are experiments until measurements show they beat targeted bank
analysis.

### Runtime validation

For a named scenario, retain:

- producer/core/settings and input-timeline digests;
- executed block and indirect-target deltas;
- ordered load/activation differences;
- first divergent checkpoint or frame;
- framebuffer and audio-block digests;
- whether the comparison lies within the documented valid horizon of the
  chosen differential.

Repeated absence is never reported as proof of unreachability. Emulator runs
that are nondeterministic remain separate trace sources; ingestion of each
source is deterministic and their intersection/union is labeled explicitly.

## Proposed commands

These names are design targets, not implemented CLI promises:

```text
fn64-discover tool export-bank SNAPSHOT BANK --out WORKSPACE
fn64-discover tool run spimdisasm SNAPSHOT --banks frontier --out RUN
fn64-discover tool run ghidra SNAPSHOT --mode unseeded --banks frontier --out RUN
fn64-discover tool ingest SNAPSHOT RUN --out SNAPSHOT
fn64-discover pack splat SNAPSHOT --state proven --out WORKSPACE
fn64-discover pack m2c SNAPSHOT FUNCTION --out WORKSPACE
fn64-discover probe plan SNAPSHOT --budget 100 --out PLAN
fn64-discover probe run mame PLAN --scenario boot --out TRACE
fn64-discover trace ingest SNAPSHOT TRACE --out SNAPSHOT
fn64-discover tool grade BEFORE AFTER ANSWER_KEY
```

`--banks frontier` means banks/regions selected from typed unresolved indexes,
not a text search. Every command prints the root artifact digest and a compact
metric delta so a change can be evaluated without opening a large report.

## Immediate experiments

1. Define the adapter source/lineage and JSONL schemas with synthetic fake
   tools. This prevents every real adapter from inventing incompatible
   provenance.
2. Pin spimdisasm and measure its unseeded per-bank function/reference output
   against the existing OoT/NW4E/NWXE grades. Keep Splat in the same lineage.
3. Emit one proven and one experimental Splat bank, assemble both, and record
   byte/relocation mismatches without promoting either partition.
4. Run m2c only on exact owners plus a small sampled set of deliberately
   missplit synthetic functions. Catalog which diagnostics are useful frontier
   signals and which accept wrong boundaries.
5. Build a minimal Ghidra headless raw-bank exporter and compare direct edges,
   switches, and starts on synthetic delay-slot/overlay fixtures before a ROM.
6. Prototype a MAME command-file trace for PCs and public PI register writes;
   require a completion event before emitting a completed DMA observation.
7. Add a bounded angr experiment for the hardest remaining indirect site only
   after the native and dynamic frontier reports can name that site precisely.

The adoption criterion is not the number of tools connected. It is a larger
deterministic, bank-correct, provenance-carrying program graph per unit of
iteration time.

## Public tool references

- [Ghidra project and license](https://github.com/NationalSecurityAgency/ghidra)
- [Ghidra HeadlessAnalyzer API](https://ghidra.re/ghidra_docs/api/ghidra/app/util/headless/HeadlessAnalyzer.html)
- [N64LoaderWV project description](https://github.com/zeroKilo/N64LoaderWV)
- [Rabbitizer features and Rust bindings](https://github.com/Decompollaborate/rabbitizer)
- [spimdisasm features and interfaces](https://github.com/Decompollaborate/spimdisasm)
- [Splat project](https://github.com/ethteck/splat)
- [m2c interface and context cache](https://github.com/matt-kempster/m2c)
- [n64sym interface and ROM/RAM caveat](https://github.com/shygoo/n64sym)
- [angr analyses](https://github.com/angr/angr)
- [angr CFG recovery documentation](https://docs.angr.io/en/latest/analyses/cfg.html)
- [N64Recomp input model](https://github.com/N64Recomp/N64Recomp)
- [MAME debugger](https://docs.mamedev.org/debugger/index.html)
- [MAME tracing and command files](https://docs.mamedev.org/debugger/general.html)
- [MAME `-debugscript` and watchdog options](https://docs.mamedev.org/commandline/commandline-all.html)
- [ares project and command-line interface](https://github.com/ares-emulator/ares)
- [CEN64 project and command-line interface](https://github.com/n64dev/cen64)
