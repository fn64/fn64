//! Phase 2+ core (docs/DISCOVER-DESIGN.md "Monotonic fact database"):
//! immutable evidence records with provenance, plus the discrete proof
//! states the design mandates instead of one blended confidence number.
//!
//! The non-negotiable invariant: **a heuristic never overwrites proven
//! evidence.** This module enforces that at the data-structure level, not
//! by convention -- [`FactDb::insert`] is append-only, and promoting a
//! fact's `ProofState` can only ever move it to a state at least as strong
//! (see [`ProofState::supersedes`]); an attempt to downgrade a `Proven`
//! fact is a logic error. Contradictory evidence may move it to `Conflict`,
//! which preserves rather than overwrites the disagreement.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// A bank-qualified address: identity is `(bank, pc)`, never `pc` alone,
/// per the design doc's "Function identity must be bank-qualified from the
/// first instruction."
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BankAddr {
    pub bank: String,
    pub pc: u32,
}

/// Which ROM coordinate system a load-image range uses. N64 titles may put
/// executable bytes directly at physical cartridge offsets, or name files by
/// a virtual ROM (VROM) interval that a separate DMA table resolves to a
/// physical, possibly-compressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RomAddressSpace {
    Physical,
    Virtual,
}

/// Closed evaluator identity for an image produced from immutable ROM input.
/// A new implementation or container interpretation requires a new variant;
/// free-form names cannot silently change the receipt's meaning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterializationEvaluatorV1 {
    /// `0x1172`, six-byte header, four-byte big-endian output length.
    HeaderedRawDeflateSequenceV1 { stream_count: u32 },
    /// `0x1173`, five-byte header, three-byte big-endian output length.
    #[serde(rename = "headered_raw_deflate_1173_sequence_v1")]
    HeaderedRawDeflate1173SequenceV1 { stream_count: u32 },
}

/// Immutable encoded input selected from the normalized ROM. `cursor` is
/// relative to `[rom_start, rom_end)`, not a host pointer or runtime VA.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MaterializedImageSourceV1 {
    pub rom_space: RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
    pub cursor: u32,
}

/// Half-open offset interval within an encoded source or evaluated output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MaterializedByteRangeV1 {
    pub start: u32,
    pub end: u32,
}

/// Content-only receipt for one stream. All ranges are relative offsets; none
/// can be mistaken for a ROM coordinate or runtime address.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MaterializedImageStreamV1 {
    pub source_range: MaterializedByteRangeV1,
    pub encoded_range: MaterializedByteRangeV1,
    pub output_range: MaterializedByteRangeV1,
    pub declared_output_len: u32,
    pub source_sha256: String,
    pub output_sha256: String,
}

/// Identity of bytes left after the explicitly requested stream sequence.
/// The bytes themselves are deliberately absent from the fact wire.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MaterializedImageSuffixV1 {
    pub offset: u32,
    pub len: u32,
    pub sha256: String,
}

/// Reproducible, content-free record of one candidate evaluated image. This
/// proves the evaluator result only; it does not prove that guest code invokes
/// the evaluator, writes the destination, or transfers control to the output.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EvaluatedImageReceiptV1 {
    pub evaluator: MaterializationEvaluatorV1,
    pub source: MaterializedImageSourceV1,
    pub source_sha256: String,
    pub output_len: u32,
    pub output_sha256: String,
    pub streams: Vec<MaterializedImageStreamV1>,
    pub trailing_suffix: MaterializedImageSuffixV1,
}

/// Stable identity for a serialized evaluated-image receipt. The hash wire is
/// manual and domain-separated so serde representation changes cannot alter
/// an existing receipt identity.
pub fn evaluated_image_receipt_sha256_v1(receipt: &EvaluatedImageReceiptV1) -> String {
    fn hash_u32(hasher: &mut Sha256, value: u32) {
        hasher.update(value.to_be_bytes());
    }
    fn hash_u64(hasher: &mut Sha256, value: u64) {
        hasher.update(value.to_be_bytes());
    }
    fn hash_str(hasher: &mut Sha256, value: &str) {
        hash_u64(hasher, value.len() as u64);
        hasher.update(value.as_bytes());
    }
    fn hash_range(hasher: &mut Sha256, range: MaterializedByteRangeV1) {
        hash_u32(hasher, range.start);
        hash_u32(hasher, range.end);
    }

    let mut hasher = Sha256::new();
    hasher.update(b"fn64.evaluated-image-receipt.v1\0");
    match receipt.evaluator {
        MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count } => {
            hasher.update([1]);
            hash_u32(&mut hasher, stream_count);
        }
        MaterializationEvaluatorV1::HeaderedRawDeflate1173SequenceV1 { stream_count } => {
            hasher.update([2]);
            hash_u32(&mut hasher, stream_count);
        }
    }
    hasher.update([match receipt.source.rom_space {
        RomAddressSpace::Physical => 1,
        RomAddressSpace::Virtual => 2,
    }]);
    hash_u32(&mut hasher, receipt.source.rom_start);
    hash_u32(&mut hasher, receipt.source.rom_end);
    hash_u32(&mut hasher, receipt.source.cursor);
    hash_str(&mut hasher, &receipt.source_sha256);
    hash_u32(&mut hasher, receipt.output_len);
    hash_str(&mut hasher, &receipt.output_sha256);
    hash_u64(&mut hasher, receipt.streams.len() as u64);
    for stream in &receipt.streams {
        hash_range(&mut hasher, stream.source_range);
        hash_range(&mut hasher, stream.encoded_range);
        hash_range(&mut hasher, stream.output_range);
        hash_u32(&mut hasher, stream.declared_output_len);
        hash_str(&mut hasher, &stream.source_sha256);
        hash_str(&mut hasher, &stream.output_sha256);
    }
    hash_u32(&mut hasher, receipt.trailing_suffix.offset);
    hash_u32(&mut hasher, receipt.trailing_suffix.len);
    hash_str(&mut hasher, &receipt.trailing_suffix.sha256);
    format!("{:x}", hasher.finalize())
}

/// Backing for one complete proven bank image. Materialized output has no ROM
/// coordinates: it is identified by the evaluator receipt and output length.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BankBackingV1 {
    RomAffine {
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
    },
    Materialized {
        receipt_sha256: String,
        output_len: u32,
    },
}

/// Backing for a subrange of a proven bank. Materialized offsets are relative
/// to the evaluated output and cannot be consumed as cartridge addresses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BankBackingSpanV1 {
    RomAffine {
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
    },
    Materialized {
        receipt_sha256: String,
        output_start: u32,
        output_end: u32,
    },
}

/// Result of resolving one nonempty runtime interval against a bank's single
/// accepted complete image. Ambiguity is decided before interval coverage:
/// choosing whichever of two competing images happens to cover a request
/// would silently turn the request into backing authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BankBackingSpanResolutionV1 {
    Missing,
    Unique(BankBackingSpanV1),
    Ambiguous,
    InvalidGeometry,
}

/// Generalized proven bank geometry. The accessor that returns this type is
/// conclusion-gated; merely inserting an evaluated-image fact is insufficient.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProvenBankImageV1 {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
    pub backing: BankBackingV1,
}

/// Address spaces connected by a configurable Phase-2 range table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MappingAddressSpace {
    PhysicalRom,
    VirtualRom,
    Vram,
}

/// Linker-declared section containing an overlay relocation site. This is the
/// section encoded in the Zelda overlay relocation word, not a section guessed
/// from the relocated value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayRelocationSection {
    Text,
    Data,
    Rodata,
}

/// Machine-readable relocation operation retained by the canonical fact log.
/// A relocated address is not automatically a callable function entry: the
/// consumer and target role remain separate proof obligations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayRelocationKind {
    Mips32,
    Mips26Call,
    Mips26Jump,
    Hi16Lo16Address,
}

/// How the value associated with an overlay relocation was obtained. This is
/// deliberately separate from relocation kind: a HI16/LO16 value depends on
/// the parser's register-pairing model and is candidate evidence, while an
/// R_MIPS_32 word is retained directly from the unrelocated image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayRelocationValueEvidence {
    StoredWord,
    JumpInstructionEncoding,
    RegisterPairedHi16Lo16,
}

/// The deterministic Phase 3 provider that produced a function-entry claim.
/// These values are part of serialized provenance, so adding a provider must
/// add a new variant rather than reusing a free-form string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CandidateDetector {
    HardwareEntrypoint,
    JalTarget,
    IndirectCallTarget,
    SemanticCallableArgument,
    ProloguePattern,
    ArgumentHomeSpillLeaf,
    TableDerived,
}

/// The exhaustive static construction behind an indirect-call target claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectCallEvidenceKind {
    Constant,
    MemoryValueSet,
    JumpTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectTransferState {
    Exhaustive,
    Bounded,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IndirectTransferKind {
    Constant,
    MemoryValueSet,
    JumpTable,
}

/// Why a finite indirect-target domain is safe to use as an exclusion proof.
///
/// This is deliberately narrower than [`IndirectTransferState::Bounded`].
/// Today only a guard-bounded jump table demoted by the resolver's target
/// usability check carries a finite over-approximation of its destinations.
/// Initial values read from mutable memory are not an exhaustive runtime
/// domain and therefore have no variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IndirectTargetDomainBasisV1 {
    GuardBoundedFinite,
}

/// Typed exclusion-only view of an indirect transfer's finite target domain.
///
/// A domain never contributes CFG successors or callable-entry authority. It
/// may only prove that a bank-scoped unresolved transfer cannot enter a
/// disjoint function-owner extent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndirectTargetDomainV1 {
    pub site: BankAddr,
    pub via_call: bool,
    pub basis: IndirectTargetDomainBasisV1,
    pub targets: Vec<u32>,
}

/// The prologue shape that justified a [`CandidateDetector::ProloguePattern`]
/// claim. A leaf claim requires a matched stack restore and `jr $ra`; a stack
/// adjustment by itself is deliberately insufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProloguePattern {
    SavesReturnAddress,
    LeafWithMatchedRestore,
}

/// Closed semantic contract that makes one constant o32 argument callable.
/// Each variant retains the exact reachable consumer evidence used by the
/// byte-verified fixed point; a generic pointer argument is never authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SemanticCallableContract {
    OsCreateThread,
    ArgumentToJalr {
        jalr_sites: Vec<BankAddr>,
    },
    CallbackRegistry {
        dispatcher: BankAddr,
        callback_store_site: BankAddr,
        list_insert_site: BankAddr,
        jalr_site: BankAddr,
    },
}

/// Machine-readable evidence carried by every Phase 3 function-entry claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FunctionEntryEvidence {
    /// The effective program entry reached after an exactly identified IPL3's
    /// boot copy. It is derived from the normalized header entry plus that
    /// IPL3 profile's admitted relocation delta. This is authoritative
    /// reachability, not a heuristic prologue or tool claim.
    RomHeaderEntrypoint,
    DirectJal {
        call_site: BankAddr,
    },
    ResolvedJalr {
        call_site: BankAddr,
        construction_start: BankAddr,
    },
    ExhaustiveIndirectCall {
        call_site: BankAddr,
        kind: IndirectCallEvidenceKind,
        memory_sources: Vec<BankAddr>,
    },
    /// A reachable direct call supplies a statically proven code pointer to a
    /// mechanically closed callback/thread-entry contract. This is Proven
    /// only when composition rederives the call, delay word, constant operand,
    /// and contract from the byte-verified authority closure.
    SemanticCallableArgument {
        call_site: BankAddr,
        callee: BankAddr,
        pointer_register: u8,
        contract: SemanticCallableContract,
    },
    Prologue {
        stack_adjust: BankAddr,
        frame_size: u32,
        pattern: ProloguePattern,
        corroborating_site: BankAddr,
    },
    /// A frameless o32 leaf begins by spilling one incoming argument to its
    /// canonical caller-allocated home slot, follows a complete preceding
    /// `jr $ra` plus delay slot, and reaches its own bounded `jr $ra` without
    /// any intervening control ambiguity. This is candidate evidence only.
    ArgumentHomeSpillLeaf {
        predecessor_return: BankAddr,
        spill_site: BankAddr,
        argument_index: u8,
        return_site: BankAddr,
    },
    TableEntry {
        table: BankAddr,
        index: u32,
    },
    /// Candidate-only pointer slot in a mechanically recognized dense or
    /// fixed-stride table-shaped run. This is not an identified descriptor
    /// table and does not establish that any reachable consumer invokes it.
    HandlerTablePointer {
        table_base: BankAddr,
        source_slot: BankAddr,
        slot_ordinal: u32,
        stride_words: u8,
        run_length: u32,
    },
}

impl BankAddr {
    pub fn new(bank: impl Into<String>, pc: u32) -> Self {
        Self {
            bank: bank.into(),
            pc,
        }
    }
}

/// One piece of raw, atomic evidence. Facts are never mutated or removed
/// once inserted -- only new facts are added, and derived conclusions
/// (banks, function ownership, proof states) are recomputed from the full
/// fact set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fact {
    /// `source` performs a direct `jal`/`j` to `target`, both bank-qualified.
    DirectCall { source: BankAddr, target: BankAddr },
    /// `source` performs a computed call whose complete statically-proven
    /// target set includes `target`, both bank-qualified.
    ///
    /// Unlike an observed `jalr`, this fact is emitted only when the source
    /// bank's typed indirect-transfer analysis is `Exhaustive` and its target
    /// set exactly matches the CFG terminator. It therefore carries the same
    /// callable-entry authority as an in-bank exhaustive resolved call without
    /// pretending the instruction was a direct `jal`.
    ResolvedCall { source: BankAddr, target: BankAddr },
    /// `bank` has a basic-block boundary starting at `pc`.
    BlockStart { bank: String, pc: u32 },
    /// The instruction at `site` loads from computed address `address`
    /// (used for HI/LO pairing and jump-table base discovery).
    LoadsFrom { site: BankAddr, address: u32 },
    /// A dynamic trace observed an indirect call/jump at `site` actually
    /// transferring control to `target`. Existence, not exhaustiveness --
    /// see docs/DISCOVER-DESIGN.md Phase 6.
    ObservedIndirectTarget {
        site: BankAddr,
        target: BankAddr,
        trace: String,
    },
    /// A dynamic trace observed the instruction at `site` execute. This is
    /// bank-scoped dynamic code-existence evidence -- strictly weaker than a
    /// proven owner or a CFG-reachability proof: it proves one word decoded
    /// and ran once in the named bank's activation, never that the trace's
    /// observed set is exhaustive (see `trace.rs`). `trace`/`sequence` pin
    /// this fact to one exact record of one exact capture so the evidence
    /// stays auditable.
    ObservedExecutedCode {
        site: BankAddr,
        trace: String,
        sequence: u64,
    },
    /// Static Phase 6 result for one reachable `jr`/`jalr`. Only an
    /// exhaustive record may contribute CFG successors; bounded/open records
    /// preserve the unresolved frontier without guessing.
    IndirectTransferAnalysis {
        site: BankAddr,
        via_call: bool,
        state: IndirectTransferState,
        kind: Option<IndirectTransferKind>,
        targets: Vec<u32>,
        memory_sources: Vec<u32>,
    },
    /// A proven ROM-interval -> runtime-VA-interval mapping: the bank
    /// identity itself. `rom_start`/`rom_end` are ROM byte offsets in the
    /// normalized (big-endian) ROM; `va_start`/`va_end` are runtime
    /// addresses; both exclusive of `_end`.
    RomMapping {
        bank: String,
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
        va_start: u32,
        va_end: u32,
    },
    /// Candidate evaluated bytes at one runtime interval. The receipt is
    /// content-addressed and carries no output bytes. This fact alone is not a
    /// mapping proof: only a separate `bank:<bank>` Proven conclusion may
    /// expose it through [`FactDb::proven_bank_images`].
    EvaluatedImage {
        bank: String,
        va_start: u32,
        va_end: u32,
        receipt: EvaluatedImageReceiptV1,
    },
    /// A bank-qualified interval whose bytes are permitted to enter code
    /// discovery. Load-image mappings describe where bytes can be loaded;
    /// they do not imply that text, rodata, and data are all executable.
    ExecutableRange {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    /// One parsed record from an explicitly-located Phase-2 mapping table.
    /// This typed fact is the provenance for both VROM-to-physical-file
    /// mappings and ROM/VROM-to-VRAM load images: table identity and record
    /// index remain machine-readable rather than living only in prose.
    LoadImageTableRecord {
        table: String,
        bank: Option<String>,
        table_space: RomAddressSpace,
        table_offset: u32,
        index: u32,
        source_space: MappingAddressSpace,
        source_start: u32,
        source_end: u32,
        destination_space: MappingAddressSpace,
        destination_start: u32,
        destination_end: u32,
    },
    /// One linker-declared Zelda overlay relocation. `site` is the loaded
    /// address of the relocated word (the LO16 instruction for a paired
    /// HI16/LO16 address), while `unrelocated_value` is the stored or decoded
    /// value from the image. `value_evidence` records whether that value was
    /// direct or depended on the parser's register-pairing model. This proves
    /// a relocation fact only; it does not prove the run-time relocated value,
    /// reachability, or a callable boundary.
    OverlayRelocation {
        site: BankAddr,
        section: OverlayRelocationSection,
        kind: OverlayRelocationKind,
        value_evidence: OverlayRelocationValueEvidence,
        unrelocated_value: u32,
    },
    /// A code-pointer entry exposed by a descriptor table or vector that an
    /// earlier phase has already identified. Phase 3 consumes this fact; it
    /// never guesses table locations or semantics from raw bytes.
    TableEntry {
        table: BankAddr,
        index: u32,
        target: BankAddr,
    },
    /// One immutable Phase 3 provider result. `proposed_state` is the
    /// provider's own evidentiary strength; the `fn:<bank>:<pc>` conclusion
    /// is derived from all claims for the target by deterministic merge.
    FunctionEntryClaim {
        target: BankAddr,
        detector: CandidateDetector,
        evidence: FunctionEntryEvidence,
        proposed_state: ProofState,
    },
    /// Free-text provenance for a `RomMapping` or other claim -- e.g. "PI
    /// DMA descriptor at ROM 0x539a0, record 0" -- kept as a fact so the
    /// evidence trail survives independent of any derived report.
    Evidence { subject: BankAddr, note: String },
}

impl Fact {
    /// Project the one bounded analysis shape whose retained targets form a
    /// finite over-approximation suitable for owner-exclusion proofs.
    ///
    /// `Bounded` is not sufficient by itself: constant and memory-value-set
    /// records can describe only initial mutable state. The resolver emits a
    /// bounded jump-table record when a guard-enumerated table was rejected
    /// from CFG admission because at least one enumerated destination was not
    /// usable. Keeping the complete enumerated set here is conservative for
    /// disjointness and confers no positive reachability authority.
    pub fn indirect_target_domain_v1(&self) -> Option<IndirectTargetDomainV1> {
        let Fact::IndirectTransferAnalysis {
            site,
            via_call,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets,
            ..
        } = self
        else {
            return None;
        };
        if targets.is_empty() {
            return None;
        }
        let mut targets = targets.clone();
        targets.sort_unstable();
        targets.dedup();
        Some(IndirectTargetDomainV1 {
            site: site.clone(),
            via_call: *via_call,
            basis: IndirectTargetDomainBasisV1::GuardBoundedFinite,
            targets,
        })
    }

    /// The bank this fact is scoped to, if it names one directly (some
    /// facts, like cross-bank `DirectCall`, don't have a single owner).
    pub fn primary_bank(&self) -> Option<&str> {
        match self {
            Fact::DirectCall { source, .. } | Fact::ResolvedCall { source, .. } => {
                Some(&source.bank)
            }
            Fact::BlockStart { bank, .. } => Some(bank),
            Fact::LoadsFrom { site, .. } => Some(&site.bank),
            Fact::ObservedIndirectTarget { site, .. } => Some(&site.bank),
            Fact::ObservedExecutedCode { site, .. } => Some(&site.bank),
            Fact::IndirectTransferAnalysis { site, .. } => Some(&site.bank),
            Fact::RomMapping { bank, .. } => Some(bank),
            Fact::EvaluatedImage { bank, .. } => Some(bank),
            Fact::ExecutableRange { bank, .. } => Some(bank),
            Fact::LoadImageTableRecord { bank, .. } => bank.as_deref(),
            Fact::OverlayRelocation { site, .. } => Some(&site.bank),
            Fact::TableEntry { table, .. } => Some(&table.bank),
            Fact::FunctionEntryClaim { target, .. } => Some(&target.bank),
            Fact::Evidence { subject, .. } => Some(&subject.bank),
        }
    }

    /// Every bank named by this fact, including both ends of an edge and all
    /// nested function-entry evidence. This is the authoritative scope used
    /// when projecting the append-only database for one bank; `primary_bank`
    /// is deliberately insufficient for cross-bank authority.
    pub fn referenced_banks(&self) -> BTreeSet<&str> {
        fn insert_addr<'a>(banks: &mut BTreeSet<&'a str>, address: &'a BankAddr) {
            banks.insert(address.bank.as_str());
        }
        let mut banks = BTreeSet::new();
        match self {
            Fact::DirectCall { source, target } | Fact::ResolvedCall { source, target } => {
                insert_addr(&mut banks, source);
                insert_addr(&mut banks, target);
            }
            Fact::BlockStart { bank, .. }
            | Fact::RomMapping { bank, .. }
            | Fact::EvaluatedImage { bank, .. }
            | Fact::ExecutableRange { bank, .. } => {
                banks.insert(bank);
            }
            Fact::LoadsFrom { site, .. }
            | Fact::ObservedExecutedCode { site, .. }
            | Fact::IndirectTransferAnalysis { site, .. } => insert_addr(&mut banks, site),
            Fact::ObservedIndirectTarget { site, target, .. } => {
                insert_addr(&mut banks, site);
                insert_addr(&mut banks, target);
            }
            Fact::LoadImageTableRecord { bank, .. } => {
                if let Some(bank) = bank {
                    banks.insert(bank);
                }
            }
            Fact::OverlayRelocation { site, .. } => insert_addr(&mut banks, site),
            Fact::TableEntry { table, target, .. } => {
                insert_addr(&mut banks, table);
                insert_addr(&mut banks, target);
            }
            Fact::FunctionEntryClaim {
                target, evidence, ..
            } => {
                insert_addr(&mut banks, target);
                match evidence {
                    FunctionEntryEvidence::RomHeaderEntrypoint => {}
                    FunctionEntryEvidence::DirectJal { call_site } => {
                        insert_addr(&mut banks, call_site)
                    }
                    FunctionEntryEvidence::ResolvedJalr {
                        call_site,
                        construction_start,
                    } => {
                        insert_addr(&mut banks, call_site);
                        insert_addr(&mut banks, construction_start);
                    }
                    FunctionEntryEvidence::ExhaustiveIndirectCall {
                        call_site,
                        memory_sources,
                        ..
                    } => {
                        insert_addr(&mut banks, call_site);
                        for source in memory_sources {
                            insert_addr(&mut banks, source);
                        }
                    }
                    FunctionEntryEvidence::SemanticCallableArgument {
                        call_site,
                        callee,
                        contract,
                        ..
                    } => {
                        insert_addr(&mut banks, call_site);
                        insert_addr(&mut banks, callee);
                        match contract {
                            SemanticCallableContract::OsCreateThread => {}
                            SemanticCallableContract::ArgumentToJalr { jalr_sites } => {
                                for site in jalr_sites {
                                    insert_addr(&mut banks, site);
                                }
                            }
                            SemanticCallableContract::CallbackRegistry {
                                dispatcher,
                                callback_store_site,
                                list_insert_site,
                                jalr_site,
                            } => {
                                insert_addr(&mut banks, dispatcher);
                                insert_addr(&mut banks, callback_store_site);
                                insert_addr(&mut banks, list_insert_site);
                                insert_addr(&mut banks, jalr_site);
                            }
                        }
                    }
                    FunctionEntryEvidence::Prologue {
                        stack_adjust,
                        corroborating_site,
                        ..
                    } => {
                        insert_addr(&mut banks, stack_adjust);
                        insert_addr(&mut banks, corroborating_site);
                    }
                    FunctionEntryEvidence::ArgumentHomeSpillLeaf {
                        predecessor_return,
                        spill_site,
                        return_site,
                        ..
                    } => {
                        insert_addr(&mut banks, predecessor_return);
                        insert_addr(&mut banks, spill_site);
                        insert_addr(&mut banks, return_site);
                    }
                    FunctionEntryEvidence::TableEntry { table, .. } => {
                        insert_addr(&mut banks, table)
                    }
                    FunctionEntryEvidence::HandlerTablePointer {
                        table_base,
                        source_slot,
                        ..
                    } => {
                        insert_addr(&mut banks, table_base);
                        insert_addr(&mut banks, source_slot);
                    }
                }
            }
            Fact::Evidence { subject, .. } => insert_addr(&mut banks, subject),
        }
        banks
    }
}

/// Discrete proof states per docs/DISCOVER-DESIGN.md's "Result
/// classifications". Ordered weakest to strongest; [`ProofState::rank`]
/// gives the total order used to enforce monotonicity. `Conflict` and
/// `Rejected` are terminal in the sense that they require new evidence
/// (not mere heuristic re-scoring) to leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProofState {
    /// A required fact is still missing.
    Open,
    /// Produced by a heuristic or detector; no independent corroboration.
    Candidate,
    /// Corroborated by independent evidence, not yet authoritative.
    Supported,
    /// Contradicted by stronger evidence than what proposed it.
    Rejected,
    /// Incompatible evidence, or more than one valid interpretation.
    Conflict,
    /// Accepted by a defined proof rule; eligible for authoritative output.
    Proven,
}

impl ProofState {
    /// True if transitioning from `self` to `next` is a legal monotonic
    /// update: never move a `Proven` conclusion to anything weaker. A
    /// contradiction may move it to `Conflict`, which surfaces the new
    /// incompatible evidence without letting a heuristic win. All other
    /// transitions are allowed -- new evidence may raise a
    /// `Candidate` to `Proven`, demote a `Candidate` to `Rejected` when a
    /// stronger fact contradicts it, or turn `Open` into `Conflict` when
    /// two incompatible proofs arrive at once. `Rejected` and `Conflict`
    /// are not "better than Candidate" in evidentiary terms -- they are
    /// alternate terminal outcomes. A proven conclusion is protected from
    /// weakening while still allowing contradictory evidence to surface as
    /// `Conflict`.
    pub fn supersedes(self, next: ProofState) -> bool {
        if self == ProofState::Proven {
            return matches!(next, ProofState::Proven | ProofState::Conflict);
        }
        true
    }
}

/// Stable conclusion key for a bank-qualified function entry.
pub fn function_entry_subject(address: &BankAddr) -> String {
    format!("fn:{}:0x{:08x}", address.bank, address.pc)
}

/// Stable conclusion key for one upstream table/vector entry.
pub fn table_entry_subject(table: &BankAddr, index: u32) -> String {
    format!("table-entry:{}:0x{:08x}:{index}", table.bank, table.pc)
}

/// Stable conclusion key for one Phase-2 load-image/file-table record.
pub fn load_image_table_record_subject(table: &str, index: u32) -> String {
    format!("load-image-table:{table}:{index}")
}

/// Stable conclusion key for one bank-qualified executable interval.
pub fn executable_range_subject(bank: &str, va_start: u32, va_end: u32) -> String {
    format!("executable-range:{bank}:0x{va_start:08x}:0x{va_end:08x}")
}

/// Stable conclusion key for one bank-qualified word's dynamic
/// execution-observed evidence (`trace::fold_into_fact_db`'s subject).
pub fn observed_executed_code_subject(bank: &str, pc: u32) -> String {
    format!("observed-executed:{bank}:0x{pc:08x}")
}

/// Stable conclusion key for one observed indirect edge
/// `(site -> target)`, both bank-qualified
/// (`trace::fold_indirect_targets_into_fact_db`'s subject). The target is
/// part of the key: one `jr`/`jalr` site legitimately reaches many
/// targets across a trace, and each observed edge is its own existence
/// fact -- never collapsed, never treated as exhaustive.
pub fn observed_indirect_target_subject(
    site_bank: &str,
    site_pc: u32,
    target_bank: &str,
    target_pc: u32,
) -> String {
    format!("observed-indirect:{site_bank}:0x{site_pc:08x}->{target_bank}:0x{target_pc:08x}")
}

/// A derived conclusion tracked with its proof state and the fact
/// indices that justify it. Conclusions are recomputed by proof rules
/// (see `banks.rs`), but the running record of "what did we conclude and
/// why" is itself kept append-only per subject so promotions are
/// auditable and demotions are refused, never silent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conclusion {
    pub subject: String,
    pub state: ProofState,
    /// Indices into `FactDb::facts` that justify this conclusion.
    pub justified_by: Vec<usize>,
    /// Human-readable rule name, e.g. "rom_mapping_from_boot_copy".
    pub rule: String,
}

/// The monotonic fact database: an append-only fact log plus a map of
/// current conclusions per subject. Facts are never removed or edited.
/// Conclusions may only move according to [`ProofState::supersedes`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactDb {
    facts: Vec<Fact>,
    conclusions: BTreeMap<String, Conclusion>,
}

/// A malformed conclusion cannot be projected without either dangling or
/// silently changing its evidence indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactProjectionError {
    DanglingJustification {
        subject: String,
        fact_index: usize,
        fact_count: usize,
    },
    UnknownConclusionOwner {
        subject: String,
    },
    ConclusionOwnerMismatch {
        subject: String,
        expected_bank: String,
        fact_index: usize,
        actual_bank: String,
    },
    CanonicalConclusionMismatch {
        subject: String,
        fact_index: usize,
        expected_subject: String,
    },
    MissingCanonicalConclusionClaim {
        subject: String,
    },
}

impl std::fmt::Display for FactProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DanglingJustification {
                subject,
                fact_index,
                fact_count,
            } => write!(
                f,
                "conclusion '{subject}' references missing fact {fact_index} (fact count {fact_count})"
            ),
            Self::UnknownConclusionOwner { subject } => write!(
                f,
                "conclusion '{subject}' has evidence but no typed semantic owner"
            ),
            Self::ConclusionOwnerMismatch {
                subject,
                expected_bank,
                fact_index,
                actual_bank,
            } => write!(
                f,
                "conclusion '{subject}' owns bank '{expected_bank}' but justification fact {fact_index} owns '{actual_bank}'"
            ),
            Self::CanonicalConclusionMismatch {
                subject,
                fact_index,
                expected_subject,
            } => write!(
                f,
                "conclusion '{subject}' does not match canonical subject '{expected_subject}' from justification fact {fact_index}"
            ),
            Self::MissingCanonicalConclusionClaim { subject } => write!(
                f,
                "conclusion '{subject}' has no typed justification of its expected kind"
            ),
        }
    }
}

impl std::error::Error for FactProjectionError {}

#[derive(Debug, Clone, Copy)]
enum OwnedConclusionKind {
    Bank,
    Function,
    ExecutableRange,
    TableEntry,
    ObservedExecuted,
    ObservedIndirect,
}

fn explicit_conclusion_owner(subject: &str) -> Option<(OwnedConclusionKind, &str)> {
    if let Some(bank) = subject.strip_prefix("bank:") {
        return Some((OwnedConclusionKind::Bank, bank));
    }
    let forms = [
        ("fn:", OwnedConclusionKind::Function),
        ("executable-range:", OwnedConclusionKind::ExecutableRange),
        ("table-entry:", OwnedConclusionKind::TableEntry),
        ("observed-executed:", OwnedConclusionKind::ObservedExecuted),
        ("observed-indirect:", OwnedConclusionKind::ObservedIndirect),
    ];
    forms.into_iter().find_map(|(prefix, kind)| {
        subject
            .strip_prefix(prefix)
            .and_then(|rest| rest.split_once(":0x"))
            .map(|(bank, _)| (kind, bank))
    })
}

fn canonical_claim_for_kind(fact: &Fact, kind: OwnedConclusionKind) -> Option<(&str, String)> {
    match (kind, fact) {
        (OwnedConclusionKind::Bank, Fact::RomMapping { bank, .. }) => {
            Some((bank, format!("bank:{bank}")))
        }
        (OwnedConclusionKind::Bank, Fact::EvaluatedImage { bank, .. }) => {
            Some((bank, format!("bank:{bank}")))
        }
        (
            OwnedConclusionKind::Bank,
            Fact::LoadImageTableRecord {
                bank: Some(bank), ..
            },
        ) => Some((bank, format!("bank:{bank}"))),
        (OwnedConclusionKind::Function, Fact::FunctionEntryClaim { target, .. }) => {
            Some((&target.bank, function_entry_subject(target)))
        }
        (
            OwnedConclusionKind::ExecutableRange,
            Fact::ExecutableRange {
                bank,
                va_start,
                va_end,
            },
        ) => Some((bank, executable_range_subject(bank, *va_start, *va_end))),
        (OwnedConclusionKind::TableEntry, Fact::TableEntry { table, index, .. }) => {
            Some((&table.bank, table_entry_subject(table, *index)))
        }
        (OwnedConclusionKind::ObservedExecuted, Fact::ObservedExecutedCode { site, .. }) => Some((
            &site.bank,
            observed_executed_code_subject(&site.bank, site.pc),
        )),
        (
            OwnedConclusionKind::ObservedIndirect,
            Fact::ObservedIndirectTarget { site, target, .. },
        ) => Some((
            &site.bank,
            observed_indirect_target_subject(&site.bank, site.pc, &target.bank, target.pc),
        )),
        _ => None,
    }
}

/// Immutable index for constructing exact bank-local views of a [`FactDb`].
/// Facts with no named bank are global. A cross-bank fact is indexed under
/// every endpoint, while each selected conclusion pulls in its complete
/// justification set and receives densely remapped indices. An owned
/// `Proven` conclusion must exactly equal the canonical subject regenerated
/// from at least one typed justification of its expected kind; bank-only or
/// prefix-only matches are rejected. Non-authoritative states retain their
/// explicit diagnostic scope even when the failed/open rule has no positive
/// typed claim. A call-site bank retains its raw claim, but does not clone the
/// merged conclusion owned by the claim's target bank.
#[derive(Debug)]
pub struct FactProjectionIndex<'a> {
    source: &'a FactDb,
    facts_by_bank: BTreeMap<String, BTreeSet<usize>>,
    global_facts: BTreeSet<usize>,
    conclusions_by_bank: BTreeMap<String, BTreeSet<String>>,
    global_conclusions: BTreeSet<String>,
}

impl<'a> FactProjectionIndex<'a> {
    pub fn new(source: &'a FactDb) -> Result<Self, FactProjectionError> {
        let vrom_banks = source
            .facts
            .iter()
            .filter_map(|fact| {
                let (bank, rom_start, rom_end) = match fact {
                    Fact::RomMapping {
                        bank,
                        rom_space: RomAddressSpace::Virtual,
                        rom_start,
                        rom_end,
                        ..
                    } => (bank, rom_start, rom_end),
                    Fact::EvaluatedImage {
                        bank,
                        receipt:
                            EvaluatedImageReceiptV1 {
                                source:
                                    MaterializedImageSourceV1 {
                                        rom_space: RomAddressSpace::Virtual,
                                        rom_start,
                                        rom_end,
                                        ..
                                    },
                                ..
                            },
                        ..
                    } => (bank, rom_start, rom_end),
                    _ => return None,
                };
                source
                    .conclusion(&format!("bank:{bank}"))
                    .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
                    .then_some((bank.as_str(), *rom_start, *rom_end))
            })
            .collect::<Vec<_>>();
        let mut projectable_global_banks = BTreeMap::<usize, BTreeSet<String>>::new();
        for (index, fact) in source.facts.iter().enumerate() {
            let Fact::LoadImageTableRecord {
                bank: None,
                source_space: MappingAddressSpace::VirtualRom,
                source_start,
                source_end,
                destination_space: MappingAddressSpace::PhysicalRom,
                ..
            } = fact
            else {
                continue;
            };
            let banks = vrom_banks
                .iter()
                .filter(|(_, rom_start, rom_end)| {
                    source_start <= rom_start && rom_end <= source_end
                })
                .map(|(bank, _, _)| (*bank).to_owned())
                .collect();
            projectable_global_banks.insert(index, banks);
        }

        let mut facts_by_bank = BTreeMap::<String, BTreeSet<usize>>::new();
        let mut global_facts = BTreeSet::new();
        for (index, fact) in source.facts.iter().enumerate() {
            if let Some(banks) = projectable_global_banks.get(&index) {
                for bank in banks {
                    facts_by_bank.entry(bank.clone()).or_default().insert(index);
                }
                continue;
            }
            let banks = fact.referenced_banks();
            if banks.is_empty() {
                global_facts.insert(index);
            } else {
                for bank in banks {
                    facts_by_bank
                        .entry(bank.to_owned())
                        .or_default()
                        .insert(index);
                }
            }
        }

        let mut conclusions_by_bank = BTreeMap::<String, BTreeSet<String>>::new();
        let mut global_conclusions = BTreeSet::new();
        for conclusion in source.conclusions.values() {
            let explicit_owner = explicit_conclusion_owner(&conclusion.subject);
            if conclusion.justified_by.is_empty() {
                if conclusion.state == ProofState::Proven && explicit_owner.is_some() {
                    return Err(FactProjectionError::MissingCanonicalConclusionClaim {
                        subject: conclusion.subject.clone(),
                    });
                }
                if let Some((_, owner)) = explicit_owner {
                    conclusions_by_bank
                        .entry(owner.to_owned())
                        .or_default()
                        .insert(conclusion.subject.clone());
                } else {
                    global_conclusions.insert(conclusion.subject.clone());
                }
                continue;
            }
            for &fact_index in &conclusion.justified_by {
                if source.facts.get(fact_index).is_none() {
                    return Err(FactProjectionError::DanglingJustification {
                        subject: conclusion.subject.clone(),
                        fact_index,
                        fact_count: source.facts.len(),
                    });
                }
            }

            if let Some((kind, owner)) = explicit_owner {
                if conclusion.state != ProofState::Proven {
                    conclusions_by_bank
                        .entry(owner.to_owned())
                        .or_default()
                        .insert(conclusion.subject.clone());
                    continue;
                }
                let mut canonical_claims = 0usize;
                for &fact_index in &conclusion.justified_by {
                    if let Some((actual, expected_subject)) =
                        canonical_claim_for_kind(&source.facts[fact_index], kind)
                    {
                        canonical_claims += 1;
                        if actual != owner {
                            return Err(FactProjectionError::ConclusionOwnerMismatch {
                                subject: conclusion.subject.clone(),
                                expected_bank: owner.to_owned(),
                                fact_index,
                                actual_bank: actual.to_owned(),
                            });
                        }
                        if expected_subject != conclusion.subject {
                            return Err(FactProjectionError::CanonicalConclusionMismatch {
                                subject: conclusion.subject.clone(),
                                fact_index,
                                expected_subject,
                            });
                        }
                    }
                }
                if canonical_claims == 0 {
                    return Err(FactProjectionError::MissingCanonicalConclusionClaim {
                        subject: conclusion.subject.clone(),
                    });
                }
                conclusions_by_bank
                    .entry(owner.to_owned())
                    .or_default()
                    .insert(conclusion.subject.clone());
                continue;
            }

            if conclusion.subject.starts_with("load-image-table:") {
                let matching_records = conclusion
                    .justified_by
                    .iter()
                    .copied()
                    .filter(|&index| {
                        matches!(
                            &source.facts[index],
                            Fact::LoadImageTableRecord { table, index, .. }
                                if load_image_table_record_subject(table, *index)
                                    == conclusion.subject
                        )
                    })
                    .collect::<Vec<_>>();
                // A table-level aggregate (or an open record with no typed
                // row) is diagnostic program evidence, not bank authority.
                // Keeping it out of every bank prevents its aggregate
                // justification vector from cloning sibling banks.
                if matching_records.is_empty() {
                    continue;
                }
                let mut banks = BTreeSet::new();
                for fact_index in matching_records {
                    match &source.facts[fact_index] {
                        Fact::LoadImageTableRecord {
                            bank: Some(bank), ..
                        } => {
                            banks.insert(bank.clone());
                        }
                        Fact::LoadImageTableRecord { bank: None, .. } => {
                            if let Some(projected) = projectable_global_banks.get(&fact_index) {
                                banks.extend(projected.iter().cloned());
                            } else {
                                global_conclusions.insert(conclusion.subject.clone());
                            }
                        }
                        _ => unreachable!("matching record predicate admits only records"),
                    }
                }
                for bank in banks {
                    conclusions_by_bank
                        .entry(bank)
                        .or_default()
                        .insert(conclusion.subject.clone());
                }
                continue;
            }

            return Err(FactProjectionError::UnknownConclusionOwner {
                subject: conclusion.subject.clone(),
            });
        }

        Ok(Self {
            source,
            facts_by_bank,
            global_facts,
            conclusions_by_bank,
            global_conclusions,
        })
    }

    pub fn project(&self, bank: &str) -> FactDb {
        let mut fact_indices = self.global_facts.clone();
        if let Some(indices) = self.facts_by_bank.get(bank) {
            fact_indices.extend(indices);
        }
        let mut conclusion_subjects = self.global_conclusions.clone();
        if let Some(subjects) = self.conclusions_by_bank.get(bank) {
            conclusion_subjects.extend(subjects.iter().cloned());
        }
        for subject in &conclusion_subjects {
            fact_indices.extend(&self.source.conclusions[subject].justified_by);
        }

        let mut remap = BTreeMap::new();
        let mut facts = Vec::with_capacity(fact_indices.len());
        for old_index in fact_indices {
            remap.insert(old_index, facts.len());
            facts.push(self.source.facts[old_index].clone());
        }
        let conclusions = conclusion_subjects
            .into_iter()
            .map(|subject| {
                let mut conclusion = self.source.conclusions[&subject].clone();
                conclusion.justified_by = conclusion
                    .justified_by
                    .iter()
                    .map(|index| remap[index])
                    .collect();
                (subject, conclusion)
            })
            .collect();
        FactDb { facts, conclusions }
    }

    pub fn global_fact_count(&self) -> usize {
        self.global_facts.len()
    }

    pub fn scoped_fact_count(&self, bank: &str) -> usize {
        self.facts_by_bank.get(bank).map_or(0, BTreeSet::len)
    }

    pub fn selected_conclusion_count(&self, bank: &str) -> usize {
        self.global_conclusions.len() + self.conclusions_by_bank.get(bank).map_or(0, BTreeSet::len)
    }

    pub fn largest_selected_justifications(
        &self,
        bank: &str,
        limit: usize,
    ) -> Vec<(String, usize)> {
        let mut subjects = self.global_conclusions.clone();
        if let Some(scoped) = self.conclusions_by_bank.get(bank) {
            subjects.extend(scoped.iter().cloned());
        }
        let mut sizes = subjects
            .into_iter()
            .map(|subject| {
                let size = self.source.conclusions[&subject].justified_by.len();
                (subject, size)
            })
            .collect::<Vec<_>>();
        sizes.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        sizes.truncate(limit);
        sizes
    }
}

/// Error returned when a proof-rule update would silently overwrite a
/// `Proven` conclusion with something weaker. Callers must either accept
/// this refusal (log it as a conflict-worthy anomaly) or add new evidence
/// strong enough to justify `Proven` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonotonicityViolation {
    pub subject: String,
    pub existing: ProofState,
    pub attempted: ProofState,
}

impl std::fmt::Display for MonotonicityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to downgrade proven fact for '{}' ({:?} -> {:?})",
            self.subject, self.existing, self.attempted
        )
    }
}

impl std::error::Error for MonotonicityViolation {}

impl FactDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a fact and return its stable index (used by conclusions'
    /// `justified_by`). Facts are never deduplicated destructively here --
    /// exact-duplicate facts are harmless and identical provenance from two
    /// independent detectors is itself useful corroborating evidence, not
    /// noise to be silently dropped.
    pub fn insert(&mut self, fact: Fact) -> usize {
        self.facts.push(fact);
        self.facts.len() - 1
    }

    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    /// Record or update a conclusion for `subject`, enforcing monotonicity.
    /// Returns the previous conclusion's state (if any) on success.
    pub fn conclude(
        &mut self,
        subject: impl Into<String>,
        state: ProofState,
        justified_by: Vec<usize>,
        rule: impl Into<String>,
    ) -> Result<Option<ProofState>, MonotonicityViolation> {
        let subject = subject.into();
        let previous = self.conclusions.get(&subject).map(|c| c.state);
        if let Some(prev) = previous {
            if !prev.supersedes(state) {
                return Err(MonotonicityViolation {
                    subject,
                    existing: prev,
                    attempted: state,
                });
            }
        }
        self.conclusions.insert(
            subject.clone(),
            Conclusion {
                subject,
                state,
                justified_by,
                rule: rule.into(),
            },
        );
        Ok(previous)
    }

    pub fn conclusion(&self, subject: &str) -> Option<&Conclusion> {
        self.conclusions.get(subject)
    }

    pub fn conclusions(&self) -> impl Iterator<Item = &Conclusion> {
        self.conclusions.values()
    }

    /// All `RomMapping` facts cited by their bank's `Proven` conclusion. A
    /// same-bank candidate fact that is absent from `justified_by` does not
    /// inherit the bank subject's authority.
    pub fn proven_rom_mappings(&self) -> Vec<&Fact> {
        self.facts
            .iter()
            .enumerate()
            .filter_map(|(index, fact)| {
                let Fact::RomMapping { bank, .. } = fact else {
                    return None;
                };
                self.proven_bank_conclusion_cites(bank, index)
                    .then_some(fact)
            })
            .collect()
    }

    /// Every accepted bank image, preserving the structural distinction
    /// between affine ROM bytes and evaluator-produced output. Candidate and
    /// Supported conclusions never enter this result.
    pub fn proven_bank_images(&self) -> Vec<ProvenBankImageV1> {
        self.facts
            .iter()
            .enumerate()
            .filter_map(|(index, fact)| match fact {
                Fact::RomMapping {
                    bank,
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    va_end,
                } if self.proven_bank_conclusion_cites(bank, index) => Some(ProvenBankImageV1 {
                    bank: bank.clone(),
                    va_start: *va_start,
                    va_end: *va_end,
                    backing: BankBackingV1::RomAffine {
                        rom_space: *rom_space,
                        rom_start: *rom_start,
                        rom_end: *rom_end,
                    },
                }),
                Fact::EvaluatedImage {
                    bank,
                    va_start,
                    va_end,
                    receipt,
                } if self.proven_bank_conclusion_cites(bank, index) => Some(ProvenBankImageV1 {
                    bank: bank.clone(),
                    va_start: *va_start,
                    va_end: *va_end,
                    backing: BankBackingV1::Materialized {
                        receipt_sha256: evaluated_image_receipt_sha256_v1(receipt),
                        output_len: receipt.output_len,
                    },
                }),
                _ => None,
            })
            .collect()
    }

    fn proven_bank_conclusion_cites(&self, bank: &str, fact_index: usize) -> bool {
        self.conclusion(&format!("bank:{bank}"))
            .is_some_and(|conclusion| {
                conclusion.state == ProofState::Proven
                    && conclusion.justified_by.contains(&fact_index)
            })
    }

    /// Resolve `[va_start, va_end)` to a typed subspan of the bank's complete
    /// proven image. Exact duplicate image facts collapse, but distinct
    /// complete images remain an ambiguity even when only one covers the
    /// requested interval. Affine mappings expose only their ROM-backed
    /// prefix; a larger VA interval may include BSS and does not manufacture
    /// cartridge coordinates for it.
    pub fn resolve_proven_bank_backing_span(
        &self,
        bank: &str,
        va_start: u32,
        va_end: u32,
    ) -> BankBackingSpanResolutionV1 {
        if va_start >= va_end {
            return BankBackingSpanResolutionV1::InvalidGeometry;
        }

        let images = self
            .proven_bank_images()
            .into_iter()
            .filter(|image| image.bank == bank)
            .collect::<BTreeSet<_>>();
        let mut images = images.into_iter();
        let Some(image) = images.next() else {
            return BankBackingSpanResolutionV1::Missing;
        };
        if images.next().is_some() {
            return BankBackingSpanResolutionV1::Ambiguous;
        }

        let Some(va_len) = image.va_end.checked_sub(image.va_start) else {
            return BankBackingSpanResolutionV1::InvalidGeometry;
        };
        if va_len == 0 {
            return BankBackingSpanResolutionV1::InvalidGeometry;
        }

        match image.backing {
            BankBackingV1::RomAffine {
                rom_space,
                rom_start,
                rom_end,
            } => {
                let Some(rom_len) = rom_end.checked_sub(rom_start) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                if rom_len == 0 || rom_len > va_len {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                }
                let Some(backed_va_end) = image.va_start.checked_add(rom_len) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                if va_start < image.va_start || va_end > backed_va_end {
                    return BankBackingSpanResolutionV1::Missing;
                }
                let Some(start_delta) = va_start.checked_sub(image.va_start) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                let Some(end_delta) = va_end.checked_sub(image.va_start) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                let Some(span_rom_start) = rom_start.checked_add(start_delta) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                let Some(span_rom_end) = rom_start.checked_add(end_delta) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::RomAffine {
                    rom_space,
                    rom_start: span_rom_start,
                    rom_end: span_rom_end,
                })
            }
            BankBackingV1::Materialized {
                receipt_sha256,
                output_len,
            } => {
                if output_len == 0 || output_len != va_len {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                }
                if va_start < image.va_start || va_end > image.va_end {
                    return BankBackingSpanResolutionV1::Missing;
                }
                let Some(output_start) = va_start.checked_sub(image.va_start) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                let Some(output_end) = va_end.checked_sub(image.va_start) else {
                    return BankBackingSpanResolutionV1::InvalidGeometry;
                };
                BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::Materialized {
                    receipt_sha256,
                    output_start,
                    output_end,
                })
            }
        }
    }

    /// Proven executable intervals for `bank`, sorted by address. An empty
    /// result means no provider has separated text from the load image yet;
    /// callers may retain their legacy whole-image behavior but must not
    /// represent that fallback as proven executable coverage.
    pub fn proven_executable_ranges(&self, bank: &str) -> Vec<(u32, u32)> {
        let mut ranges: Vec<_> = self
            .facts
            .iter()
            .filter_map(|fact| {
                let Fact::ExecutableRange {
                    bank: fact_bank,
                    va_start,
                    va_end,
                } = fact
                else {
                    return None;
                };
                (fact_bank == bank
                    && self
                        .conclusion(&executable_range_subject(fact_bank, *va_start, *va_end))
                        .is_some_and(|conclusion| conclusion.state == ProofState::Proven))
                .then_some((*va_start, *va_end))
            })
            .collect();
        ranges.sort_unstable();
        ranges
    }

    /// Proven VROM-to-physical-ROM file mappings supplied by Phase 2. These
    /// are the byte-backing evidence used to materialize VROM load images;
    /// rejected/conflicting records cannot leak into detector input.
    pub fn proven_vrom_file_mappings(&self) -> Vec<(usize, &Fact)> {
        self.facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| {
                matches!(
                    fact,
                    Fact::LoadImageTableRecord {
                        table,
                        index,
                        source_space: MappingAddressSpace::VirtualRom,
                        destination_space: MappingAddressSpace::PhysicalRom,
                        ..
                    } if self
                        .conclusion(&load_image_table_record_subject(table, *index))
                        .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
                )
            })
            .collect()
    }

    /// Record an entry exposed by an already-identified table or vector.
    /// The caller supplies the table evidence's proof state; Phase 3 carries
    /// it forward and validates the target against discovered load images.
    pub fn record_table_entry(
        &mut self,
        table: BankAddr,
        index: u32,
        target: BankAddr,
        state: ProofState,
        rule: impl Into<String>,
    ) -> Result<usize, MonotonicityViolation> {
        let subject = table_entry_subject(&table, index);
        let fact = self.insert(Fact::TableEntry {
            table,
            index,
            target,
        });
        self.conclude(subject, state, vec![fact], rule)?;
        Ok(fact)
    }

    /// Function entries whose merged Phase 3 conclusion is authoritative.
    /// Candidate/supported/conflict entries remain visible in the database
    /// but cannot become CFG roots through this API.
    pub fn proven_function_entries(&self, bank: &str) -> Vec<u32> {
        let mut entries = BTreeSet::new();
        for fact in &self.facts {
            let Fact::FunctionEntryClaim { target, .. } = fact else {
                continue;
            };
            if target.bank == bank
                && self
                    .conclusion(&function_entry_subject(target))
                    .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
            {
                entries.insert(target.pc);
            }
        }
        entries.into_iter().collect()
    }

    /// Hardware entrypoints whose own typed claim is already proven.
    ///
    /// This is intentionally narrower than [`Self::proven_function_entries`]:
    /// consumers that inductively derive new callable authority must not let a
    /// traversal seed, table claim, or independently merged heuristic become
    /// the root of that proof chain.
    pub fn proven_hardware_function_entries(&self, bank: &str) -> Vec<u32> {
        let mut entries = BTreeSet::new();
        for fact in &self.facts {
            let Fact::FunctionEntryClaim {
                target,
                detector: CandidateDetector::HardwareEntrypoint,
                evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
                proposed_state: ProofState::Proven,
            } = fact
            else {
                continue;
            };
            if target.bank == bank
                && self
                    .conclusion(&function_entry_subject(target))
                    .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
            {
                entries.insert(target.pc);
            }
        }
        entries.into_iter().collect()
    }

    /// Function-entry candidates suitable only for exploratory CFG coverage.
    /// This deliberately includes non-authoritative claims but never changes
    /// their proof state; callers must keep resulting owners/candidates out of
    /// exact-proof admission.
    pub fn candidate_function_entries(&self, bank: &str) -> Vec<u32> {
        let mut entries = BTreeSet::new();
        for fact in &self.facts {
            let Fact::FunctionEntryClaim { target, .. } = fact else {
                continue;
            };
            if target.bank != bank {
                continue;
            }
            let state = self
                .conclusion(&function_entry_subject(target))
                .map(|conclusion| conclusion.state);
            if matches!(
                state,
                Some(ProofState::Candidate | ProofState::Supported | ProofState::Proven)
            ) {
                entries.insert(target.pc);
            }
        }
        entries.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluated_receipt() -> EvaluatedImageReceiptV1 {
        EvaluatedImageReceiptV1 {
            evaluator: MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            source: MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x2000,
                rom_end: 0x2040,
                cursor: 4,
            },
            source_sha256: "11".repeat(32),
            output_len: 8,
            output_sha256: "22".repeat(32),
            streams: vec![MaterializedImageStreamV1 {
                source_range: MaterializedByteRangeV1 { start: 4, end: 32 },
                encoded_range: MaterializedByteRangeV1 { start: 10, end: 32 },
                output_range: MaterializedByteRangeV1 { start: 0, end: 8 },
                declared_output_len: 8,
                source_sha256: "33".repeat(32),
                output_sha256: "22".repeat(32),
            }],
            trailing_suffix: MaterializedImageSuffixV1 {
                offset: 32,
                len: 32,
                sha256: "44".repeat(32),
            },
        }
    }

    fn prove_bank(db: &mut FactDb, bank: &str, facts: Vec<usize>) {
        db.conclude(format!("bank:{bank}"), ProofState::Proven, facts, "test")
            .unwrap();
    }

    fn banks(fact: &Fact) -> Vec<String> {
        fact.referenced_banks()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn indirect_target_domain_projection_is_guard_bounded_and_fail_closed() {
        let analysis = |state, kind, targets| Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", 0x8000_0040),
            via_call: false,
            state,
            kind,
            targets,
            memory_sources: vec![0x8000_1000],
        };

        let domain = analysis(
            IndirectTransferState::Bounded,
            Some(IndirectTransferKind::JumpTable),
            vec![0x8000_0090, 0x8000_0080, 0x8000_0090],
        )
        .indirect_target_domain_v1()
        .expect("guard-bounded jump table has an exclusion-only domain");
        assert_eq!(
            domain.basis,
            IndirectTargetDomainBasisV1::GuardBoundedFinite
        );
        assert_eq!(domain.targets, vec![0x8000_0080, 0x8000_0090]);

        for fact in [
            analysis(
                IndirectTransferState::Exhaustive,
                Some(IndirectTransferKind::JumpTable),
                vec![0x8000_0080],
            ),
            analysis(
                IndirectTransferState::Open,
                Some(IndirectTransferKind::JumpTable),
                vec![0x8000_0080],
            ),
            analysis(
                IndirectTransferState::Bounded,
                Some(IndirectTransferKind::MemoryValueSet),
                vec![0x8000_0080],
            ),
            analysis(
                IndirectTransferState::Bounded,
                Some(IndirectTransferKind::JumpTable),
                vec![],
            ),
        ] {
            assert!(fact.indirect_target_domain_v1().is_none());
        }
    }

    #[test]
    fn referenced_banks_covers_edges_globals_and_nested_entry_evidence() {
        let a = BankAddr::new("a", 0x8000_0000);
        let b = BankAddr::new("b", 0x8010_0000);
        let c = BankAddr::new("c", 0x8020_0000);
        assert_eq!(
            banks(&Fact::DirectCall {
                source: a.clone(),
                target: b.clone()
            }),
            vec!["a", "b"]
        );
        assert_eq!(
            banks(&Fact::ResolvedCall {
                source: a.clone(),
                target: b.clone()
            }),
            vec!["a", "b"]
        );
        assert_eq!(
            banks(&Fact::ObservedIndirectTarget {
                site: a.clone(),
                target: b.clone(),
                trace: "t".into()
            }),
            vec!["a", "b"]
        );
        assert_eq!(
            banks(&Fact::TableEntry {
                table: a.clone(),
                index: 0,
                target: b.clone()
            }),
            vec!["a", "b"]
        );
        assert!(banks(&Fact::LoadImageTableRecord {
            table: "dma".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: MappingAddressSpace::PhysicalRom,
            source_start: 0,
            source_end: 4,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 4,
            destination_end: 8,
        })
        .is_empty());
        let evaluated = Fact::EvaluatedImage {
            bank: "image".into(),
            va_start: 0x8020_0000,
            va_end: 0x8020_0008,
            receipt: evaluated_receipt(),
        };
        assert_eq!(evaluated.primary_bank(), Some("image"));
        assert_eq!(banks(&evaluated), vec!["image"]);

        let nested = [
            FunctionEntryEvidence::DirectJal {
                call_site: a.clone(),
            },
            FunctionEntryEvidence::ResolvedJalr {
                call_site: a.clone(),
                construction_start: c.clone(),
            },
            FunctionEntryEvidence::ExhaustiveIndirectCall {
                call_site: a.clone(),
                kind: IndirectCallEvidenceKind::MemoryValueSet,
                memory_sources: vec![c.clone()],
            },
            FunctionEntryEvidence::SemanticCallableArgument {
                call_site: a.clone(),
                callee: c.clone(),
                pointer_register: 5,
                contract: SemanticCallableContract::CallbackRegistry {
                    dispatcher: c.clone(),
                    callback_store_site: a.clone(),
                    list_insert_site: c.clone(),
                    jalr_site: c.clone(),
                },
            },
            FunctionEntryEvidence::Prologue {
                stack_adjust: a.clone(),
                frame_size: 16,
                pattern: ProloguePattern::SavesReturnAddress,
                corroborating_site: c.clone(),
            },
            FunctionEntryEvidence::ArgumentHomeSpillLeaf {
                predecessor_return: a.clone(),
                spill_site: b.clone(),
                argument_index: 0,
                return_site: c.clone(),
            },
            FunctionEntryEvidence::TableEntry {
                table: c.clone(),
                index: 1,
            },
        ];
        for evidence in nested {
            let referenced = banks(&Fact::FunctionEntryClaim {
                target: b.clone(),
                detector: CandidateDetector::TableDerived,
                evidence,
                proposed_state: ProofState::Supported,
            });
            assert!(referenced.iter().any(|bank| bank == "b"));
            assert!(referenced.len() >= 2);
        }

        // Single-bank variants exercise every remaining top-level arm.
        let single_bank = [
            Fact::BlockStart {
                bank: "a".into(),
                pc: a.pc,
            },
            Fact::LoadsFrom {
                site: a.clone(),
                address: 0,
            },
            Fact::ObservedExecutedCode {
                site: a.clone(),
                trace: "t".into(),
                sequence: 0,
            },
            Fact::IndirectTransferAnalysis {
                site: a.clone(),
                via_call: false,
                state: IndirectTransferState::Open,
                kind: None,
                targets: vec![],
                memory_sources: vec![],
            },
            Fact::RomMapping {
                bank: "a".into(),
                rom_space: RomAddressSpace::Physical,
                rom_start: 0,
                rom_end: 4,
                va_start: a.pc,
                va_end: a.pc + 4,
            },
            Fact::ExecutableRange {
                bank: "a".into(),
                va_start: a.pc,
                va_end: a.pc + 4,
            },
            Fact::Evidence {
                subject: a.clone(),
                note: "e".into(),
            },
        ];
        for fact in &single_bank {
            assert_eq!(banks(fact), vec!["a"]);
        }
        assert_eq!(
            banks(&Fact::LoadImageTableRecord {
                table: "dma".into(),
                bank: Some("a".into()),
                table_space: RomAddressSpace::Physical,
                table_offset: 0,
                index: 0,
                source_space: MappingAddressSpace::PhysicalRom,
                source_start: 0,
                source_end: 4,
                destination_space: MappingAddressSpace::Vram,
                destination_start: a.pc,
                destination_end: a.pc + 4,
            }),
            vec!["a"]
        );
        assert_eq!(
            banks(&Fact::FunctionEntryClaim {
                target: b,
                detector: CandidateDetector::HardwareEntrypoint,
                evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
                proposed_state: ProofState::Proven,
            }),
            vec!["b"]
        );
    }

    #[test]
    fn projection_preserves_cross_bank_edges_globals_and_conclusion_indices() {
        let mut db = FactDb::new();
        let edge = db.insert(Fact::DirectCall {
            source: BankAddr::new("source", 0x8000_0000),
            target: BankAddr::new("target", 0x8010_0000),
        });
        let target_detail = db.insert(Fact::BlockStart {
            bank: "target".into(),
            pc: 0x8010_0000,
        });
        db.insert(Fact::BlockStart {
            bank: "irrelevant".into(),
            pc: 0x8020_0000,
        });
        let global = db.insert(Fact::LoadImageTableRecord {
            table: "dma".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: MappingAddressSpace::PhysicalRom,
            source_start: 0,
            source_end: 4,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 4,
            destination_end: 8,
        });
        let _ = (edge, target_detail);
        db.conclude("global", ProofState::Open, vec![], "unscoped")
            .unwrap();
        db.conclude(
            load_image_table_record_subject("dma", 0),
            ProofState::Proven,
            vec![global, global],
            "dma",
        )
        .unwrap();

        let index = FactProjectionIndex::new(&db).unwrap();
        for bank in ["source", "target"] {
            let projected = index.project(bank);
            assert!(projected
                .facts()
                .iter()
                .any(|fact| matches!(fact, Fact::DirectCall { .. })));
            assert!(projected
                .facts()
                .iter()
                .any(|fact| matches!(fact, Fact::LoadImageTableRecord { bank: None, .. })));
            assert!(!projected
                .facts()
                .iter()
                .any(|fact| matches!(fact, Fact::BlockStart { bank, .. } if bank == "irrelevant")));
            for conclusion in projected.conclusions() {
                assert!(conclusion
                    .justified_by
                    .iter()
                    .all(|fact| *fact < projected.facts().len()));
            }
            assert!(projected.conclusion("global").is_some());
            let duplicate = &projected
                .conclusion(&load_image_table_record_subject("dma", 0))
                .unwrap()
                .justified_by;
            assert_eq!(duplicate.len(), 2);
            assert_eq!(duplicate[0], duplicate[1]);
        }
    }

    #[test]
    fn projection_keeps_foreign_claim_but_scopes_merged_conclusion_to_target() {
        let source = BankAddr::new("source", 0x8000_0000);
        let target = BankAddr::new("target", 0x8010_0000);
        let mut db = FactDb::new();
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal { call_site: source },
            proposed_state: ProofState::Proven,
        });
        let subject = function_entry_subject(&target);
        db.conclude(&subject, ProofState::Proven, vec![claim], "direct_jal")
            .unwrap();

        let index = FactProjectionIndex::new(&db).unwrap();
        let source_projection = index.project("source");
        assert_eq!(source_projection.facts().len(), 1);
        assert!(source_projection.conclusion(&subject).is_none());

        let target_projection = index.project("target");
        assert_eq!(target_projection.facts().len(), 1);
        assert_eq!(
            target_projection.conclusion(&subject).unwrap().justified_by,
            vec![0]
        );
    }

    #[test]
    fn projection_assigns_bankless_file_records_only_to_backed_vrom_banks() {
        let mut db = FactDb::new();
        let mapping = db.insert(Fact::RomMapping {
            bank: "overlay".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1100,
            rom_end: 0x1200,
            va_start: 0x8010_0000,
            va_end: 0x8010_0100,
        });
        db.conclude("bank:overlay", ProofState::Proven, vec![mapping], "mapping")
            .unwrap();
        let shared_mapping = db.insert(Fact::RomMapping {
            bank: "shared_overlay".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1180,
            rom_end: 0x11c0,
            va_start: 0x8020_0000,
            va_end: 0x8020_0040,
        });
        db.conclude(
            "bank:shared_overlay",
            ProofState::Proven,
            vec![shared_mapping],
            "mapping",
        )
        .unwrap();
        let candidate_mapping = db.insert(Fact::RomMapping {
            bank: "candidate_overlay".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1100,
            rom_end: 0x1200,
            va_start: 0x8030_0000,
            va_end: 0x8030_0100,
        });
        db.conclude(
            "bank:candidate_overlay",
            ProofState::Candidate,
            vec![candidate_mapping],
            "candidate",
        )
        .unwrap();
        let covering = db.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x100,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: 0x1000,
            source_end: 0x1300,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x2000,
            destination_end: 0x2300,
        });
        let covering_subject = load_image_table_record_subject("files", 0);
        db.conclude(
            &covering_subject,
            ProofState::Proven,
            vec![covering],
            "file",
        )
        .unwrap();
        let unrelated = db.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x100,
            index: 1,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: 0x3000,
            source_end: 0x3100,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x4000,
            destination_end: 0x4100,
        });
        let unrelated_subject = load_image_table_record_subject("files", 1);
        db.conclude(
            &unrelated_subject,
            ProofState::Proven,
            vec![unrelated],
            "file",
        )
        .unwrap();
        let truly_global = db.insert(Fact::LoadImageTableRecord {
            table: "global".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x200,
            index: 0,
            source_space: MappingAddressSpace::PhysicalRom,
            source_start: 0x5000,
            source_end: 0x5100,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x6000,
            destination_end: 0x6100,
        });
        let global_subject = load_image_table_record_subject("global", 0);
        db.conclude(
            &global_subject,
            ProofState::Open,
            vec![truly_global],
            "global",
        )
        .unwrap();

        let index = FactProjectionIndex::new(&db).unwrap();
        let overlay = index.project("overlay");
        assert!(overlay.conclusion(&covering_subject).is_some());
        assert!(overlay.conclusion(&unrelated_subject).is_none());
        assert!(overlay.conclusion(&global_subject).is_some());
        assert!(overlay.facts().iter().any(|fact| matches!(
            fact,
            Fact::LoadImageTableRecord { table, index: 0, .. } if table == "files"
        )));
        assert!(!overlay.facts().iter().any(|fact| matches!(
            fact,
            Fact::LoadImageTableRecord { table, index: 1, .. } if table == "files"
        )));

        let physical = index.project("physical");
        assert!(physical.conclusion(&covering_subject).is_none());
        assert!(physical.conclusion(&unrelated_subject).is_none());
        assert!(physical.conclusion(&global_subject).is_some());
        assert!(index
            .project("shared_overlay")
            .conclusion(&covering_subject)
            .is_some());
        assert!(index
            .project("candidate_overlay")
            .conclusion(&covering_subject)
            .is_none());
    }

    #[test]
    fn projection_routes_virtual_source_backing_to_proven_evaluated_image() {
        let mut db = FactDb::new();
        let mut receipt = evaluated_receipt();
        receipt.source.rom_space = RomAddressSpace::Virtual;
        receipt.source.rom_start = 0x7100;
        receipt.source.rom_end = 0x7180;
        let evaluated = db.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: 0x8040_0000,
            va_end: 0x8040_0008,
            receipt: receipt.clone(),
        });
        db.conclude(
            "bank:materialized",
            ProofState::Proven,
            vec![evaluated],
            "test full proof",
        )
        .unwrap();
        let candidate = db.insert(Fact::EvaluatedImage {
            bank: "candidate_materialized".into(),
            va_start: 0x8050_0000,
            va_end: 0x8050_0008,
            receipt,
        });
        db.conclude(
            "bank:candidate_materialized",
            ProofState::Supported,
            vec![candidate],
            "candidate evaluation only",
        )
        .unwrap();
        let backing = db.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x100,
            index: 7,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: 0x7000,
            source_end: 0x7200,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x9000,
            destination_end: 0x9200,
        });
        let backing_subject = load_image_table_record_subject("files", 7);
        db.conclude(&backing_subject, ProofState::Proven, vec![backing], "file")
            .unwrap();

        let index = FactProjectionIndex::new(&db).unwrap();
        let projected = index.project("materialized");
        assert!(projected.conclusion(&backing_subject).is_some());
        assert!(projected.facts().iter().any(|fact| matches!(
            fact,
            Fact::LoadImageTableRecord { table, index: 7, .. } if table == "files"
        )));
        assert!(index
            .project("candidate_materialized")
            .conclusion(&backing_subject)
            .is_none());
    }

    #[test]
    fn projection_rejects_mismatched_and_unknown_owned_conclusions() {
        let target = BankAddr::new("actual", 0x8010_0000);
        let mut mismatched = FactDb::new();
        let claim = mismatched.insert(Fact::FunctionEntryClaim {
            target,
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("source", 0x8000_0000),
            },
            proposed_state: ProofState::Proven,
        });
        mismatched
            .conclude(
                function_entry_subject(&BankAddr::new("claimed", 0x8010_0000)),
                ProofState::Proven,
                vec![claim],
                "corrupt",
            )
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&mismatched),
            Err(FactProjectionError::ConclusionOwnerMismatch { .. })
        ));

        let mut wrong_address = FactDb::new();
        let target = BankAddr::new("bank", 0x8010_0000);
        let claim = wrong_address.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("source", 0x8000_0000),
            },
            proposed_state: ProofState::Proven,
        });
        wrong_address
            .conclude(
                function_entry_subject(&BankAddr::new("bank", target.pc + 4)),
                ProofState::Proven,
                vec![claim],
                "wrong address",
            )
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&wrong_address),
            Err(FactProjectionError::CanonicalConclusionMismatch { .. })
        ));

        let mut malformed = FactDb::new();
        let claim = malformed.insert(Fact::FunctionEntryClaim {
            target,
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("source", 0x8000_0000),
            },
            proposed_state: ProofState::Proven,
        });
        malformed
            .conclude(
                "fn:bank:0xnot-an-address",
                ProofState::Proven,
                vec![claim],
                "malformed",
            )
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&malformed),
            Err(FactProjectionError::CanonicalConclusionMismatch { .. })
        ));

        let mut unrelated_only = FactDb::new();
        let unrelated = unrelated_only.insert(Fact::BlockStart {
            bank: "bank".into(),
            pc: 0x8010_0000,
        });
        unrelated_only
            .conclude(
                "fn:bank:0x80100000",
                ProofState::Proven,
                vec![unrelated],
                "unrelated",
            )
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&unrelated_only),
            Err(FactProjectionError::MissingCanonicalConclusionClaim { .. })
        ));

        let mut unknown = FactDb::new();
        let fact = unknown.insert(Fact::BlockStart {
            bank: "bank".into(),
            pc: 0x8000_0000,
        });
        unknown
            .conclude("caller-authored", ProofState::Proven, vec![fact], "unknown")
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&unknown),
            Err(FactProjectionError::UnknownConclusionOwner { .. })
        ));
    }

    #[test]
    fn projection_rejects_proven_owned_empty_and_scopes_open_diagnostics() {
        let mut owned = FactDb::new();
        owned
            .conclude(
                "fn:bank:0x80100000",
                ProofState::Proven,
                vec![],
                "missing claim",
            )
            .unwrap();
        assert!(matches!(
            FactProjectionIndex::new(&owned),
            Err(FactProjectionError::MissingCanonicalConclusionClaim { .. })
        ));

        let mut open_owned = FactDb::new();
        open_owned
            .conclude("bank:a", ProofState::Open, vec![], "no record")
            .unwrap();
        let index = FactProjectionIndex::new(&open_owned).unwrap();
        assert!(index.project("a").conclusion("bank:a").is_some());
        assert!(index.project("b").conclusion("bank:a").is_none());

        let mut unscoped = FactDb::new();
        unscoped
            .conclude("analysis-frontier", ProofState::Open, vec![], "open")
            .unwrap();
        let index = FactProjectionIndex::new(&unscoped).unwrap();
        for bank in ["a", "b"] {
            assert!(index
                .project(bank)
                .conclusion("analysis-frontier")
                .is_some());
        }
    }

    #[test]
    fn projection_semantics_match_unprojected_bank_without_aggregate_siblings() {
        let mut db = FactDb::new();
        let mapping_a = db.insert(Fact::RomMapping {
            bank: "a".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1000,
            rom_end: 0x1100,
            va_start: 0x8010_0000,
            va_end: 0x8010_0100,
        });
        db.conclude("bank:a", ProofState::Proven, vec![mapping_a], "mapping")
            .unwrap();
        let mapping_b = db.insert(Fact::RomMapping {
            bank: "b".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1080,
            rom_end: 0x1100,
            va_start: 0x8020_0000,
            va_end: 0x8020_0080,
        });
        db.conclude("bank:b", ProofState::Proven, vec![mapping_b], "mapping")
            .unwrap();
        let executable = db.insert(Fact::ExecutableRange {
            bank: "a".into(),
            va_start: 0x8010_0000,
            va_end: 0x8010_0100,
        });
        db.conclude(
            executable_range_subject("a", 0x8010_0000, 0x8010_0100),
            ProofState::Proven,
            vec![executable],
            "executable",
        )
        .unwrap();
        let entry = BankAddr::new("a", 0x8010_0000);
        let claim = db.insert(Fact::FunctionEntryClaim {
            target: entry.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&entry),
            ProofState::Proven,
            vec![claim, claim],
            "entry",
        )
        .unwrap();
        let shared_file = db.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: 0x1000,
            source_end: 0x1100,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: 0x2000,
            destination_end: 0x2100,
        });
        let shared_subject = load_image_table_record_subject("files", 0);
        db.conclude(
            &shared_subject,
            ProofState::Proven,
            vec![shared_file],
            "file",
        )
        .unwrap();
        db.conclude(
            "load-image-table:files",
            ProofState::Proven,
            vec![mapping_a, mapping_b, shared_file],
            "aggregate",
        )
        .unwrap();
        db.conclude("zero", ProofState::Open, vec![], "unscoped")
            .unwrap();

        let projected = FactProjectionIndex::new(&db).unwrap().project("a");
        let unprojected_mappings = db
            .proven_rom_mappings()
            .into_iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping { bank, .. } if bank == "a" => Some(bank.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let projected_mappings = projected
            .proven_rom_mappings()
            .into_iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping { bank, .. } => Some(bank.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(projected_mappings, unprojected_mappings);
        assert_eq!(
            projected.proven_executable_ranges("a"),
            db.proven_executable_ranges("a")
        );
        assert_eq!(
            projected.proven_function_entries("a"),
            db.proven_function_entries("a")
        );
        assert_eq!(
            projected.proven_hardware_function_entries("a"),
            db.proven_hardware_function_entries("a")
        );
        assert!(projected.conclusion(&shared_subject).is_some());
        assert!(projected.conclusion("load-image-table:files").is_none());
        assert!(projected.conclusion("zero").is_some());
        assert_eq!(
            projected
                .conclusion(&function_entry_subject(&entry))
                .unwrap()
                .justified_by,
            vec![2, 2]
        );
        assert!(!projected
            .facts()
            .iter()
            .any(|fact| matches!(fact, Fact::RomMapping { bank, .. } if bank == "b")));
    }

    #[test]
    fn projection_rejects_dangling_justification() {
        let mut db = FactDb::new();
        db.conclude("bad", ProofState::Open, vec![9], "bad")
            .unwrap();
        assert_eq!(
            FactProjectionIndex::new(&db).unwrap_err(),
            FactProjectionError::DanglingJustification {
                subject: "bad".into(),
                fact_index: 9,
                fact_count: 0,
            }
        );
    }

    #[test]
    fn insert_is_append_only_and_indices_are_stable() {
        let mut db = FactDb::new();
        let i0 = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        let i1 = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0410,
        });
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);
        assert_eq!(db.facts().len(), 2);
    }

    #[test]
    fn conclude_allows_strengthening_candidate_to_proven() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Candidate,
            vec![f],
            "heuristic",
        )
        .unwrap();
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Proven,
            vec![f],
            "direct_call_closure",
        )
        .unwrap();
        assert_eq!(
            db.conclusion("fn:boot:0x80000400").unwrap().state,
            ProofState::Proven
        );
    }

    #[test]
    fn conclude_refuses_to_downgrade_a_proven_fact() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude(
            "fn:boot:0x80000400",
            ProofState::Proven,
            vec![f],
            "direct_call_closure",
        )
        .unwrap();
        let err = db
            .conclude(
                "fn:boot:0x80000400",
                ProofState::Candidate,
                vec![f],
                "later_heuristic",
            )
            .unwrap_err();
        assert_eq!(err.existing, ProofState::Proven);
        assert_eq!(err.attempted, ProofState::Candidate);
        // The conclusion must be unchanged after the refused attempt.
        assert_eq!(
            db.conclusion("fn:boot:0x80000400").unwrap().state,
            ProofState::Proven
        );
    }

    #[test]
    fn conclude_surfaces_a_conflict_with_proven_evidence() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Proven, vec![f], "proof")
            .unwrap();
        db.conclude("x", ProofState::Conflict, vec![f], "contradiction")
            .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Conflict);
    }

    #[test]
    fn table_entries_and_proven_function_roots_remain_bank_qualified() {
        let mut db = FactDb::new();
        let table = BankAddr::new("boot", 0x8000_1000);
        let target = BankAddr::new("overlay", 0x8010_0040);
        db.record_table_entry(
            table.clone(),
            3,
            target.clone(),
            ProofState::Proven,
            "proven_vector",
        )
        .unwrap();
        assert_eq!(
            db.conclusion(&table_entry_subject(&table, 3))
                .unwrap()
                .state,
            ProofState::Proven
        );

        let claim = db.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::TableDerived,
            evidence: FunctionEntryEvidence::TableEntry { table, index: 3 },
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&target),
            ProofState::Proven,
            vec![claim],
            "table_entry_merge",
        )
        .unwrap();
        assert_eq!(db.proven_function_entries("overlay"), vec![0x8010_0040]);
        assert!(db.proven_function_entries("boot").is_empty());
    }

    #[test]
    fn inductive_authority_roots_include_only_proven_hardware_claims() {
        let mut db = FactDb::new();
        let hardware = BankAddr::new("boot", 0x8000_0400);
        let hardware_claim = db.insert(Fact::FunctionEntryClaim {
            target: hardware.clone(),
            detector: CandidateDetector::HardwareEntrypoint,
            evidence: FunctionEntryEvidence::RomHeaderEntrypoint,
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&hardware),
            ProofState::Proven,
            vec![hardware_claim],
            "hardware entry",
        )
        .unwrap();

        let table_target = BankAddr::new("boot", 0x8000_0800);
        let table_claim = db.insert(Fact::FunctionEntryClaim {
            target: table_target.clone(),
            detector: CandidateDetector::TableDerived,
            evidence: FunctionEntryEvidence::TableEntry {
                table: BankAddr::new("boot", 0x8000_1000),
                index: 0,
            },
            proposed_state: ProofState::Proven,
        });
        db.conclude(
            function_entry_subject(&table_target),
            ProofState::Proven,
            vec![table_claim],
            "table entry",
        )
        .unwrap();

        assert_eq!(
            db.proven_function_entries("boot"),
            vec![0x8000_0400, 0x8000_0800]
        );
        assert_eq!(
            db.proven_hardware_function_entries("boot"),
            vec![0x8000_0400]
        );
        assert!(db.proven_hardware_function_entries("overlay").is_empty());
    }

    #[test]
    fn conclude_allows_proven_to_proven_reconfirmation() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Proven, vec![f], "rule_a")
            .unwrap();
        db.conclude("x", ProofState::Proven, vec![f], "rule_b")
            .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Proven);
    }

    #[test]
    fn conclude_allows_candidate_to_rejected_on_contradiction() {
        let mut db = FactDb::new();
        let f = db.insert(Fact::BlockStart {
            bank: "boot".into(),
            pc: 0x8000_0400,
        });
        db.conclude("x", ProofState::Candidate, vec![f], "heuristic")
            .unwrap();
        db.conclude(
            "x",
            ProofState::Rejected,
            vec![f],
            "contradicted_by_data_write",
        )
        .unwrap();
        assert_eq!(db.conclusion("x").unwrap().state, ProofState::Rejected);
    }

    #[test]
    fn proven_rom_mappings_filters_by_conclusion_not_presence() {
        let mut db = FactDb::new();
        let f0 = db.insert(Fact::RomMapping {
            bank: "boot".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x2000,
            va_start: 0x8000_0400,
            va_end: 0x8000_1400,
        });
        let f1 = db.insert(Fact::RomMapping {
            bank: "overlay_1".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x3000,
            rom_end: 0x4000,
            va_start: 0x8010_0000,
            va_end: 0x8010_1000,
        });
        db.conclude("bank:boot", ProofState::Proven, vec![f0], "boot_copy")
            .unwrap();
        db.conclude(
            "bank:overlay_1",
            ProofState::Candidate,
            vec![f1],
            "heuristic",
        )
        .unwrap();

        let proven = db.proven_rom_mappings();
        assert_eq!(proven.len(), 1);
        assert!(matches!(proven[0], Fact::RomMapping { bank, .. } if bank == "boot"));
    }

    #[test]
    fn evaluated_image_receipt_identity_is_stable_content_only() {
        let receipt = evaluated_receipt();
        let digest = evaluated_image_receipt_sha256_v1(&receipt);
        assert_eq!(
            digest,
            "b8d111b46163af2978271f8f66513133a7b3bd3e78eef308c601dbf2b25111d5"
        );
        assert_eq!(digest, evaluated_image_receipt_sha256_v1(&receipt));

        let wire = serde_json::to_value(&receipt).unwrap();
        assert!(wire.get("bytes").is_none());
        assert!(wire["streams"][0].get("bytes").is_none());
        assert!(wire["trailing_suffix"].get("bytes").is_none());

        let mut changed = receipt;
        changed.trailing_suffix.len += 1;
        assert_ne!(digest, evaluated_image_receipt_sha256_v1(&changed));
    }

    #[test]
    fn evaluated_image_1173_receipt_has_distinct_stable_identity() {
        let mut receipt = evaluated_receipt();
        receipt.evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflate1173SequenceV1 { stream_count: 1 };

        assert_eq!(
            evaluated_image_receipt_sha256_v1(&receipt),
            "3bda2c534cdefe150c9c5406a6d983891708597d4e736c95de580c5bd9300f77"
        );
        assert_eq!(
            serde_json::to_value(&receipt).unwrap()["evaluator"]["kind"],
            "headered_raw_deflate_1173_sequence_v1"
        );
    }

    #[test]
    fn proven_bank_images_preserve_typed_backing_and_proof_gate() {
        let mut db = FactDb::new();
        let affine = db.insert(Fact::RomMapping {
            bank: "affine".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x1000,
            rom_end: 0x1010,
            va_start: 0x8010_0000,
            va_end: 0x8010_0010,
        });
        let supported = db.insert(Fact::EvaluatedImage {
            bank: "supported".into(),
            va_start: 0x8020_0000,
            va_end: 0x8020_0008,
            receipt: evaluated_receipt(),
        });
        let receipt = evaluated_receipt();
        let materialized_digest = evaluated_image_receipt_sha256_v1(&receipt);
        let materialized = db.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: 0x8030_0000,
            va_end: 0x8030_0008,
            receipt,
        });
        db.conclude("bank:affine", ProofState::Proven, vec![affine], "test")
            .unwrap();
        db.conclude(
            "bank:supported",
            ProofState::Supported,
            vec![supported],
            "candidate evaluation only",
        )
        .unwrap();
        db.conclude(
            "bank:materialized",
            ProofState::Proven,
            vec![materialized],
            "test full proof",
        )
        .unwrap();

        assert_eq!(db.proven_rom_mappings().len(), 1);
        assert_eq!(
            db.proven_bank_images(),
            vec![
                ProvenBankImageV1 {
                    bank: "affine".into(),
                    va_start: 0x8010_0000,
                    va_end: 0x8010_0010,
                    backing: BankBackingV1::RomAffine {
                        rom_space: RomAddressSpace::Virtual,
                        rom_start: 0x1000,
                        rom_end: 0x1010,
                    },
                },
                ProvenBankImageV1 {
                    bank: "materialized".into(),
                    va_start: 0x8030_0000,
                    va_end: 0x8030_0008,
                    backing: BankBackingV1::Materialized {
                        receipt_sha256: materialized_digest,
                        output_len: 8,
                    },
                },
            ]
        );
    }

    #[test]
    fn projection_retains_evaluated_image_canonical_bank_authority() {
        let mut db = FactDb::new();
        let fact = db.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: 0x8030_0000,
            va_end: 0x8030_0008,
            receipt: evaluated_receipt(),
        });
        db.conclude(
            "bank:materialized",
            ProofState::Proven,
            vec![fact],
            "test full proof",
        )
        .unwrap();

        let projection = FactProjectionIndex::new(&db).unwrap();
        let projected = projection.project("materialized");
        assert!(matches!(
            projected.facts(),
            [Fact::EvaluatedImage { bank, .. }] if bank == "materialized"
        ));
        assert_eq!(projected.proven_bank_images(), db.proven_bank_images());
    }

    #[test]
    fn proven_bank_images_retain_same_bank_backing_ambiguity() {
        let mut db = FactDb::new();
        let affine = db.insert(Fact::RomMapping {
            bank: "ambiguous".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1008,
            va_start: 0x8040_0000,
            va_end: 0x8040_0008,
        });
        let evaluated = db.insert(Fact::EvaluatedImage {
            bank: "ambiguous".into(),
            va_start: 0x8040_0000,
            va_end: 0x8040_0008,
            receipt: evaluated_receipt(),
        });
        db.conclude(
            "bank:ambiguous",
            ProofState::Proven,
            vec![affine, evaluated],
            "test competing proven evidence",
        )
        .unwrap();

        let images = db.proven_bank_images();
        assert_eq!(images.len(), 2);
        assert!(matches!(images[0].backing, BankBackingV1::RomAffine { .. }));
        assert!(matches!(
            images[1].backing,
            BankBackingV1::Materialized { .. }
        ));
    }

    #[test]
    fn uncited_same_bank_candidate_does_not_inherit_bank_authority() {
        let mut db = FactDb::new();
        let affine = db.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1008,
            va_start: 0x8040_0000,
            va_end: 0x8040_0008,
        });
        db.insert(Fact::EvaluatedImage {
            bank: "bank".into(),
            va_start: 0x8040_0000,
            va_end: 0x8040_0008,
            receipt: evaluated_receipt(),
        });
        db.conclude(
            "bank:bank",
            ProofState::Proven,
            vec![affine],
            "only the affine image is proven",
        )
        .unwrap();

        assert_eq!(db.proven_bank_images().len(), 1);
        assert!(matches!(
            db.resolve_proven_bank_backing_span("bank", 0x8040_0000, 0x8040_0008),
            BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::RomAffine { .. })
        ));
    }

    #[test]
    fn materialized_backing_span_wire_has_no_rom_coordinates() {
        let span = BankBackingSpanV1::Materialized {
            receipt_sha256: "55".repeat(32),
            output_start: 4,
            output_end: 12,
        };
        let wire = serde_json::to_value(span).unwrap();
        assert_eq!(wire["kind"], "materialized");
        assert!(wire.get("rom_start").is_none());
        assert!(wire.get("rom_end").is_none());
        assert_eq!(wire["output_start"], 4);
        assert_eq!(wire["output_end"], 12);
    }

    #[test]
    fn backing_span_resolves_physical_and_virtual_affine_offsets() {
        for (bank, rom_space) in [
            ("physical", RomAddressSpace::Physical),
            ("virtual", RomAddressSpace::Virtual),
        ] {
            let mut db = FactDb::new();
            let fact = db.insert(Fact::RomMapping {
                bank: bank.into(),
                rom_space,
                rom_start: 0x1200,
                rom_end: 0x1240,
                va_start: 0x8000_1000,
                va_end: 0x8000_1040,
            });
            prove_bank(&mut db, bank, vec![fact]);

            assert_eq!(
                db.resolve_proven_bank_backing_span(bank, 0x8000_100c, 0x8000_1024),
                BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::RomAffine {
                    rom_space,
                    rom_start: 0x120c,
                    rom_end: 0x1224,
                })
            );
        }
    }

    #[test]
    fn backing_span_excludes_affine_bss() {
        let mut db = FactDb::new();
        let fact = db.insert(Fact::RomMapping {
            bank: "bss".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x2000,
            rom_end: 0x2020,
            va_start: 0x8010_0000,
            va_end: 0x8010_0040,
        });
        prove_bank(&mut db, "bss", vec![fact]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("bss", 0x8010_001c, 0x8010_0024),
            BankBackingSpanResolutionV1::Missing
        );
        assert_eq!(
            db.resolve_proven_bank_backing_span("bss", 0x8010_0020, 0x8010_0024),
            BankBackingSpanResolutionV1::Missing
        );
    }

    #[test]
    fn backing_span_resolves_materialized_offsets() {
        let mut db = FactDb::new();
        let receipt = evaluated_receipt();
        let digest = evaluated_image_receipt_sha256_v1(&receipt);
        let fact = db.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: 0x8020_0000,
            va_end: 0x8020_0008,
            receipt,
        });
        prove_bank(&mut db, "materialized", vec![fact]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("materialized", 0x8020_0002, 0x8020_0007),
            BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::Materialized {
                receipt_sha256: digest,
                output_start: 2,
                output_end: 7,
            })
        );
    }

    #[test]
    fn backing_span_excludes_supported_images() {
        let mut db = FactDb::new();
        let fact = db.insert(Fact::EvaluatedImage {
            bank: "supported".into(),
            va_start: 0x8030_0000,
            va_end: 0x8030_0008,
            receipt: evaluated_receipt(),
        });
        db.conclude(
            "bank:supported",
            ProofState::Supported,
            vec![fact],
            "test candidate",
        )
        .unwrap();

        assert_eq!(
            db.resolve_proven_bank_backing_span("supported", 0x8030_0000, 0x8030_0004),
            BankBackingSpanResolutionV1::Missing
        );
    }

    #[test]
    fn backing_span_collapses_exact_duplicate_images() {
        let mut db = FactDb::new();
        let mapping = Fact::RomMapping {
            bank: "duplicate".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x3000,
            rom_end: 0x3010,
            va_start: 0x8040_0000,
            va_end: 0x8040_0010,
        };
        let first = db.insert(mapping.clone());
        let second = db.insert(mapping);
        prove_bank(&mut db, "duplicate", vec![first, second]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("duplicate", 0x8040_0004, 0x8040_0008),
            BankBackingSpanResolutionV1::Unique(BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x3004,
                rom_end: 0x3008,
            })
        );
    }

    #[test]
    fn backing_span_rejects_distinct_receipt_ambiguity_before_coverage() {
        let mut db = FactDb::new();
        let first = db.insert(Fact::EvaluatedImage {
            bank: "ambiguous".into(),
            va_start: 0x8050_0000,
            va_end: 0x8050_0008,
            receipt: evaluated_receipt(),
        });
        let mut other_receipt = evaluated_receipt();
        other_receipt.source_sha256 = "99".repeat(32);
        let second = db.insert(Fact::EvaluatedImage {
            bank: "ambiguous".into(),
            va_start: 0x8060_0000,
            va_end: 0x8060_0008,
            receipt: other_receipt,
        });
        prove_bank(&mut db, "ambiguous", vec![first, second]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("ambiguous", 0x8050_0000, 0x8050_0004),
            BankBackingSpanResolutionV1::Ambiguous
        );
    }

    #[test]
    fn backing_span_rejects_affine_materialized_ambiguity() {
        let mut db = FactDb::new();
        let affine = db.insert(Fact::RomMapping {
            bank: "mixed".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: 0x4000,
            rom_end: 0x4008,
            va_start: 0x8070_0000,
            va_end: 0x8070_0008,
        });
        let materialized = db.insert(Fact::EvaluatedImage {
            bank: "mixed".into(),
            va_start: 0x8070_0000,
            va_end: 0x8070_0008,
            receipt: evaluated_receipt(),
        });
        prove_bank(&mut db, "mixed", vec![affine, materialized]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("mixed", 0x8070_0000, 0x8070_0004),
            BankBackingSpanResolutionV1::Ambiguous
        );
    }

    #[test]
    fn backing_span_reports_invalid_request_and_inverted_affine_geometry() {
        let db = FactDb::new();
        assert_eq!(
            db.resolve_proven_bank_backing_span("missing", 4, 4),
            BankBackingSpanResolutionV1::InvalidGeometry
        );
        assert_eq!(
            db.resolve_proven_bank_backing_span("missing", 8, 4),
            BankBackingSpanResolutionV1::InvalidGeometry
        );

        let mut inverted_va = FactDb::new();
        let fact = inverted_va.insert(Fact::RomMapping {
            bank: "inverted-va".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1008,
            va_start: 0x8080_0008,
            va_end: 0x8080_0000,
        });
        prove_bank(&mut inverted_va, "inverted-va", vec![fact]);
        assert_eq!(
            inverted_va.resolve_proven_bank_backing_span("inverted-va", 0x8080_0000, 0x8080_0004),
            BankBackingSpanResolutionV1::InvalidGeometry
        );

        let mut inverted_rom = FactDb::new();
        let fact = inverted_rom.insert(Fact::RomMapping {
            bank: "inverted-rom".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1010,
            rom_end: 0x1000,
            va_start: 0x8090_0000,
            va_end: 0x8090_0010,
        });
        prove_bank(&mut inverted_rom, "inverted-rom", vec![fact]);
        assert_eq!(
            inverted_rom.resolve_proven_bank_backing_span("inverted-rom", 0x8090_0000, 0x8090_0004),
            BankBackingSpanResolutionV1::InvalidGeometry
        );
    }

    #[test]
    fn backing_span_reports_materialized_output_mismatch() {
        let mut db = FactDb::new();
        let fact = db.insert(Fact::EvaluatedImage {
            bank: "mismatch".into(),
            va_start: 0x80a0_0000,
            va_end: 0x80a0_0010,
            receipt: evaluated_receipt(),
        });
        prove_bank(&mut db, "mismatch", vec![fact]);

        assert_eq!(
            db.resolve_proven_bank_backing_span("mismatch", 0x80a0_0000, 0x80a0_0004),
            BankBackingSpanResolutionV1::InvalidGeometry
        );
    }
}
