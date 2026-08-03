//! Code banks, block programs, and the generated-runner source receipts.
//! Split from the execution module body purely by size; `mod.rs` re-exports
//! everything public so `execution::` paths are unchanged.

use super::*;

/// One owned, contiguous executable span within a bank.
///
/// Construction binds the span to its bank identity and proves nonempty,
/// aligned, non-overflowing geometry. Cross-span ordering and overlap are
/// validated by [`CodeBank::from_spans`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    bank: BankId,
    vram_start: GuestPc,
    words: Vec<u32>,
}

impl CodeSpan {
    pub fn new(bank: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        if !vram_start.is_instruction_aligned() {
            return Err(BankError::UnalignedStart {
                bank,
                start: vram_start,
            });
        }
        if words.is_empty() {
            return Err(BankError::Empty { bank });
        }
        let byte_len = u32::try_from(words.len())
            .ok()
            .and_then(|len| len.checked_mul(4))
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        vram_start
            .get()
            .checked_add(byte_len)
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        Ok(Self {
            bank,
            vram_start,
            words,
        })
    }

    pub const fn bank(&self) -> BankId {
        self.bank
    }

    pub const fn vram_start(&self) -> GuestPc {
        self.vram_start
    }

    pub fn vram_end(&self) -> GuestPc {
        GuestPc::new(self.vram_start.get() + self.words.len() as u32 * 4)
    }

    pub fn instruction_count(&self) -> usize {
        self.words.len()
    }

    /// Exact big-endian instruction words owned by this immutable span.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let offset = pc.get().checked_sub(self.vram_start.get())?;
        self.words.get((offset / 4) as usize).copied()
    }
}

/// One immutable sparse executable image admitted to the block translator.
///
/// A bank owns sorted, disjoint [`CodeSpan`] values. Its lowest/highest
/// addresses are diagnostic bounds only; addresses in holes never resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBank {
    id: BankId,
    spans: Vec<CodeSpan>,
}

/// Stable 256-bit identity of the executable artifact installed by a host.
///
/// Function-lane native callables are opaque to safe Rust, so their producer
/// supplies the SHA-256 (or an equally stable 256-bit build identity) of the
/// actual generated artifact. Block programs derive their aggregate identity
/// from the canonical bank image plus each runner's supplied artifact
/// identity. Native addresses are never accepted as artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramArtifactIdentity([u8; 32]);

/// Callable shape installed around one generated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratedAdapterRole {
    DirectGenerated,
    EntryContextGate,
    DenseInstrumentationGate,
    OverlayGenerationGate,
    ExternalDigestGate,
}

impl GeneratedAdapterRole {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::DirectGenerated => 0,
            Self::EntryContextGate => 1,
            Self::DenseInstrumentationGate => 2,
            Self::OverlayGenerationGate => 3,
            Self::ExternalDigestGate => 4,
        }
    }
}

impl ProgramArtifactIdentity {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    /// Identity of an installed callable which combines a handwritten
    /// adapter with one exact generated bank runner.
    pub fn generated_adapter(
        adapter_source_identity: [u8; 32],
        generated_runner_source_identity: [u8; 32],
        bank: BankId,
        role: GeneratedAdapterRole,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"fn64:generated-runner-adapter:v1:");
        hasher.update(adapter_source_identity);
        hasher.update(generated_runner_source_identity);
        hasher.update(bank.get().to_be_bytes());
        hasher.update([role.tag()]);
        Self(hasher.finalize().into())
    }
}

pub const GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2: &str =
    "fn64.generated-runner-source-attestation.v2";
/// Canonical hash-domain prefix shared by the source-attestation issuer and
/// the independent selected-build verifier.
pub const GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2: &[u8] =
    b"fn64:cargo-generated-runner-source-attestation:v2:";
pub const GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1: &str =
    "fn64.generated-runner-runtime-source.v1";
pub const GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2: &str =
    "fn64.generated-runner-runtime-source.v2";

/// Exact source receipt for the implementation linked by typed arbitrary-PC
/// runners.
///
/// These files own typed RDRAM/MMIO routing, host-boundary exits, and
/// block-program admission. `fn64-recomp-rs-codegen` issues the separate
/// emitter-source receipt. Neither receipt says anything about a separately
/// compiled callable; only the external build owner proves that relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRuntimeSourceReceiptV1 {
    schema: &'static str,
    source_sha256: [u8; 32],
    typed_rdram: bool,
    typed_mmio: bool,
    typed_host_boundaries: bool,
}

impl GeneratedRunnerRuntimeSourceReceiptV1 {
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn typed_rdram(self) -> bool {
        self.typed_rdram
    }

    pub const fn typed_mmio(self) -> bool {
        self.typed_mmio
    }

    pub const fn typed_host_boundaries(self) -> bool {
        self.typed_host_boundaries
    }
}

pub fn generated_runner_runtime_source_receipt_v1() -> GeneratedRunnerRuntimeSourceReceiptV1 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("../lib.rs")),
        (
            b"src/execution/catalog_v1.rs",
            include_bytes!("catalog_v1.rs"),
        ),
        (b"src/execution/mod.rs", include_bytes!("mod.rs")),
        (
            b"src/execution/program.rs",
            include_bytes!("program.rs"),
        ),
        (
            b"src/execution/tests/mod.rs",
            include_bytes!("tests/mod.rs"),
        ),
        (
            b"src/execution/tests/programs.rs",
            include_bytes!("tests/programs.rs"),
        ),
        (
            b"src/generated_support.rs",
            include_bytes!("../generated_support.rs"),
        ),
        (
            b"src/runtime/fpu_ops.rs",
            include_bytes!("../runtime/fpu_ops.rs"),
        ),
        (
            b"src/runtime/host.rs",
            include_bytes!("../runtime/host.rs"),
        ),
        (
            b"src/runtime/mod.rs",
            include_bytes!("../runtime/mod.rs"),
        ),
        (
            b"src/runtime/tests.rs",
            include_bytes!("../runtime/tests.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:generated-runner-runtime-source:v1:");
    for (label, source) in sources {
        hasher.update(
            u64::try_from(label.len())
                .expect("generated-runner source label length fits u64")
                .to_be_bytes(),
        );
        hasher.update(label);
        hasher.update(
            u64::try_from(source.len())
                .expect("generated-runner source length fits u64")
                .to_be_bytes(),
        );
        hasher.update(source);
    }
    GeneratedRunnerRuntimeSourceReceiptV1 {
        schema: GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1,
        source_sha256: hasher.finalize().into(),
        typed_rdram: true,
        typed_mmio: true,
        typed_host_boundaries: true,
    }
}

/// Source-complete runtime identity for typed arbitrary-PC runners.
///
/// V1 remains immutable for existing source-attestation V2 producers and
/// consumers. V2 adds `fpu.rs`, whose floating-point implementation is called
/// through `runtime.rs` and therefore changes generated-runner semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRuntimeSourceReceiptV2 {
    schema: &'static str,
    source_sha256: [u8; 32],
    typed_rdram: bool,
    typed_mmio: bool,
    typed_host_boundaries: bool,
}

impl GeneratedRunnerRuntimeSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn typed_rdram(self) -> bool {
        self.typed_rdram
    }

    pub const fn typed_mmio(self) -> bool {
        self.typed_mmio
    }

    pub const fn typed_host_boundaries(self) -> bool {
        self.typed_host_boundaries
    }
}

pub fn generated_runner_runtime_source_receipt_v2() -> GeneratedRunnerRuntimeSourceReceiptV2 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("../lib.rs")),
        (
            b"src/execution/catalog_v1.rs",
            include_bytes!("catalog_v1.rs"),
        ),
        (b"src/execution/mod.rs", include_bytes!("mod.rs")),
        (
            b"src/execution/program.rs",
            include_bytes!("program.rs"),
        ),
        (
            b"src/execution/tests/mod.rs",
            include_bytes!("tests/mod.rs"),
        ),
        (
            b"src/execution/tests/programs.rs",
            include_bytes!("tests/programs.rs"),
        ),
        (
            b"src/generated_support.rs",
            include_bytes!("../generated_support.rs"),
        ),
        (
            b"src/runtime/fpu_ops.rs",
            include_bytes!("../runtime/fpu_ops.rs"),
        ),
        (
            b"src/runtime/host.rs",
            include_bytes!("../runtime/host.rs"),
        ),
        (
            b"src/runtime/mod.rs",
            include_bytes!("../runtime/mod.rs"),
        ),
        (
            b"src/runtime/tests.rs",
            include_bytes!("../runtime/tests.rs"),
        ),
        (b"src/fpu.rs", include_bytes!("../fpu.rs")),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:generated-runner-runtime-source:v2:");
    for (label, source) in sources {
        hasher.update(
            u64::try_from(label.len())
                .expect("generated-runner source label length fits u64")
                .to_be_bytes(),
        );
        hasher.update(label);
        hasher.update(
            u64::try_from(source.len())
                .expect("generated-runner source length fits u64")
                .to_be_bytes(),
        );
        hasher.update(source);
    }
    GeneratedRunnerRuntimeSourceReceiptV2 {
        schema: GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2,
        source_sha256: hasher.finalize().into(),
        typed_rdram: true,
        typed_mmio: true,
        typed_host_boundaries: true,
    }
}

/// One callable/source relation exported by a repository-controlled generated
/// Cargo package and linked into the program-owning root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CargoGeneratedRunnerSourceBindingV1 {
    pub bank: BankId,
    pub generated_runner_source_sha256: [u8; 32],
    pub code_words_sha256: [u8; 32],
    pub vram_start: GuestPc,
    pub vram_end: GuestPc,
    pub composite_subrunner_count: u32,
    pub adapter_role: GeneratedAdapterRole,
}

/// Build-script measurements supplied at the Cargo source boundary.
///
/// Safe Rust cannot derive source identity from `GeneratedBankFn`. The caller
/// of this attestation is expected to be the checked-in root which
/// owns Cargo dependencies, generated includes, and the exported callable
/// table. Generic or third-party runner registration is deliberately outside
/// this source projection.
pub struct CargoGeneratedProgramSourceAttestationV2<'a> {
    pub root_adapter_source_sha256: [u8; 32],
    pub shard_cargo_source_tree_sha256: [u8; 32],
    pub expected_emitter_source_sha256: [u8; 32],
    /// Measured by the checked-in adapter and revalidated only by the outer
    /// verifier. This lower catalog is explicitly not an issuer of emitter
    /// source authority.
    pub externally_measured_emitter_source_sha256: [u8; 32],
    pub expected_runtime_source_sha256: [u8; 32],
    pub runtime_source_receipt: GeneratedRunnerRuntimeSourceReceiptV1,
    pub runners: &'a [CargoGeneratedRunnerSourceBindingV1],
}

/// Evidence projection claiming that one complete canonical block program was
/// paired with the measured Cargo source graph above.
///
/// This is deliberately named an attestation, not authority: this crate can
/// validate all pointer-free fields but cannot prove that rustc compiled a
/// supplied function pointer from particular bytes. SI or another completion
/// validator must not consume it. A verifier which owns an isolated Cargo
/// build (or an external build attestation) is required to mint authority.
#[derive(Debug)]
pub struct GeneratedRunnerSourceAttestationV2 {
    pub(super) schema: &'static str,
    pub(super) cargo_source_fields_validated: bool,
    pub(super) program_identity: ProgramArtifactIdentity,
    pub(super) root_adapter_source_sha256: [u8; 32],
    pub(super) shard_cargo_source_tree_sha256: [u8; 32],
    pub(super) emitter_source_sha256: [u8; 32],
    pub(super) runtime_source_sha256: [u8; 32],
    pub(super) binding_sha256: [u8; 32],
    pub(super) build_receipt: StaticExecutionBuildReceipt,
}

impl GeneratedRunnerSourceAttestationV2 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub const fn cargo_source_fields_validated(&self) -> bool {
        self.cargo_source_fields_validated
    }

    pub const fn program_identity(&self) -> ProgramArtifactIdentity {
        self.program_identity
    }

    pub const fn root_adapter_source_sha256(&self) -> [u8; 32] {
        self.root_adapter_source_sha256
    }

    pub const fn shard_cargo_source_tree_sha256(&self) -> [u8; 32] {
        self.shard_cargo_source_tree_sha256
    }

    pub const fn emitter_source_sha256(&self) -> [u8; 32] {
        self.emitter_source_sha256
    }

    pub const fn runtime_source_sha256(&self) -> [u8; 32] {
        self.runtime_source_sha256
    }

    pub const fn binding_sha256(&self) -> [u8; 32] {
        self.binding_sha256
    }

    pub const fn build_receipt(&self) -> StaticExecutionBuildReceipt {
        self.build_receipt
    }
}

/// Authority behind a program artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramIdentitySource {
    /// The host identified an opaque generated native artifact.
    CallerSupplied,
    /// fn64 hashed the complete canonical block code plus the stable artifact
    /// identity of every generated bank runner.
    CanonicalBlockProgramSha256,
}

/// Identity plus the authority which established it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramIdentityEvidenceSnapshot {
    pub identity: ProgramArtifactIdentity,
    pub source: ProgramIdentitySource,
}

/// Pointer-independent image of one contiguous executable span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpanEvidenceSnapshot {
    pub vram_start: GuestPc,
    pub words: Vec<u32>,
}

/// Pointer-independent image of one immutable sparse code bank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBankEvidenceSnapshot {
    pub id: BankId,
    pub runner_artifact_identity: ProgramArtifactIdentity,
    pub spans: Vec<CodeSpanEvidenceSnapshot>,
}

/// Complete canonical executable image owned by a [`BlockProgram`].
///
/// Virtual and physical banks, spans, and mapped AOT entries are sorted by
/// their typed identities/addresses. Instruction word order is architectural
/// and is retained verbatim. Generated runner pointers are deliberately
/// absent, but each generated unit retains its stable artifact identity: the
/// words alone cannot prove two native callables implement the same semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProgramEvidenceSnapshot {
    pub identity: ProgramIdentityEvidenceSnapshot,
    pub banks: Vec<CodeBankEvidenceSnapshot>,
    pub physical_banks: Vec<PhysicalCodeBankEvidenceSnapshot>,
    pub mapped_aot: Vec<MappedAotEvidenceSnapshot>,
}

/// One successfully entered bank-qualified guest execution destination.
///
/// The bank identity names the immutable code-image generation, while the
/// optional runner identity names the generated native artifact that was
/// actually entered. `None` is retained for the compatibility
/// [`GeneratedBankRunner::new`] path and the mapped-interpreter fallback;
/// neither may be promoted to release evidence without a typed artifact
/// authority.
/// Historical execution observations are intentionally separate from
/// [`BlockProgramEvidenceSnapshot`]: they describe what happened, not state
/// which can affect future execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionDestinationObservation {
    pub destination: ExecutionKey,
    pub runner_artifact_identity: Option<ProgramArtifactIdentity>,
    /// Architecturally retired instructions in this runner entry. Retaining
    /// the count lets a minimum-budget diagnostic reconstruct the exact
    /// straight-line PC sequence without instrumenting generated bodies.
    pub instructions: u32,
}

impl CodeBank {
    /// Convenience constructor for a single contiguous executable span.
    pub fn new(id: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        Self::from_spans(id, vec![CodeSpan::new(id, vram_start, words)?])
    }

    /// Admit sorted, disjoint executable spans under one immutable identity.
    pub fn from_spans(id: BankId, mut spans: Vec<CodeSpan>) -> Result<Self, BankError> {
        if spans.is_empty() {
            return Err(BankError::Empty { bank: id });
        }
        for span in &spans {
            if span.bank() != id {
                return Err(BankError::SpanBankMismatch {
                    bank: id,
                    span_bank: span.bank(),
                    start: span.vram_start(),
                });
            }
        }
        spans.sort_by_key(CodeSpan::vram_start);
        for pair in spans.windows(2) {
            let left_end = pair[0].vram_end();
            let right_start = pair[1].vram_start();
            if right_start < left_end {
                return Err(BankError::OverlappingSpans {
                    bank: id,
                    left_end,
                    right_start,
                });
            }
        }
        Ok(Self { id, spans })
    }

    pub const fn id(&self) -> BankId {
        self.id
    }

    pub fn vram_start(&self) -> GuestPc {
        self.spans[0].vram_start()
    }

    pub fn vram_end(&self) -> GuestPc {
        self.spans
            .last()
            .expect("CodeBank construction requires a span")
            .vram_end()
    }

    pub fn instruction_count(&self) -> usize {
        self.spans.iter().map(CodeSpan::instruction_count).sum()
    }

    pub fn spans(&self) -> &[CodeSpan] {
        &self.spans
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let candidate = self
            .spans
            .partition_point(|span| span.vram_start() <= pc)
            .checked_sub(1)?;
        let span = &self.spans[candidate];
        if pc < span.vram_end() {
            span.resolve(pc)
        } else {
            None
        }
    }
}

/// Failure to admit an executable image into a [`CodeCatalog`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankError {
    Empty {
        bank: BankId,
    },
    UnalignedStart {
        bank: BankId,
        start: GuestPc,
    },
    AddressOverflow {
        bank: BankId,
        start: GuestPc,
    },
    SpanBankMismatch {
        bank: BankId,
        span_bank: BankId,
        start: GuestPc,
    },
    OverlappingSpans {
        bank: BankId,
        left_end: GuestPc,
        right_start: GuestPc,
    },
    DuplicateId {
        bank: BankId,
    },
}

impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BankError::Empty { bank } => write!(f, "{bank} has no executable words"),
            BankError::UnalignedStart { bank, start } => {
                write!(f, "{bank} starts at unaligned PC {start}")
            }
            BankError::AddressOverflow { bank, start } => {
                write!(
                    f,
                    "{bank} starting at {start} exceeds the guest address space"
                )
            }
            BankError::SpanBankMismatch {
                bank,
                span_bank,
                start,
            } => write!(
                f,
                "{bank} cannot own span from {span_bank} starting at {start}"
            ),
            BankError::OverlappingSpans {
                bank,
                left_end,
                right_start,
            } => write!(
                f,
                "{bank} has overlapping executable spans at {left_end} and {right_start}"
            ),
            BankError::DuplicateId { bank } => {
                write!(f, "executable identity {bank} is already registered")
            }
        }
    }
}

impl std::error::Error for BankError {}

/// A resolved instruction word and the bank-qualified address that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedInstruction {
    pub key: ExecutionKey,
    pub word: u32,
}

/// Deterministic registry of immutable executable images.
///
/// Banks may overlap in virtual address space.  Only their identities must be
/// unique, which is exactly what prevents an overlay lookup from silently
/// selecting whichever same-VA image happened to be registered last.
#[derive(Clone, Debug, Default)]
pub struct CodeCatalog {
    banks: BTreeMap<BankId, CodeBank>,
}

impl CodeCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, bank: CodeBank) -> Result<(), BankError> {
        let id = bank.id();
        if self.banks.contains_key(&id) {
            return Err(BankError::DuplicateId { bank: id });
        }
        self.banks.insert(id, bank);
        Ok(())
    }

    pub fn bank(&self, id: BankId) -> Option<&CodeBank> {
        self.banks.get(&id)
    }

    pub fn banks(&self) -> impl Iterator<Item = &CodeBank> {
        self.banks.values()
    }

    fn unregister(&mut self, id: BankId) -> Option<CodeBank> {
        self.banks.remove(&id)
    }

    pub fn resolve(&self, key: ExecutionKey) -> Result<ResolvedInstruction, CpuFault> {
        if !key.pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(key));
        }
        let bank = self.banks.get(&key.bank).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnknownBank,
        })?;
        let start = bank.vram_start().get();
        let end = bank.vram_end().get();
        let word = bank.resolve(key.pc).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start,
                bank_end: end,
            },
        })?;
        Ok(ResolvedInstruction { key, word })
    }

    fn missing_virtual_mapping(&self, fault_bank: BankId, target_pc: GuestPc) -> CpuFault {
        let at = ExecutionKey::new(fault_bank, target_pc);
        match self.banks.get(&fault_bank) {
            Some(bank) => CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: bank.vram_start().get(),
                    bank_end: bank.vram_end().get(),
                },
            },
            None => CpuFault {
                at,
                kind: CpuFaultKind::UnknownBank,
            },
        }
    }

    fn resolve_unique_virtual(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.resolve_unique_virtual_where(fault_bank, target_pc, |_| true)
    }

    fn resolve_unique_virtual_where(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
        mut admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        let mut candidates = self
            .banks
            .values()
            .filter(|bank| admits_bank(bank.id()) && bank.resolve(target_pc).is_some())
            .map(CodeBank::id);
        let Some(first_candidate) = candidates.next() else {
            return Err(self.missing_virtual_mapping(fault_bank, target_pc));
        };
        let Some(second_candidate) = candidates.next() else {
            return Ok(ExecutionKey::new(first_candidate, target_pc));
        };
        let remaining = candidates.count();
        let candidate_count = u32::try_from(remaining)
            .ok()
            .and_then(|remaining| remaining.checked_add(2))
            .expect("virtual code-bank candidate count exceeds u32");
        Err(CpuFault {
            at: ExecutionKey::new(fault_bank, target_pc),
            kind: CpuFaultKind::AmbiguousPc {
                first_candidate,
                second_candidate,
                candidate_count,
            },
        })
    }

    /// Resolve a bankless static entry against every admitted virtual bank.
    /// `fault_bank` anchors typed failure context only; it receives no
    /// preference. Physical/mapped generations are outside this catalog.
    pub fn resolve_entry(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                fault_bank, target_pc,
            )));
        }
        self.resolve_unique_virtual(fault_bank, target_pc)
    }

    pub(super) fn resolve_entry_where(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
        admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                fault_bank, target_pc,
            )));
        }
        self.resolve_unique_virtual_where(fault_bank, target_pc, admits_bank)
    }

    /// Resolve one static guest transfer. The source bank wins when it admits
    /// the exact sparse target; otherwise resolution requires exactly one
    /// admitting virtual bank. Generated callbacks and physical generations
    /// are deliberately not consulted.
    pub fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        if self
            .banks
            .get(&source_bank)
            .is_some_and(|bank| bank.resolve(target_pc).is_some())
        {
            return Ok(ExecutionKey::new(source_bank, target_pc));
        }
        self.resolve_unique_virtual(source_bank, target_pc)
    }

    pub(super) fn resolve_transfer_where(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
        mut admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        if admits_bank(source_bank)
            && self
                .banks
                .get(&source_bank)
                .is_some_and(|bank| bank.resolve(target_pc).is_some())
        {
            return Ok(ExecutionKey::new(source_bank, target_pc));
        }
        self.resolve_unique_virtual_where(source_bank, target_pc, admits_bank)
    }

    /// Classify an admitted instruction for table-backed dispatch.  Resolution
    /// goes through the same sparse bank catalog as execution, so a data hole
    /// cannot acquire a classification merely because it lies inside a
    /// bounding interval.
    pub fn classify(&self, key: ExecutionKey) -> Result<BankWordKind, CpuFault> {
        let resolved = self.resolve(key)?;
        let instruction = crate::decode(resolved.word);
        Ok(
            if matches!(instruction, crate::decoder::Instruction::Unknown { .. }) {
                BankWordKind::Unknown
            } else if instruction.has_delay_slot() {
                BankWordKind::ControlTransfer
            } else {
                BankWordKind::Straight
            },
        )
    }
}

/// Failure to atomically pair admitted code with its generated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramError {
    RunnerBankMismatch {
        code_bank: BankId,
        runner_bank: BankId,
    },
    DuplicateBank {
        bank: BankId,
    },
    PhysicalCode(PhysicalCodeError),
    DuplicateMappedEntry {
        entry: ExecutionKey,
    },
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RunnerBankMismatch {
                code_bank,
                runner_bank,
            } => write!(
                f,
                "generated runner for {runner_bank} cannot execute code admitted as {code_bank}"
            ),
            Self::DuplicateBank { bank } => write!(f, "block program already contains {bank}"),
            Self::PhysicalCode(error) => error.fmt(f),
            Self::DuplicateMappedEntry { entry } => {
                write!(f, "block program already contains mapped AOT entry {entry}")
            }
        }
    }
}

impl std::error::Error for ProgramError {}

/// Immutable-code catalog and generated callables registered as one program.
///
/// The maps are private and registration validates both identities before
/// mutating either one. A call is admitted through [`CodeCatalog::resolve`]
/// before the generated function runs, so a broad generated match cannot
/// accidentally make a sparse-bank hole executable.
#[derive(Default)]
pub struct BlockProgram {
    pub(super) code: CodeCatalog,
    pub(super) runners: BTreeMap<BankId, (GeneratedBankFn, Option<ProgramArtifactIdentity>)>,
    pub(super) physical_code: PhysicalCodeCatalog,
    pub(super) mapped_aot: BTreeMap<ExecutionKey, MappedAotBlock>,
    execution_destinations: RefCell<VecDeque<ExecutionDestinationObservation>>,
    execution_destination_history_limit: Option<NonZeroUsize>,
    execution_destination_history_suppressed: bool,
}

impl BlockProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<(), ProgramError> {
        let code_bank = code.id();
        if runner.bank != code_bank {
            return Err(ProgramError::RunnerBankMismatch {
                code_bank,
                runner_bank: runner.bank,
            });
        }
        if self.code.bank(code_bank).is_some()
            || self.runners.contains_key(&code_bank)
            || self.physical_code.contains_bank(code_bank)
        {
            return Err(ProgramError::DuplicateBank { bank: code_bank });
        }
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.runners
            .insert(code_bank, (runner.run, runner.artifact_identity));
        Ok(())
    }

    /// Admit one immutable physical code generation for canonical 32-bit
    /// mapped fetch. Every aligned VA resolved to this `BankId` can execute
    /// immediately through the interpreter fallback; registered mapped AOT
    /// units override individual entries without changing the fetch contract.
    pub fn register_physical_code(&mut self, code: PhysicalCodeBank) -> Result<(), ProgramError> {
        let bank = code.id();
        if self.code.bank(bank).is_some() || self.runners.contains_key(&bank) {
            return Err(ProgramError::DuplicateBank { bank });
        }
        self.physical_code
            .register(code)
            .map_err(ProgramError::PhysicalCode)
    }

    /// Install one fetch-bound generated unit into the main program runner.
    /// The containing physical generation must already be registered so no
    /// optional side catalog can become a second execution authority.
    pub fn register_mapped_aot(&mut self, block: MappedAotBlock) -> Result<(), ProgramError> {
        let entry = ExecutionKey::new(block.bank(), block.entry());
        assert!(
            self.physical_code.contains_bank(block.bank()),
            "mapped AOT entry {entry} has no admitted physical code generation"
        );
        if self.mapped_aot.contains_key(&entry) {
            return Err(ProgramError::DuplicateMappedEntry { entry });
        }
        self.mapped_aot.insert(entry, block);
        Ok(())
    }

    pub fn code(&self) -> &CodeCatalog {
        &self.code
    }

    pub fn physical_code(&self) -> &PhysicalCodeCatalog {
        &self.physical_code
    }

    /// Copy retained execution history in authoritative entry order.
    ///
    /// Resolution and classification do not append here. An observation is
    /// added only after sparse code admission and runner lookup both succeed.
    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.execution_destinations
            .borrow()
            .iter()
            .copied()
            .collect()
    }

    /// Bound diagnostic execution history without changing executable state.
    /// `None` retains the complete history and remains the default required by
    /// certification evidence; a limit retains only the newest observations.
    pub fn set_execution_destination_history_limit(&mut self, limit: Option<NonZeroUsize>) {
        self.execution_destination_history_limit = limit;
        if let Some(limit) = limit {
            let destinations = self.execution_destinations.get_mut();
            while destinations.len() > limit.get() {
                destinations.pop_front();
            }
        }
    }

    /// Enable or suppress diagnostic execution history. Complete history is
    /// enabled by default; suppressing it also clears any retained entries.
    pub fn set_execution_destination_history_enabled(&mut self, enabled: bool) {
        self.execution_destination_history_suppressed = !enabled;
        if !enabled {
            self.execution_destinations.get_mut().clear();
        }
    }

    /// Start a new observation lifetime without changing executable state.
    pub fn clear_execution_destinations(&mut self) {
        self.execution_destinations.get_mut().clear();
    }

    fn observe_execution_destination(&self, observation: ExecutionDestinationObservation) {
        if self.execution_destination_history_suppressed {
            return;
        }
        let mut destinations = self.execution_destinations.borrow_mut();
        destinations.push_back(observation);
        if let Some(limit) = self.execution_destination_history_limit {
            while destinations.len() > limit.get() {
                destinations.pop_front();
            }
        }
    }

    /// Capture the complete immutable guest-code image without native
    /// callable addresses.
    ///
    /// Catalog maps sort bank/AOT identities and bank construction sorts
    /// spans, so equivalent registration order produces byte-identical
    /// evidence. The domain-separated SHA-256 covers every virtual and
    /// physical bank identity, span address, length, instruction word, mapped
    /// entry, translated instruction identity, and runner artifact identity,
    /// all encoded big-endian. Code words alone are insufficient because
    /// registration accepts independently generated native runners.
    pub fn evidence_snapshot(&self) -> BlockProgramEvidenceSnapshot {
        let banks = self
            .code
            .banks
            .values()
            .map(|bank| {
                let runner_artifact_identity = self
                    .runners
                    .get(&bank.id)
                    .and_then(|(_, identity)| *identity)
                    .unwrap_or_else(|| {
                        panic!(
                            "block-program release evidence requires a stable artifact identity for generated runner {}",
                            bank.id
                        )
                    });
                CodeBankEvidenceSnapshot {
                    id: bank.id,
                    runner_artifact_identity,
                    spans: bank
                        .spans
                        .iter()
                        .map(|span| CodeSpanEvidenceSnapshot {
                            vram_start: span.vram_start,
                            words: span.words.clone(),
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let physical_banks = self.physical_code.evidence_snapshot();
        let mapped_aot = self
            .mapped_aot
            .values()
            .map(MappedAotBlock::evidence_snapshot)
            .collect::<Vec<_>>();
        let mut hasher = Sha256::new();
        if physical_banks.is_empty() {
            hasher.update(b"fn64.block-program.identity.v1\0");
        } else {
            hasher.update(b"fn64.block-program.identity.v2\0");
        }
        hasher.update(
            u64::try_from(banks.len())
                .expect("block-program bank count exceeds identity wire")
                .to_be_bytes(),
        );
        for bank in &banks {
            hasher.update(bank.id.get().to_be_bytes());
            hasher.update(bank.runner_artifact_identity.bytes());
            hasher.update(
                u64::try_from(bank.spans.len())
                    .expect("block-program span count exceeds identity wire")
                    .to_be_bytes(),
            );
            for span in &bank.spans {
                hasher.update(span.vram_start.get().to_be_bytes());
                hasher.update(
                    u64::try_from(span.words.len())
                        .expect("block-program instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &span.words {
                    hasher.update(word.to_be_bytes());
                }
            }
        }
        if !physical_banks.is_empty() {
            hasher.update(
                u64::try_from(physical_banks.len())
                    .expect("physical block-program bank count exceeds identity wire")
                    .to_be_bytes(),
            );
            for bank in &physical_banks {
                hasher.update(bank.id.get().to_be_bytes());
                hasher.update(
                    u64::try_from(bank.spans.len())
                        .expect("physical block-program span count exceeds identity wire")
                        .to_be_bytes(),
                );
                for span in &bank.spans {
                    hasher.update(span.physical_start.to_be_bytes());
                    hasher.update(
                        u64::try_from(span.words.len())
                            .expect("physical block-program word count exceeds identity wire")
                            .to_be_bytes(),
                    );
                    for word in &span.words {
                        hasher.update(word.to_be_bytes());
                    }
                }
            }
            hasher.update(
                u64::try_from(mapped_aot.len())
                    .expect("mapped AOT unit count exceeds identity wire")
                    .to_be_bytes(),
            );
            for unit in &mapped_aot {
                hasher.update(unit.entry.bank.get().to_be_bytes());
                hasher.update(unit.entry.pc.get().to_be_bytes());
                hasher.update(unit.runner_artifact_identity.bytes());
                hasher.update(
                    u64::try_from(unit.instructions.len())
                        .expect("mapped AOT instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for instruction in &unit.instructions {
                    hasher.update(instruction.bank.get().to_be_bytes());
                    hasher.update(instruction.physical_address.to_be_bytes());
                }
                hasher.update(
                    u64::try_from(unit.expected_words.len())
                        .expect("mapped AOT expected-word count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &unit.expected_words {
                    hasher.update(word.to_be_bytes());
                }
            }
        }
        BlockProgramEvidenceSnapshot {
            identity: ProgramIdentityEvidenceSnapshot {
                identity: ProgramArtifactIdentity::new(hasher.finalize().into()),
                source: ProgramIdentitySource::CanonicalBlockProgramSha256,
            },
            banks,
            physical_banks,
            mapped_aot,
        }
    }

    /// Atomically retire one immutable code generation and its callable.
    /// Returning `false` means neither half existed; a one-sided presence is
    /// an internal invariant violation rather than a recoverable stale state.
    pub fn unregister(&mut self, bank: BankId) -> bool {
        if let Some(_physical) = self.physical_code.unregister(bank) {
            self.mapped_aot.retain(|entry, _| entry.bank != bank);
            return true;
        }
        let code = self.code.unregister(bank);
        let runner = self.runners.remove(&bank);
        assert_eq!(
            code.is_some(),
            runner.is_some(),
            "block program generation {bank} existed in only one ownership map"
        );
        code.is_some()
    }

    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        #[cfg(feature = "dev-interpreter")]
        {
            let mut no_mmio = NoMmio;
            return self.run_with_memory_port(
                entry,
                budget,
                ctx,
                mem,
                &mut MemoryPort::mmio_only(&mut no_mmio),
            );
        }
        #[cfg(not(feature = "dev-interpreter"))]
        self.run_without_dynamic_memory_port(entry, budget, ctx, mem)
    }

    #[cfg(not(feature = "dev-interpreter"))]
    fn run_without_dynamic_memory_port(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        self.run_inner(entry, budget, ctx, mem, None)
    }

    /// [`Self::run`] with a composed memory port for mapped interpreter
    /// fallback units. Static and mapped-AOT runners remain unchanged.
    #[cfg(feature = "dev-interpreter")]
    pub fn run_with_memory_port(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut MemoryPort<'_>,
    ) -> BlockRun {
        self.run_inner(entry, budget, ctx, mem, Some(port))
    }

    fn run_inner(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        #[cfg(feature = "dev-interpreter")] mut port: Option<&mut MemoryPort<'_>>,
        #[cfg(not(feature = "dev-interpreter"))] _port: Option<()>,
    ) -> BlockRun {
        if self.physical_code.contains_bank(entry.bank) {
            if let Some(block) = self.mapped_aot.get(&entry) {
                if let Err(run) = block.preflight(&self.physical_code, ctx) {
                    return run;
                }
                let result = block.run_preflighted(budget, ctx, mem);
                self.observe_execution_destination(ExecutionDestinationObservation {
                    destination: entry,
                    runner_artifact_identity: block.runner_artifact_identity(),
                    instructions: result.instructions,
                });
                return result;
            }
            #[cfg(not(feature = "dev-interpreter"))]
            return BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::MissingAotEntry,
                }),
                0,
            );
            #[cfg(feature = "dev-interpreter")]
            {
                let unit = match admit_mapped_unit(&self.physical_code, entry.bank, entry.pc, ctx) {
                    Ok(unit) => unit,
                    Err(run) => return run,
                };
                let result = match port.as_deref_mut() {
                    Some(port) => {
                        run_admitted_mapped_unit_with_memory_port(unit, budget, ctx, mem, port)
                    }
                    None => run_admitted_mapped_unit(unit, budget, ctx, mem),
                }
                .unwrap_or_else(|unsupported| {
                    BlockRun::new(BlockExit::Fault(unsupported.into_cpu_fault()), 0)
                });
                self.observe_execution_destination(ExecutionDestinationObservation {
                    destination: entry,
                    runner_artifact_identity: None,
                    instructions: result.instructions,
                });
                return result;
            }
        }
        if let Err(fault) = self.code.resolve(entry) {
            let attempted_fetch = u32::from(matches!(fault.kind, CpuFaultKind::Exception { .. }));
            return BlockRun::new(BlockExit::Fault(fault), attempted_fetch);
        }
        let (run, runner_artifact_identity) =
            self.runners.get(&entry.bank).copied().unwrap_or_else(|| {
                panic!(
                    "block program invariant violated: admitted {} has no generated runner",
                    entry.bank
                )
            });
        let result = run(entry, budget, ctx, mem);
        if !matches!(result.exit, BlockExit::ImageChanged { .. }) {
            self.observe_execution_destination(ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity,
                instructions: result.instructions,
            });
        }
        result
    }

    /// Run the registered arbitrary-PC program through transfers and
    /// synchronous architectural exception entry until execution reaches a
    /// scheduler/device boundary.
    ///
    /// Exception vectors are virtual addresses, so they go through the same
    /// active-mapping resolver as computed transfers. CP0 state is committed
    /// before vector resolution; a missing vector therefore returns the
    /// resolver's mapping fault without erasing the guest exception state.
    pub fn dispatch<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, None, true)
    }

    /// [`Self::dispatch`] with a composed memory port for mapped interpreter
    /// fallback units encountered during the dispatch.
    #[cfg(feature = "dev-interpreter")]
    pub fn dispatch_with_memory_port<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
        port: &mut MemoryPort<'_>,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, Some(port), true)
    }

    /// Dispatch while returning architectural exceptions to the live owner.
    /// Hosts whose typed scheduler replaces libultra's raw thread dispatcher
    /// need to publish fault events and stop the current coroutine themselves;
    /// they must not run a second scheduler through the guest vector first.
    pub fn dispatch_exposing_exceptions<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, None, false)
    }

    /// [`Self::dispatch_exposing_exceptions`] with a composed memory port for
    /// mapped interpreter fallback units.
    #[cfg(feature = "dev-interpreter")]
    pub fn dispatch_exposing_exceptions_with_memory_port<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
        port: &mut MemoryPort<'_>,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, Some(port), false)
    }

    fn dispatch_with_exception_vectoring<V>(
        &self,
        mut entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
        #[cfg(feature = "dev-interpreter")] mut port: Option<&mut MemoryPort<'_>>,
        #[cfg(not(feature = "dev-interpreter"))] _port: Option<()>,
        vector_exceptions: bool,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        let mut instructions = 0u32;
        let mut blocks = 0u32;

        loop {
            let remaining = budget.get() - instructions;
            if remaining < InstructionBudget::MIN {
                return Ok(DispatchRun {
                    exit: BlockExit::Checkpoint(entry),
                    instructions,
                    blocks,
                });
            }
            let turn_budget = InstructionBudget::new(remaining)
                .expect("remaining budget was checked against InstructionBudget::MIN");
            #[cfg(feature = "dev-interpreter")]
            let run = match port.as_deref_mut() {
                Some(port) => self.run_with_memory_port(entry, turn_budget, ctx, mem, port),
                None => self.run(entry, turn_budget, ctx, mem),
            };
            #[cfg(not(feature = "dev-interpreter"))]
            let run = self.run(entry, turn_budget, ctx, mem);
            let run = BlockRun::new(
                finalize_executable_write_exit(entry.bank, run.exit),
                run.instructions,
            );
            if run.instructions > remaining {
                return Err(DispatchError::RunnerExceededBudget {
                    at: entry,
                    budget: turn_budget,
                    actual: run.instructions,
                });
            }
            if run.instructions == 0
                && run.exit == BlockExit::Checkpoint(entry)
                && !turn_budget.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
            {
                return Err(DispatchError::IndivisibleUnitExceedsBudget {
                    at: entry,
                    budget: turn_budget,
                    required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
                });
            }
            let continuing_without_progress = run.instructions == 0
                && matches!(
                    run.exit,
                    BlockExit::Checkpoint(_)
                        | BlockExit::Transfer(_)
                        | BlockExit::ResolveTransfer { .. }
                        | BlockExit::ResolveCall { .. }
                        | BlockExit::ExecutableWrite { .. }
                        | BlockExit::ExecutableWriteResolveCall { .. }
                        | BlockExit::ExecutableWriteFault(_)
                        | BlockExit::Fault(CpuFault {
                            kind: CpuFaultKind::Exception { .. },
                            ..
                        })
                );
            if continuing_without_progress {
                return Err(DispatchError::ContinuingExitWithoutProgress {
                    at: entry,
                    exit: run.exit,
                });
            }
            instructions = instructions
                .checked_add(run.instructions)
                .ok_or(DispatchError::InstructionCountOverflow)?;
            blocks = blocks
                .checked_add(1)
                .ok_or(DispatchError::BlockCountOverflow)?;

            let resolution = match run.exit {
                BlockExit::ExecutableWrite {
                    source_bank,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWrite {
                            source_bank,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteResolveCall {
                            source_bank,
                            target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteFault(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteFault(fault),
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ImageChanged { at, miss } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ImageChanged { at, miss },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::Transfer(next) => {
                    entry = next;
                    continue;
                }
                BlockExit::ResolveTransfer {
                    source_bank,
                    target_pc,
                } => resolver.resolve(source_bank, target_pc),
                BlockExit::ResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => match resolver.resolve_call(source_bank, target_pc, resume) {
                    Ok(CallResolution::Guest(next)) => {
                        entry = next;
                        continue;
                    }
                    Ok(CallResolution::Host) => {
                        return Ok(DispatchRun {
                            exit: BlockExit::HostCall {
                                vram: target_pc,
                                resume,
                            },
                            instructions,
                            blocks,
                        });
                    }
                    Err(fault) => Err(fault),
                },
                BlockExit::Fault(fault) => {
                    if !vector_exceptions && matches!(fault.kind, CpuFaultKind::Exception { .. }) {
                        return Ok(DispatchRun {
                            exit: BlockExit::Fault(fault),
                            instructions,
                            blocks,
                        });
                    }
                    let Some(vector) = fault.enter_exception(ctx) else {
                        return Ok(DispatchRun {
                            exit: run.exit,
                            instructions,
                            blocks,
                        });
                    };
                    resolver.resolve(fault.at.bank, vector)
                }
                exit => {
                    return Ok(DispatchRun {
                        exit,
                        instructions,
                        blocks,
                    });
                }
            };

            match resolution {
                Ok(next) => entry = next,
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            }
        }
    }
}
