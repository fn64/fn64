# Discovery storage and graph architecture

Status: proposed design  
Date: 2026-07-17

This document specifies the data model underneath
[`fn64-discover`](../crates/fn64-discover). It complements
[`DISCOVER-DESIGN.md`](DISCOVER-DESIGN.md), which defines what counts as
evidence, and [`DISCOVER-PLAN.md`](DISCOVER-PLAN.md), which orders the analysis
work. The goal here is narrower: make every discovery pass fast,
deterministic, incrementally reusable, and useful to more than one downstream
tool.

The central decision is:

> The canonical program representation is a bank-qualified basic-block and
> reference graph. Functions, contiguous compile regions, Splat sections, and
> N64Recomp-style address/size rows are derived views over that graph, not its
> storage unit.

This does not remove the need to recover functions for function-oriented
decompilation. It removes the requirement that one uncertain historical
function boundary block all other analysis and all possible recompilation
strategies.

## Provenance and clean-room boundary

This design is based on fn64's current implementation and its own design
documents, the MIT N64Recomp input model, and public N64/libultra hardware
documentation. In particular, load-image edges model the source, destination,
and size exposed by the public
[`osPiStartDma` contract](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/n64man/os/osPiStartDma.htm)
and the public Programming Manual's
[overlay procedure](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro10/10-03.html).
No GPL runtime implementation is a source for this document.

## Current representation: what to preserve and what to replace

The current implementation has several good invariants that remain:

- Addresses are bank-qualified where identity matters.
- Facts are immutable and conclusions have discrete proof states.
- Provider output is sorted before merge, so worker scheduling does not
  affect serialized output.
- CFG construction understands delay slots, branch-likely annulment, direct
  calls, tails, and unresolved indirect sites.
- Load images and executable intervals are separate facts.
- The 4 KiB `LoadImageIndex` avoids an all-image scan for every call target.
- Bounded value-set analysis fails open when a target set becomes too large.

The present shapes are appropriate for an initial implementation but not for
corpus-scale iteration:

- `FactDb` is a `Vec<Fact>` plus a `BTreeMap<String, Conclusion>`. Most typed
  queries rescan the heterogeneous fact vector, duplicate bank/subject
  strings, and reconstruct subject strings. A fact index is stable only
  inside one insertion order, not across independently cached or merged
  artifacts.
- Replacing a conclusion in the map retains its supporting fact indices but
  not the sequence of earlier conclusion decisions. The immutable evidence is
  preserved; the derivation history is not itself a first-class table.
- `harvest::LoadImage` copies materialized bytes, and executable subranges copy
  them again. Multiple logical overlays can also name identical physical
  bytes.
- The page-to-image postings index duplicates an image ID once per covered
  page and indexes raw VA globally. It works for current inputs but is less
  direct than a bank-local interval lookup and does not itself express which
  overlapping bank is active.
- `Cfg` stores pointer-heavy `BTreeMap`/`BTreeSet` structures while building,
  and fixed-point closure rebuilds the graph when roots or indirect edges
  grow.
- `partition` performs a graph traversal for every root and linearly searches
  `cfg.proven_roots` on tail edges. Its worst-case cost is
  `O(R * (V + E))`, where `R` is callable roots.
- Coverage and provenance queries repeatedly scan all facts and re-union
  intervals.
- Serialized Rust enums have no artifact header, schema version, table
  directory, or migration boundary.

These are performance and evolution constraints, not reasons to discard the
current proof rules. Migration should preserve results byte-for-byte before
changing algorithms.

## Two identities: stable on disk, dense in memory

Persisted identities must survive parallel merge, cache reuse, and insertion
of unrelated records. Dense integers are still the right representation for
hot graph traversal. Use both explicitly:

```rust
struct Digest256([u8; 32]);

struct RomId(Digest256);       // SHA-256 of canonical big-endian ROM
struct ImageId(Digest256);     // content-derived load-image identity
struct BankId(Digest256);      // image + activation identity
struct ClaimId(Digest256);     // canonical atomic claim + provenance source
struct SourceId(Digest256);    // manifest, trace, detector run, or tool output
struct BlockId(Digest256);     // bank + start VA + byte/terminator identity

struct BankIx(u32);            // snapshot-local dense index
struct BlockIx(u32);
struct ClaimIx(u32);
struct RootIx(u32);
struct DataIx(u32);
```

Full digests are persisted and collision-checked. A frozen snapshot sorts
stable IDs lexicographically and assigns dense `*Ix` values. All hot tables
and CSR edges use the dense indices. An index is never serialized without the
stable-ID table that defines its namespace.

`ImageId` hashes the normalized ROM identity, source address space and span,
destination span, transform kind, and the stable ID of the descriptor or
loader event that distinguishes otherwise identical loads. `BankId` adds the
activation discriminator. If new evidence materially changes a mapping, it
creates a new identity and invalidates dependent artifacts instead of
silently changing the meaning of an old ID.

Names such as `boot` or `overlay_03` are aliases in an intern table. They are
never identity. This allows a descriptor-generated name and a later semantic
name to refer to the same bank without rewriting graph keys.

### Typed address spaces

Do not carry address-space tags beside untyped `u32` values in analysis APIs.
Make invalid arithmetic unrepresentable:

```rust
struct PhysRom;
struct Vrom;
struct Va;

struct Addr<S> {
    raw: u32,
    _space: PhantomData<S>,
}

struct Span<S> {
    start: Addr<S>,
    len: NonZeroU32,
}

struct BankVa {
    bank: BankId,
    va: Addr<Va>,
}
```

All span ends are computed with checked `u64` arithmetic, then narrowed only
after validation. KSEG aliases, physical RDRAM, VROM, and physical cartridge
offsets are not collapsed merely because a numeric conversion is familiar;
an explicit typed mapping edge records every allowed conversion.

## Load images and interval indexes

A load image is not one flat tuple. It is a node connected to address-space
spans by mapping segments:

```text
physical ROM span --[raw or compressed file]--> VROM/materialized blob span
VROM or physical span --[DMA/decompress]------> bank-qualified VA span
bank-qualified VA span --[permission]---------> text/data/BSS/unknown regions
```

`MappingSegment` contains source span, destination span, transform
(`AffineCopy`, `Yaz0`, `ZeroFill`, or `UnknownTransform`), activation evidence,
and claim IDs. Compression is not represented as a fake affine byte mapping.
The materialized decompressed blob is a content-addressed object, and its
origin edge preserves the compressed source.

Use three index forms:

1. **Per-bank VA index.** Mapping and permission intervals within one bank
   must be non-overlapping after conflicts are removed. Store a sorted flat
   vector and find a point or first overlap by binary search:
   `O(log n + k)` time and compact memory.
2. **Physical ROM and VROM indexes.** Different files, aliases, and load
   images may overlap. Freeze intervals into a deterministic augmented
   interval tree stored in arrays, not heap nodes. A point/range query is
   `O(log n + k)` and returns every mapping, never an arbitrary winner.
3. **Unqualified VA index.** This is diagnostic only: a raw runtime VA can
   match several mutually exclusive overlays. Use the same augmented interval
   representation and return `(BankIx, MappingIx)` postings. Semantic queries
   require an active-bank set or an explicit `BankIx`.

For fewer than 32 intervals, a sorted-vector scan is normally cheaper than
building a tree; the frozen index may select this representation by count.
That selection is deterministic and recorded in the artifact, not based on a
runtime timing heuristic.

Bytes are held once in a `BlobStore` keyed by digest. An image references
`BlobSlice { blob, start, len }`; executable subranges reference a smaller
slice. Uncompressed physical ROM normally uses the normalized ROM blob
directly. This removes the current copies in `load_images` and makes identical
overlay backing naturally share storage.

Activation is separate from address lookup. Store an `ActivationGraph` whose
nodes are banks and whose typed edges mean `MayCoexist`, `MutuallyExclusive`,
`Loads`, or `ReplacesAtVa`. Static loader evidence and named dynamic traces
justify those edges. Unknown activation remains unknown; raw VA lookup does
not guess a bank.

## Frozen program snapshot

Analysis passes append claims to mutable, pass-local builders. A deterministic
merge freezes them into a `ProgramSnapshot` with sorted, column-oriented
tables:

### Sealed cold-training workspace

The schema-v4 snapshot workspace is the current disk boundary for training a
general discovery mechanism against known ROMs. It is produced from ROM bytes
without a label file and declares
`intended_use = sealed_cold_function_training_input` and
`answer_key_present = false`. The fixed namespace contains a manifest, one
`cold-candidates.json` receipt, and indexed bank-byte/snapshot artifacts; the
manifest is published last. It lives in a caller-owned mode-0700 workspace
outside Git, with regular mode-0600 artifacts, and contains no path-derived
bank names.

Candidate receipt v3 retains the addressed identity
`(RomAddressSpace, ROM address, VA)` within its bank-qualified receipt. The
address-space component is semantic: equal numeric physical-ROM and VROM
coordinates are distinct identities. V3 additionally fixes a seven-detector
denominator containing typed semantic-callable arguments derived by composed
authority closure. A v1 or v2 digest cannot authorize v3 grading.

`validate_snapshot_workspace_streaming` is the only training intake boundary.
It visits the bounded candidate receipt and each potentially large bank
artifact once, retaining only the compact caller-selected index, and returns a
workspace identity only after validating the complete namespace. Validation
binds schema and fixed limits, canonical path and permissions, artifact sizes
and hashes, semantic snapshot digests, bank geometry and bytes, normalized ROM
identity, candidate identity, ordering, and absence of symlinks or
unmanifested files. Open workspaces with `no_proven_banks` remain explicit and
may still carry the cold candidate receipt; they do not manufacture bank
authority.

The label adapter must receive the returned identity before it opens an answer
key. It then constructs a compact address-space/bank/VA lookup while streaming,
opens a bounded digest-pinned label file, and emits exhaustive attribution
statuses and miss clusters. Its candidate denominator is the unique union of
combined, per-detector-only, translated, and ungradable identities; exact
matches remain explicitly candidate-level and do not claim proven ownership
or extent. The cold artifacts are immutable inputs to that operation: labels
cannot change mappings, scan bounds, candidates, or detectors. This separation
makes cached validation cheap and permits leave-one-ROM-out training without
loading all snapshot JSON into memory.

The working-tree `ProgramSnapshotV1` Rust type (serialized as schema v5) is
deliberately a smaller compatibility artifact while S1-S4 remain open: one or
more byte-verified physical or proven-file-table-resolved VROM banks, the cloned
fact log, broad exploratory CFG/value-set closure, separately retained
authority-rooted closure, partition, complete owner report, blocker histogram,
and separated coverage metrics. Candidate traversal roots can therefore widen
discovery without entering execution-closure evidence. It proves the
pass-composition and serialization boundary without pretending the planned
columnar indexes/CSR graph already exist. Its JSON is an
inspection/interchange form, not the hot cache format described below.
Historical schema v4 added typed, candidate-only handler-table pointer provenance (table
base, exact source slot, ordinal, stride, and admitted run length). It does not
change the authority closure: a structural pointer run is not an identified
descriptor table and cannot become a proven callable entry by serialization.
Schema v5 adds typed semantic-callable entry evidence carrying the target,
call site, callee, pointer register, and validated contract. Those exact Proven
claims are re-applied whenever prepared traversal facts are rebuilt; the V3
candidate receipt exposes the same authority without discarding its provenance.

The compatibility multi-bank composer is intentionally bounded. Before
composition it indexes facts by every bank they reference, including both ends
of cross-bank edges and nested function-entry evidence. Owned conclusion
subjects (`bank`, function, executable range, table entry, and observed trace
claims) explicitly identify their semantic bank; projection validates that
at least one expected typed justification regenerates the identical complete
canonical subject (including address/range/index/edge), rejects every same-kind
mismatch, and fails closed on malformed or unknown caller-authored subjects.
Thus a foreign call site retains its raw
function-entry claim while the merged function conclusion stays with the target
bank. Table-level aggregate conclusions remain program diagnostics rather than
being copied into every bank. Bankless VROM-to-physical file records are
assigned only to **proven** VROM bank mappings whose source interval they
contain; unrelated or merely candidate mappings cannot affect that bank's
materialization or proof. Other truly unscoped facts remain global. Each
snapshot receives only this bank projection and the complete justification set
for every retained conclusion; conclusion indices are remapped without dangling
references.
Zero-justification conclusions remain global rather than being assigned a
guessed scope. Its default fail-closed envelope is 4,000,000
aggregate projected fact rows, 256 MiB of aggregate projected compact fact JSON,
256 MiB of aggregate materialized bank bytes, and 1,048,576 unique cross-bank
authority records. `MultiBankCompositionLimits` and the `*_with_limits` entry
points make a different envelope explicit; raising one count does not bypass
the independent byte limits. Cross-bank target lookup is a deterministic
interval point query which returns every overlapping bank, so mutually
exclusive banks at the same VA retain the earlier all-match authority semantics
without scanning the complete catalog for each call.

These limits are a safety layer, not the final storage design. The remaining
structural step is a streaming two-pass composer: first byte-verify and close
one bank at a time while retaining only a bounded, digest-bound cross-bank
authority run; then point-query that complete run and finish/serialize one bank
at a time with one temporary projected `FactDb`. A future bundle wire can store
global facts once and bank-local deltas separately; today's compatibility wire
still repeats global facts in each projection and returns every snapshot in one
`Vec`.

The companion Rust type `BlockPackV1` emits wire schema V3. Each block stores a
tagged `BankBackingSpanV1`: affine Physical/VROM coordinates or an evaluated-
image receipt identity with output-relative offsets. Authoritative emission
accepts only the move-only `ValidatedComposedSnapshotsV2` minted by V6 byte-
verifying composition; deserialized `ProgramSnapshotV1` remains diagnostic.
V5 snapshots are not promoted and must be regenerated. Legacy V1 physical and
V2 affine pack wires remain readable and retain their old restrictions; V1
rejects Virtual backing and both reject evaluated-image backing. The pack
stores sorted disjoint geometry and digests, never instruction words.
Materialization checks ROM identity, re-derives evaluated output under fixed
bounds when present, checks each block digest, and feeds the sparse emitter
without widening spans across holes. The ROM-only workspace publisher and the
external-tool staging command remain explicitly affine-only. This is
intentionally not the final indexed program database:
the runtime's small `CodeCatalog` now provides a binary-search sparse address
index, but live generated-runner registration and generation-aware cache
invalidation remain separate open mechanisms.

The opaque composition wrapper closes post-composition snapshot forgery; it is
not a provenance signature over an arbitrary input `FactDb`. Composition still
trusts `Proven` mapping and entry conclusions already present in `base_facts`.
Current project gates satisfy that boundary by passing the in-process output of
the configured discovery/evidence pipeline. A file or API intake boundary must
not deserialize or caller-author `FactDb` proof conclusions and then treat the
resulting wrapper as independent validation; closing that broader intake case
would require a separately opaque validated-evidence capability.

```text
ProgramSnapshot
  header                 schema, ROM ID, evidence-set ID, algorithm IDs
  strings                interned aliases and cited free text
  sources                manifests, traces, detector/tool runs
  images / banks         stable ID table plus dense fields
  mapping_segments       typed spans and transforms
  region_features        multi-scale candidate features
  claims_by_kind         one typed table per atomic claim kind
  conclusions            typed subjects, state, rule, justification range
  blocks / control_edges canonical block graph
  data_objects           tables, strings, blobs, relocations, unknown regions
  reference_edges        load/store/address-taken and table membership edges
  root_claims             callable-entry evidence
  indirect_sites         exhaustive/bounded/open target sets
  owner_views            exact/coarse/ambiguous/unowned derived assignment
  dependency_edges       artifact/pass invalidation graph
```

Column-oriented here means each frequently filtered field is contiguous. It
does not require Apache Arrow. For example, all direct-call sources are one
`Vec<BlockIx>`, targets another `Vec<TargetRef>`, and evidence ranges another.
Rust enum payloads are split into tables by kind rather than stored in one
large tagged union. This provides predictable scans, avoids allocating fields
irrelevant to a query, and permits a new fact kind without rewriting every old
row.

Builders may use `HashMap` for speed, but hashing is never iteration order.
Freeze sorts by the canonical tuple for each table, remaps temporary IDs, and
deduplicates exact rows. Parallel providers produce sorted runs; merge is a
stable k-way merge.

### Facts, conclusions, and provenance

Split the current `Fact` concept into four tables:

- `Source`: immutable origin metadata: provider name/version, normalized input
  digest, parameter digest, external citation, trace scenario, and parent
  source IDs.
- `Claim`: a typed atomic assertion with `ClaimId`, `SourceId`, subject, and
  payload. Two providers making the same assertion create distinct claims
  because their source IDs differ.
- `Derivation`: rule ID, ordered input claim/conclusion IDs, output subject,
  output state, and algorithm version. This is the history currently implicit
  in calls to `conclude`.
- `Conclusion`: the materialized latest state for one typed `SubjectKey`, plus
  a range into its supporting derivations.

`SubjectKey` is an enum of typed IDs, not a formatted string:

```rust
enum SubjectKey {
    Bank(BankId),
    Mapping(MappingId),
    Executable(RegionId),
    Word(BankIx, WordOffset),
    Block(BlockId),
    FunctionEntry(BankVa),
    Owner(RootId),
    IndirectSite(BlockId, u16),
    DataObject(DataId),
}
```

Maintain secondary indexes as sorted posting lists:

- subject -> claim/derivation ranges;
- source -> claims;
- proof state -> conclusions;
- bank -> every bank-local table range;
- address interval -> claims that touch it;
- unresolved-kind -> frontier items.

Most lookups then become `O(log n + k)` or direct slice access instead of an
`O(F)` scan. Justifications use stable claim IDs on disk and dense claim
indices only inside a frozen snapshot.

The proof-state transition table should be explicit data tested for every
pair. `Rejected` and `Conflict` are terminal outcomes until a named rule that
consumes genuinely new evidence creates a successor derivation. A generic
"all transitions except weakening Proven" predicate is too weak to encode
that contract.

## Canonical block and reference graph

Decode each bank-backed executable candidate into instruction rows, but make
basic blocks the smallest canonical graph node:

```rust
struct BlockRow {
    id: BlockId,
    bank: BankIx,
    start_word: u32,
    word_len: NonZeroU16, // spill to u32 side table for exceptional blocks
    byte_digest: Digest256,
    terminator: TerminatorTag,
    flags: BlockFlags,
    evidence: Range<EvidenceIx>,
}
```

`start_word` is relative to the bank mapping, so common operations use checked
integer addition rather than repeated VA-map lookups. Absolute VA remains a
derived typed value. A block containing more than `u16::MAX` words is an
anomaly and uses a loud side-table escape, not truncation.

Control edges use forward and reverse compressed sparse row (CSR) arrays:

```text
out_offsets[V + 1]  out_targets[E]  out_kind[E]  out_site[E]  out_evidence[E]
in_offsets[V + 1]   in_sources[E]   in_kind[E]
```

Edge kinds distinguish fallthrough, branch-taken, branch-likely fallthrough,
direct call, local jump, candidate tail, exhaustive indirect jump, and
return-to-caller continuation. Delay-slot ownership is a block field/edge
property; consumers never reconstruct it from adjacency.

CSR gives `O(out-degree)` successor and `O(in-degree)` predecessor queries,
compact sequential traversal, and deterministic serialization. Mutation
happens in an edge-list builder; every analysis checkpoint freezes a new CSR
artifact. Discovery is naturally pass-oriented, so a fully dynamic graph
library would add complexity without improving the common path.

Calls are also projected into a separate call graph. Its nodes are callable
entry views, unresolved target-set nodes, and external/runtime symbols. A call
edge retains its source block and instruction site. Direct control-flow edges
inside a caller remain in the block graph; the call graph is an index, not a
replacement.

The current migration seam is `program_transfer_index::ProgramTransferIndex`.
It derives one deterministic edge set only from validated composed snapshots:
intra-bank CFG edges plus exact direct and exhaustive-resolved calls whose
typed facts identify another composed bank. It exposes raw forward/reverse
queries plus exact-owner `callees_of`/`callers_of` queries that retain source
blocks and edge-origin instruction sites. Cross-bank jumps remain omitted
until composition retains typed jump authority at this boundary. Open,
bounded, observed, candidate-owner, and ambiguous-owner evidence stays out of
the function projection. This interim view still uses vectors and `BTreeMap`
postings and is rebuilt per composed snapshot set; it is neither the frozen
CSR artifact nor the persistent corpus database specified here.

The program graph adds data nodes and typed reference edges:

- `Reads`, `Writes`, and `TakesAddressOf` from instruction site to data object;
- `TableContains` from a table object to a target/data object;
- `Relocates` for proven HI/LO, GP-relative, jump, and pointer constructions;
- `BackedBy` and `LoadedAs` between data/image/mapping nodes.

This is the metadata decompilers need to recover arrays, jump tables, strings,
relocations, and cross-references. A function list alone cannot answer those
questions.

### Word classifications

A `BTreeMap<u32, WordClass>` is expensive when millions of words become
reachable. Freeze classification per bank in 4 KiB byte pages (1,024 words):

- absent page: all `Unknown`;
- sparse page: a 1,024-bit presence bitmap plus packed three-bit classes;
- dense page: packed three-bit classes for all words.

Six current states fit in three bits. Construction uses a byte per word; the
freeze step chooses sparse or dense at a fixed documented occupancy threshold.
A fully classified 64 MiB ROM needs about 6 MiB for three-bit classes, before
page headers, rather than at least 16 MiB for bytes or substantially more for
tree nodes. Logical overlay classifications are bank-local and therefore may
legitimately count shared physical bytes more than once.

## SCCs, ownership, and incremental closure

Compute strongly connected components with deterministic Tarjan traversal
over blocks sorted by `BlockIx`; cost is `O(V + E)`. Store:

- block -> SCC;
- SCC member CSR;
- condensation DAG forward/reverse CSR;
- topological order;
- per-SCC flags for return, open indirect, conflict, and external escape.

The condensation DAG is the unit for value-flow scheduling and ownership
propagation. It prevents every loop iteration from re-enqueuing its individual
blocks and gives a natural cache boundary for abstract states.

Replace the current per-root DFS owner partition with claimant propagation on
the ownership DAG:

1. Freeze the callable-root set.
2. Derive ownership edges by excluding call targets and treating a tail edge
   as cross-owner only when its target is independently a root.
3. Seed each root's SCC with its `RootIx`.
4. In topological order, union claimant sets into successors.
5. A singleton set is exclusive ownership; two or more roots are explicit
   ambiguity; empty is unowned.

Use `SmallClaimants` with inline storage for zero, one, or two roots. Promote
larger sets to an interned sorted vector; add a Roaring-bitmap dependency only
if corpus measurements show large ambiguous sets dominate. This keeps the
common case tiny while bounding propagation at roughly
`O(E * claimant_union_cost)`, rather than `O(R * (V + E))`.

Root addition is incrementally monotonic: enqueue its SCC and propagate only
new claimant bits forward. An exhaustive indirect edge addition similarly
starts at the changed SCC. However, promoting a target to a callable root can
change whether an incoming tail edge is an ownership edge. Edge deletion and
proof rejection are not safely handled by monotonic propagation; invalidate
and recompute the affected bank's ownership artifact. The bank is the simple,
honest invalidation unit until profiling proves it too coarse.

SCC maintenance follows the same rule. Additions that stay inside one known
SCC reuse it; an edge that can close a cycle across SCCs invalidates SCCs for
that bank. Implementing a fully dynamic SCC algorithm is not justified for
N64-sized bank graphs before measurement.

Abstract interpretation caches input/output state by `(pass_id, scc_id,
predecessor_state_digest)`. When one block or edge changes, traverse the
reverse dependency CSR to invalidate only downstream SCC states. Widening
limits and worklist order remain fixed algorithm parameters included in the
cache key.

## Multi-scale feature store

The multi-pass idea in `DISCOVER-PLAN.md` should be stored as a feature
pyramid, not as independent vectors of verbose window structs.

At a base tile size (initially 64 bytes), emit integer columns for instruction
validity, coherent branch/jump counts, returns, zero words, plausible
ROM/VROM/VA pointers, nonzero bytes, and other detector-specific counts.
Construct dyadic aggregate levels by summing adjacent rows. Adjacent-window
derivatives are views over neighboring rows, not separately duplicated data.
This gives:

- every power-of-two scale in `O(1)` per window after an `O(N)` build;
- a total row count below twice the base level;
- deterministic exact integer features, with no floating-point ordering;
- direct correspondence from a transition back to physical bytes and claims.

Features that are not additive, such as exact byte diversity or entropy, are
computed at selected documented scales and stored in their own columns, or
computed on demand from the referenced blob. Do not approximate a proof input
with probabilistic sketches. A sketch may rank candidates but remains labeled
as such.

Each detector consumes named feature columns and emits claims. It never writes
a blended confidence back into the feature table. Prefix sums over base
columns support arbitrary aligned window experiments without rescanning ROM
bytes; accepted production scales remain explicit algorithm parameters.

## Cross-ROM corpus indexes

Cross-ROM matching needs a corpus-wide inverted index separate from each ROM's
program snapshot. Store several independent fingerprints because each masks a
different source of variance:

1. exact block bytes;
2. decoded instruction words with jump targets and proven relocation fields
   masked;
3. opcode/register skeletons with immediates retained in a side channel;
4. rolling instruction n-grams, initially 4, 8, and 16 words;
5. local CFG/SCC shape hashes computed for fixed refinement rounds;
6. constant, string, table-shape, and relocation-use fingerprints.

An inverted posting is:

```text
(ROM ID, Bank ID, Block ID, word offset, fingerprint kind)
```

Postings are sorted and delta-encoded within a ROM/bank. Select rare n-grams
first, intersect postings, and then validate the complete candidate bytes,
decoded instructions, relocation compatibility, and graph neighborhood. A
hash or locality-sensitive match proposes a candidate only. It never proves
a function boundary or transfers a semantic name authoritatively.

Use deterministic winnowing to limit dense rolling-hash postings: select the
minimum `(hash, position)` in each fixed window with a specified tie rule.
Bottom-k/MinHash or LSH tables may be added for ranking large corpora, using
fixed published seeds, but exact verification remains mandatory. Keep a
runner-up score so a non-unique match cannot be presented as unique.

For the corpus catalog, SQLite is a reasonable first implementation: it gives
transactional schema migrations and indexed lookup without making SQLite the
canonical evidence format. Use `WITHOUT ROWID` tables keyed by fingerprint
kind/hash and store large sorted postings in immutable content-addressed
blobs. Benchmark this against a sorted on-disk key/postings file before adding
a specialized database. A general graph database is not appropriate: hot
queries are dense traversal and range lookup, for which CSR and flat arrays
are simpler and faster.

## Content-addressed pass cache

Use a two-tier persistence model:

- **Evidence/export artifacts:** versioned, canonical, portable snapshots and
  human-readable reports. These are the basis of claims.
- **Local acceleration cache:** disposable content-addressed pass results and
  the corpus fingerprint catalog. Cache presence is never evidence.

Every pass key is:

```text
SHA-256(
  artifact schema major/minor,
  pass algorithm ID and semantic version,
  normalized ROM digest,
  evidence-manifest digest,
  ordered upstream artifact digests,
  canonical parameter bytes
)
```

Paths, mtimes, thread count, and iteration order are forbidden cache-key
inputs. A source-code change that alters semantics must change the algorithm
version; debug-only changes need not invalidate data. A trace-ingest change
invalidates trace-derived artifacts and their descendants, not normalization,
static decode, or region features.

Store objects under a user-selected out-of-tree cache root as
`objects/aa/<full digest>`, written to a same-directory temporary file,
checksummed, fsynced when requested, and atomically renamed. A per-key lock
prevents duplicate writers; duplicate computation is otherwise harmless
because objects are immutable. Named run refs point to root artifact digests.
Garbage collection walks refs through dependency edges.

For game-derived discovery and external-tool data, the cache root is further
partitioned as `roms/<full RomId>/`; its content-addressed objects, banks, tool
runs, refs, and reports remain inside that namespace. There is no cross-ROM
hard-linking or deduplication of game-derived objects. This deliberately trades
some disk reuse for auditable containment and makes deletion of one ROM's
derived data a namespace-local operation. `DISCOVER-TOOLCHAIN.md` defines the
directory layout, permissions, symlink policy, and immutable tool-run identity.
A shared cache is permitted only for non-game tool binaries/environments.

ROM bytes, materialized game data, traces, emitted code, and derived snapshots
remain out of git per `AGENTS.md`. Repo tests use only synthetic fixtures.

Suggested artifact boundaries are:

```text
normalized ROM -> mappings -> materialized images -> region features
               -> decode candidates -> frozen block graph -> SCC/value closure
               -> root claims -> owner views -> packs/reports
trace ---------> observed claims/activation -----------^
corpus index --> homology candidates -----------------^
```

This makes an experimental detector cheap: it reuses normalization, images,
features, and decode, then invalidates only its claims and downstream views.

## Serialization and migration

The portable binary snapshot has:

```text
magic = "FN64DSC\0"
schema_major, schema_minor
header length and table-directory length
ROM ID, evidence-set ID, producer/algorithm IDs
table directory: kind, version, row count, offset, length, SHA-256
independent canonical table payloads
root artifact SHA-256
```

Use fixed-width little-endian integers and length-prefixed UTF-8. Sort every
table by its declared canonical key. Maps and sets serialize as sorted rows;
no serializer-dependent map order is allowed. CSR targets are sorted by
`(source, edge kind, target, site, evidence)` before offsets are generated.

Table versions permit adding an optional table without rewriting unrelated
payloads. Readers skip unknown optional tables and reject unknown required
tables. A schema-major mismatch fails loudly. A schema-minor migration is a
pure, tested `vN -> vN+1` transformation. A changed proof rule or decoder is
not a schema migration: it changes the pass algorithm ID and recomputes the
affected artifact.

Keep canonical JSON export for inspection and interchange, with numeric
addresses rendered explicitly and tables sorted. JSON is not the hot cache
format. Avoid serializing `rkyv` memory layouts as the evidence format: they
optimize zero-copy reads but couple durable data to Rust layout evolution.
Likewise, SQLite is an index/catalog, not the sole copy of a proof snapshot.

Before selecting a binary codec crate, benchmark custom table encoding,
`postcard`, and a simple length-delimited format on the corpus. The schema
above, not a codec's derive macro, is the compatibility contract.

## Required queries and costs

The implementation is acceptable only when these queries are direct:

| Query | Index / representation | Target cost |
|---|---|---:|
| Physical/VROM mappings overlapping a span | augmented interval arrays | `O(log M + k)` |
| Bank mapping containing a VA | sorted per-bank intervals | `O(log M)` |
| All banks that could own a raw VA | unqualified VA interval index | `O(log M + k)` |
| Materialize image bytes | mapping index + blob slice | `O(log M + output)` |
| Claims/evidence for a subject | subject posting range | `O(log C + k)` |
| Full provenance ancestry | derivation adjacency | `O(visited claims)` |
| Block starting at/containing VA | per-bank sorted starts | `O(log V)` |
| Successors/predecessors | forward/reverse CSR | `O(degree)` |
| Callers/callees and call sites | call-graph CSR | `O(degree)` |
| SCC and owner for a block | dense arrays | `O(1)` |
| All ambiguous/unowned blocks | state posting ranges | `O(k)` |
| Open indirect sites in bank/range | bank/state/range index | `O(log I + k)` |
| Data/string/table xrefs | reference CSR | `O(degree)` |
| Feature window/derivative | feature pyramid/prefix column | `O(1)` |
| Cross-ROM n-gram matches | corpus inverted index | `O(log K + postings)` |
| Physical/logical/executable coverage | cached interval unions | `O(1)` report read |
| Artifacts affected by changed evidence | reverse dependency CSR | `O(affected)` |

`k` is output size. Performance tests should assert asymptotic behavior with
synthetic many-bank/many-root inputs, not only wall-clock time on one ROM.

## Memory and speed budget

An N64 ROM can be large enough that per-word tree nodes are wasteful, but
small enough that compact arrays fit comfortably:

- 64 MiB contains about 16.8 million words.
- Three-bit dense word classifications are about 6 MiB, plus page metadata.
- A block target in CSR is four bytes; edge kind and compact site metadata
  should keep common control edges near 8-12 bytes before evidence postings.
- Stable 32-byte digests live once in ID tables; hot rows use four-byte dense
  indices.
- Base feature tiles and their aggregate pyramid must have an explicit per-ROM
  byte budget. Expensive feature families are separate cache artifacts and
  loaded on demand.
- Materialized blobs are reference-counted/memory-mapped and sliced, never
  copied per detector.

Do not optimize solely for the theoretical 64 MiB dense case. Overlays create
logical bytes that can exceed unique physical bytes, and corpus matching can
span hundreds of ROMs. Report both unique blob memory and logical bank memory.

Initial performance gates on a warm local cache should be:

- no `O(number of facts)` scan in an address or subject point query;
- no per-root full CFG traversal in ownership;
- no duplicate byte allocation for executable subranges;
- graph freeze and serialization deterministic across 1 and N worker threads;
- changing one detector reuses normalization, mapping, feature, and decode
  artifacts;
- ten clean deterministic runs produce identical root artifact digests.

Wall-clock targets belong in `DISCOVER-PLAN.md` and should be adjusted from
measurements. These structural gates should not.

## Downstream views: recompilation and decompilation

### Region/basic-block recompilation

A recompiler need not require source-like function partitions. A region
adapter can group canonical blocks by bank, executable interval, SCC, and
contiguous layout into `CompileRegion` nodes. Each region emits a dispatcher
over block entry labels. The callable-entry table maps
`(BankId, VA) -> (CompileRegionId, BlockIx)`. Direct edges can become native
branches/calls where semantics permit; unresolved indirect transfers use the
bank activation/address dispatcher and trap loudly when no proven target can
be resolved.

This representation can also support an interrupted or interior block entry,
which the current whole-function runtime shape cannot represent
(`DESIGN.md` section 1.0 documents that wall). Adopting it is a separate
runtime/recompiler design decision, not something the discovery database can
silently assume. The graph makes the option available and gives it exact
inputs.

Region recompilation still requires every executed instruction and transfer
to be correct. It merely changes the required partition from "recover the
original function extent" to "recover executable blocks and dispatchable
edges." Unknown blocks and open indirects remain explicit compile errors or
loud runtime traps.

### Function-oriented decompilation adapter

The function adapter consumes root claims and owner propagation:

- one root plus a contiguous exclusive block run becomes an exact
  address/size function row;
- a non-contiguous exclusive owner becomes a block-list function view, not a
  fabricated flat extent;
- shared tails are emitted once with aliases/edges from each claimant, or
  duplicated only if the target tool explicitly requires it and the adapter
  records that transform;
- interior callable entries become aliases into the same owner when proven;
- ambiguous blocks and open ends are rejected from authoritative packs and
  emitted in the frontier report;
- candidate partitions may be exported separately for Splat/spimdisasm/Ghidra
  experiments, labeled with their proof state and never mixed into the proven
  pack.

The same graph can therefore feed current N64Recomp-style function metadata,
Splat section/symbol configuration, Ghidra review inputs, and a future
region emitter. None of those adapters owns the underlying truth.

## Staged migration from `FactDb`

Do not rewrite the pipeline in one step. Every stage keeps the old path as an
oracle until results match.

### S0: measurement

- Add counters/timers for fact scans, byte copies, CFG rebuilds, owner-edge
  visits, and serialized bytes.
- Add synthetic stress fixtures with thousands of banks/roots and overlapping
  raw VAs.
- Record current deterministic JSON digests and all corpus grade metrics.

### S1: typed IDs and indexes behind `FactDb`

- Intern bank names and introduce `BankId`/`BankIx`, typed addresses, and
  typed `SubjectKey` internally.
- Keep `FactDb`'s public API, but maintain subject, bank, fact-kind, proof-state,
  and interval indexes on insert/freeze.
- Replace formatted-string lookups in hot paths. Assert indexed query results
  equal legacy scans in tests.

### S2: blob slices and mapping indexes

- Introduce `BlobStore`, `BlobSlice`, `MappingSegment`, and the three interval
  indexes.
- Remove `LoadImage`/executable-range byte copies.
- Differentially compare every materialized byte digest and mapping query.

### S3: typed columnar claims and derivations

- Split the `Fact` enum into per-kind tables while retaining a compatibility
  iterator for existing code.
- Give claims stable content IDs and make conclusion derivations first-class.
- Add proof-state transition-matrix tests and canonical freeze ordering.
- Emit legacy JSON and the new snapshot from the same run; compare semantic
  rows.

### S4: frozen CSR block/reference graph

- Convert the current `Cfg` result to forward/reverse CSR without changing its
  builder initially.
- Move call, tail, indirect, xref, and block containment queries to graph
  indexes.
- Compare all current CFG and value-set tests edge-for-edge.

### S5: SCC and claimant propagation

- Add SCC condensation and replace per-root partition DFS with claimant
  propagation.
- Differentially compare owner, ambiguous, and unowned sets against the old
  partitioner on all fixtures and corpus banks.
- Add incremental-root tests and full-bank invalidation tests for root
  rejection/tail-edge changes.

### S6: artifacts and cache

- Define the versioned table directory and pass keys.
- Cache normalization, mappings, features, decode, graph, closure, and views
  independently.
- Test corruption rejection, atomic duplicate writers, migration, selective
  invalidation, cold/warm equality, and one-thread/many-thread identity.

### S7: corpus index and adapters

- Build exact and relocation-masked block indexes first; add n-gram winnowing
  and graph fingerprints only after exact verification tests exist.
- Emit both a current function-oriented pack and an experimental block/region
  pack from the same snapshot.
- Grade entry precision/recall, exact/coarse owner coverage, executable-byte
  closure, unresolved transfers, recompiler acceptance, and total ROM content
  classification separately.

No migration stage changes a proof rule and a storage representation in the
same patch. A performance improvement that changes authoritative conclusions
is an algorithm change and must be reviewed and graded as one.

## Decisions deliberately deferred to measurement

- `postcard`, custom table encoding, or another binary codec;
- SQLite versus a sorted key/postings file for the corpus catalog;
- interned sorted claimant sets versus Roaring bitmaps;
- memory mapping every frozen table versus loading small tables eagerly;
- exact base feature tile size and retained pyramid scales;
- incremental SCC maintenance finer than one bank.

The interfaces above permit each choice without changing evidence identity or
downstream pack semantics. Benchmark implementations on multiple ROM families
and a held-out ROM before committing to a specialized dependency.
