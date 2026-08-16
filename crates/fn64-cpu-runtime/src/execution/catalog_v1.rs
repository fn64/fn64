//! Catalog-bound block programs and executable-region rewrite.
//! Split from the execution module body purely by size.

use super::*;

/// Failure to bind a [`BlockProgram`] to one canonical catalog entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogBlockProgramErrorV1 {
    EntryNotAdmitted(CpuFault),
    MissingRunnerArtifactIdentity { bank: BankId },
    NonCanonicalProgramEvidence,
    GeneratedRunnerSourceAttestation(GeneratedRunnerSourceAttestationErrorV1),
}

impl fmt::Display for CatalogBlockProgramErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryNotAdmitted(fault) => {
                write!(
                    formatter,
                    "catalog block-program entry is not admitted: {fault}"
                )
            }
            Self::MissingRunnerArtifactIdentity { bank } => write!(
                formatter,
                "catalog block-program runner {bank} has no stable artifact identity"
            ),
            Self::NonCanonicalProgramEvidence => write!(
                formatter,
                "catalog block-program evidence is not canonically derived"
            ),
            Self::GeneratedRunnerSourceAttestation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CatalogBlockProgramErrorV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratedRunnerSourceAttestationErrorV1 {
    ZeroSourceDigest { field: &'static str },
    EmitterSourceReceiptMismatch,
    NonVirtualExecutionNotAttested,
    RunnerBindingCount { expected: usize, actual: usize },
    DuplicateRunnerBinding { bank: BankId },
    MissingRunnerBinding { bank: BankId },
    UnknownRunnerBinding { bank: BankId },
    EmptyCompositeRunner { bank: BankId },
    RunnerArtifactMismatch { bank: BankId },
}

impl fmt::Display for GeneratedRunnerSourceAttestationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSourceDigest { field } => {
                write!(formatter, "generated-runner source digest {field} is zero")
            }
            Self::EmitterSourceReceiptMismatch => formatter.write_str(
                "generated-runner external emitter or linked runtime source receipt mismatch",
            ),
            Self::NonVirtualExecutionNotAttested => formatter.write_str(
                "generated-runner source attestation v2 admits only virtual CodeBank runners",
            ),
            Self::RunnerBindingCount { expected, actual } => write!(
                formatter,
                "generated-runner source binding count {actual} does not match program runner count {expected}"
            ),
            Self::DuplicateRunnerBinding { bank } => {
                write!(formatter, "generated-runner source bindings repeat {bank}")
            }
            Self::MissingRunnerBinding { bank } => {
                write!(formatter, "generated-runner source binding is missing for {bank}")
            }
            Self::UnknownRunnerBinding { bank } => {
                write!(formatter, "generated-runner source binding names unknown {bank}")
            }
            Self::EmptyCompositeRunner { bank } => write!(
                formatter,
                "generated-runner source binding for {bank} contains zero emitted subrunners"
            ),
            Self::RunnerArtifactMismatch { bank } => write!(
                formatter,
                "generated-runner source/adapter identity does not match installed artifact for {bank}"
            ),
        }
    }
}

impl std::error::Error for GeneratedRunnerSourceAttestationErrorV1 {}

/// One canonical, fixed-entry execution substrate for a future ABI install.
///
/// Construction captures the existing pointer-independent program evidence
/// and the feature receipt compiled into this crate. The wrapper deliberately
/// exposes neither `BlockProgram` mutation nor transfer-resolver dispatch: a
/// replacement must arrive as a complete independently constructed program
/// and pass the same admission/evidence checks before the old one is retired.
pub struct CatalogBlockProgramV1 {
    program: BlockProgram,
    entry: ExecutionKey,
    budget: InstructionBudget,
    evidence: BlockProgramEvidenceSnapshot,
    build_receipt: StaticExecutionBuildReceipt,
    generated_runner_source_attestation: Option<GeneratedRunnerSourceAttestationV2>,
}

impl CatalogBlockProgramV1 {
    pub fn new(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        Self::new_inner(program, entry, budget, None)
    }

    /// Construct a catalog with a checked pointer-free Cargo-source
    /// attestation. This validates source, role, and program agreement but is
    /// intentionally not generated-runner semantics authority: any caller can
    /// still pair an arbitrary `GeneratedBankFn` with matching public fields.
    pub fn new_with_cargo_generated_runner_source_attestation_v2(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
        sources: CargoGeneratedProgramSourceAttestationV2<'_>,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        let attestation = Self::validate_generated_runner_source_attestation(&program, sources)
            .map_err(CatalogBlockProgramErrorV1::GeneratedRunnerSourceAttestation)?;
        Self::new_inner(program, entry, budget, Some(attestation))
    }

    fn new_inner(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
        generated_runner_source_attestation: Option<GeneratedRunnerSourceAttestationV2>,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        Self::validate_entry(&program, entry)?;
        for (&bank, (_, artifact_identity)) in &program.runners {
            if artifact_identity.is_none() {
                return Err(CatalogBlockProgramErrorV1::MissingRunnerArtifactIdentity { bank });
            }
        }
        let evidence = program.evidence_snapshot();
        if evidence.identity.source != ProgramIdentitySource::CanonicalBlockProgramSha256 {
            return Err(CatalogBlockProgramErrorV1::NonCanonicalProgramEvidence);
        }
        Ok(Self {
            program,
            entry,
            budget,
            evidence,
            build_receipt: static_execution_build_receipt(),
            generated_runner_source_attestation,
        })
    }

    fn validate_generated_runner_source_attestation(
        program: &BlockProgram,
        sources: CargoGeneratedProgramSourceAttestationV2<'_>,
    ) -> Result<GeneratedRunnerSourceAttestationV2, GeneratedRunnerSourceAttestationErrorV1> {
        for (field, digest) in [
            (
                "root_adapter_source_sha256",
                sources.root_adapter_source_sha256,
            ),
            (
                "shard_cargo_source_tree_sha256",
                sources.shard_cargo_source_tree_sha256,
            ),
        ] {
            if digest == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest { field });
            }
        }
        if sources.expected_emitter_source_sha256 == [0; 32] {
            return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                field: "expected_emitter_source_sha256",
            });
        }
        if sources.expected_emitter_source_sha256
            != sources.externally_measured_emitter_source_sha256
        {
            return Err(GeneratedRunnerSourceAttestationErrorV1::EmitterSourceReceiptMismatch);
        }
        let linked_runtime = generated_runner_runtime_source_receipt_v1();
        if sources.runtime_source_receipt != linked_runtime
            || sources.expected_runtime_source_sha256 != linked_runtime.source_sha256()
        {
            return Err(GeneratedRunnerSourceAttestationErrorV1::EmitterSourceReceiptMismatch);
        }
        if !program.physical_code.evidence_snapshot().is_empty() || !program.mapped_aot.is_empty() {
            return Err(GeneratedRunnerSourceAttestationErrorV1::NonVirtualExecutionNotAttested);
        }
        if sources.runners.len() != program.runners.len() {
            return Err(
                GeneratedRunnerSourceAttestationErrorV1::RunnerBindingCount {
                    expected: program.runners.len(),
                    actual: sources.runners.len(),
                },
            );
        }

        let mut bindings = sources.runners.to_vec();
        bindings.sort_unstable_by_key(|binding| binding.bank);
        for pair in bindings.windows(2) {
            if pair[0].bank == pair[1].bank {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::DuplicateRunnerBinding {
                        bank: pair[0].bank,
                    },
                );
            }
        }
        for (&bank, (_, artifact_identity)) in &program.runners {
            let binding = bindings
                .binary_search_by_key(&bank, |binding| binding.bank)
                .ok()
                .map(|index| bindings[index])
                .ok_or(GeneratedRunnerSourceAttestationErrorV1::MissingRunnerBinding { bank })?;
            if binding.generated_runner_source_sha256 == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                    field: "generated_runner_source_sha256",
                });
            }
            if binding.code_words_sha256 == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                    field: "code_words_sha256",
                });
            }
            if binding.composite_subrunner_count == 0 {
                return Err(GeneratedRunnerSourceAttestationErrorV1::EmptyCompositeRunner { bank });
            }
            let expected_artifact = ProgramArtifactIdentity::generated_adapter(
                sources.root_adapter_source_sha256,
                binding.generated_runner_source_sha256,
                bank,
                binding.adapter_role,
            );
            if *artifact_identity != Some(expected_artifact) {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::RunnerArtifactMismatch { bank },
                );
            }
            let code = program
                .code
                .bank(bank)
                .expect("every registered runner has one atomically registered code bank");
            let mut code_hasher = Sha256::new();
            for span in code.spans() {
                for word in span.words() {
                    code_hasher.update(word.to_be_bytes());
                }
            }
            let actual_code_sha256: [u8; 32] = code_hasher.finalize().into();
            if code.vram_start() != binding.vram_start
                || code.vram_end() != binding.vram_end
                || actual_code_sha256 != binding.code_words_sha256
            {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::RunnerArtifactMismatch { bank },
                );
            }
        }
        for binding in &bindings {
            if !program.runners.contains_key(&binding.bank) {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::UnknownRunnerBinding {
                        bank: binding.bank,
                    },
                );
            }
        }

        let evidence = program.evidence_snapshot();
        let mut binding_hasher = Sha256::new();
        binding_hasher.update(GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2);
        binding_hasher.update(evidence.identity.identity.bytes());
        binding_hasher.update(sources.root_adapter_source_sha256);
        binding_hasher.update(sources.shard_cargo_source_tree_sha256);
        binding_hasher.update(sources.externally_measured_emitter_source_sha256);
        binding_hasher.update(linked_runtime.source_sha256());
        for binding in bindings {
            binding_hasher.update(binding.bank.get().to_be_bytes());
            binding_hasher.update(binding.generated_runner_source_sha256);
            binding_hasher.update(binding.code_words_sha256);
            binding_hasher.update(binding.vram_start.get().to_be_bytes());
            binding_hasher.update(binding.vram_end.get().to_be_bytes());
            binding_hasher.update(binding.composite_subrunner_count.to_be_bytes());
            binding_hasher.update([binding.adapter_role.tag()]);
        }
        let build_receipt = static_execution_build_receipt();
        binding_hasher.update(build_receipt.schema.to_be_bytes());
        binding_hasher.update([
            u8::from(build_receipt.aot_runtime),
            u8::from(build_receipt.production_aot),
            u8::from(build_receipt.dev_interpreter),
        ]);
        Ok(GeneratedRunnerSourceAttestationV2 {
            schema: GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2,
            cargo_source_fields_validated: true,
            program_identity: evidence.identity.identity,
            root_adapter_source_sha256: sources.root_adapter_source_sha256,
            shard_cargo_source_tree_sha256: sources.shard_cargo_source_tree_sha256,
            emitter_source_sha256: sources.externally_measured_emitter_source_sha256,
            runtime_source_sha256: linked_runtime.source_sha256(),
            binding_sha256: binding_hasher.finalize().into(),
            build_receipt,
        })
    }

    fn validate_entry(
        program: &BlockProgram,
        entry: ExecutionKey,
    ) -> Result<(), CatalogBlockProgramErrorV1> {
        if !entry.pc.is_instruction_aligned() {
            return Err(CatalogBlockProgramErrorV1::EntryNotAdmitted(
                CpuFault::instruction_address_error(entry),
            ));
        }
        if program.physical_code.contains_bank(entry.bank) {
            return program.mapped_aot.contains_key(&entry).then_some(()).ok_or(
                CatalogBlockProgramErrorV1::EntryNotAdmitted(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::MissingAotEntry,
                }),
            );
        }
        program
            .code
            .resolve(entry)
            .map(|_| ())
            .map_err(CatalogBlockProgramErrorV1::EntryNotAdmitted)
    }

    pub const fn entry(&self) -> ExecutionKey {
        self.entry
    }

    pub const fn budget(&self) -> InstructionBudget {
        self.budget
    }

    pub const fn identity(&self) -> ProgramIdentityEvidenceSnapshot {
        self.evidence.identity
    }

    pub fn evidence(&self) -> &BlockProgramEvidenceSnapshot {
        &self.evidence
    }

    pub const fn build_receipt(&self) -> StaticExecutionBuildReceipt {
        self.build_receipt
    }

    pub const fn generated_runner_source_attestation(
        &self,
    ) -> Option<&GeneratedRunnerSourceAttestationV2> {
        self.generated_runner_source_attestation.as_ref()
    }

    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.program.copy_execution_destinations()
    }

    /// Whether the immutable static install owns this bank identity through
    /// either virtual code or the physical mapped-code catalog.
    ///
    /// Dynamic operational catalogs use this complete query to keep their
    /// content-derived identities disjoint from every static execution lane.
    pub fn reserves_bank(&self, bank: BankId) -> bool {
        self.program.code.bank(bank).is_some() || self.program.physical_code.contains_bank(bank)
    }

    /// Whether either the immutable program or any precompiled generation,
    /// including an inactive generation, reserves this bank identity.
    pub fn reserves_bank_with_generations(
        &self,
        bank: BankId,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> bool {
        self.reserves_bank(bank) || generations.contains_reserved_bank(bank)
    }

    /// Resolve a static virtual entry without preferring the wrapper's entry
    /// bank. The owned entry bank is used only to retain typed fault context.
    pub fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        self.program.code.resolve_entry(self.entry.bank, target_pc)
    }

    /// Resolve a static virtual transfer with exact source-bank preference.
    /// Active physical/dynamic generation selection remains an outer owner.
    pub fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program.code.resolve_transfer(source_bank, target_pc)
    }

    pub fn validate_precompiled_generations(
        &self,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<(), GenerationCatalogError> {
        generations.validate_program(&self.program)
    }

    pub fn resolve_entry_with_generations(
        &self,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                self.entry.bank,
                target_pc,
            )));
        }
        match generations.resolve_active(target_pc) {
            Ok(entry) => Ok(entry),
            Err(crate::generation::GenerationLookupError::NoActiveGeneration { .. }) => {
                Err(CpuFault {
                    at: ExecutionKey::new(self.entry.bank, target_pc),
                    kind: CpuFaultKind::NoActiveGeneration,
                })
            }
            Err(crate::generation::GenerationLookupError::UnmappedPc { .. }) => self
                .program
                .code
                .resolve_entry_where(self.entry.bank, target_pc, |bank| {
                    !generations.contains_reserved_bank(bank)
                }),
            Err(error) => unreachable!("resolve_active returned activation-time error: {error}"),
        }
    }

    pub fn resolve_transfer_with_generations(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        match generations.resolve_active(target_pc) {
            Ok(entry) => Ok(entry),
            Err(crate::generation::GenerationLookupError::NoActiveGeneration { .. }) => {
                Err(CpuFault {
                    at: ExecutionKey::new(source_bank, target_pc),
                    kind: CpuFaultKind::NoActiveGeneration,
                })
            }
            Err(crate::generation::GenerationLookupError::UnmappedPc { .. }) => self
                .program
                .code
                .resolve_transfer_where(source_bank, target_pc, |bank| {
                    !generations.contains_reserved_bank(bank)
                }),
            Err(error) => unreachable!("resolve_active returned activation-time error: {error}"),
        }
    }

    /// Execute exactly the entry and budget owned by this substrate. Transfer
    /// resolution remains an outer ABI responsibility and is not accepted as
    /// a callback here.
    pub fn run(&self, ctx: &mut RecompContext, mem: &mut Rdram<'_>) -> BlockRun {
        self.program.run(self.entry, self.budget, ctx, mem)
    }

    /// Dispatch an arbitrary admitted continuation using only this owned
    /// static program and one exact host-function catalog. No resolver
    /// callback or ambient host lookup participates in the decision.
    pub fn dispatch_exposing_exceptions_at(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        self.dispatch_exposing_exceptions_at_budget(entry, hosts, self.budget, ctx, mem)
    }

    /// Dispatch with a caller-owned slice budget. This is the budget-preserving
    /// seam used when static and dynamic execution share one architectural
    /// checkpoint; it does not mutate the install's configured outer budget.
    pub fn dispatch_exposing_exceptions_at_budget(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        let mut resolver = CatalogStaticTransferResolverV1 {
            program: self,
            hosts,
        };
        self.program
            .dispatch_exposing_exceptions(entry, budget, ctx, mem, &mut resolver)
    }

    pub fn dispatch_exposing_exceptions_with_generations_at(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        generations: &BackedPrecompiledGenerationCatalogV1,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        self.dispatch_exposing_exceptions_with_generations_at_budget(
            entry,
            hosts,
            generations,
            self.budget,
            ctx,
            mem,
        )
    }

    pub fn dispatch_exposing_exceptions_with_generations_at_budget(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        generations: &BackedPrecompiledGenerationCatalogV1,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        let mut resolver = CatalogGenerationTransferResolverV1 {
            program: self,
            hosts,
            generations,
        };
        self.program
            .dispatch_exposing_exceptions(entry, budget, ctx, mem, &mut resolver)
    }

    pub fn set_entry(&mut self, entry: ExecutionKey) -> Result<(), CatalogBlockProgramErrorV1> {
        Self::validate_entry(&self.program, entry)?;
        self.entry = entry;
        Ok(())
    }

    pub fn set_budget(&mut self, budget: InstructionBudget) {
        self.budget = budget;
    }

    /// Atomically replace the complete program and its entry. Validation and
    /// canonical evidence capture finish before the installed substrate is
    /// changed.
    pub fn replace_program(
        &mut self,
        program: BlockProgram,
        entry: ExecutionKey,
    ) -> Result<(), CatalogBlockProgramErrorV1> {
        let replacement = Self::new(program, entry, self.budget)?;
        *self = replacement;
        Ok(())
    }
}

struct CatalogStaticTransferResolverV1<'a> {
    program: &'a CatalogBlockProgramV1,
    hosts: &'a HostFunctionCatalogV1,
}

impl TransferResolver for CatalogStaticTransferResolverV1<'_> {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if self.hosts.resolve(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

struct CatalogGenerationTransferResolverV1<'a> {
    program: &'a CatalogBlockProgramV1,
    hosts: &'a HostFunctionCatalogV1,
    generations: &'a BackedPrecompiledGenerationCatalogV1,
}

impl TransferResolver for CatalogGenerationTransferResolverV1<'_> {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program
            .resolve_transfer_with_generations(source_bank, target_pc, self.generations)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if self.hosts.resolve(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

/// Failure to publish a new executable generation into a fixed virtual
/// region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationError {
    RegionMismatch {
        region_start: GuestPc,
        region_end: GuestPc,
        bank_start: GuestPc,
        bank_end: GuestPc,
    },
    Program(ProgramError),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RegionMismatch {
                region_start,
                region_end,
                bank_start,
                bank_end,
            } => write!(
                f,
                "executable generation [{bank_start}, {bank_end}) does not exactly replace region [{region_start}, {region_end})"
            ),
            Self::Program(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for GenerationError {}

/// One virtual code region with exactly one active immutable generation.
///
/// Installing a replacement removes the old `CodeBank` and generated runner
/// together before publishing the new pair. The region therefore never
/// resolves stale code by virtual address after a successful rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutableRegion {
    start: GuestPc,
    end: GuestPc,
    active: Option<BankId>,
}

impl ExecutableRegion {
    pub fn new(start: GuestPc, end: GuestPc) -> Self {
        assert!(start < end, "executable region must be nonempty");
        assert!(
            start.is_instruction_aligned() && end.is_instruction_aligned(),
            "executable region bounds must be instruction-aligned"
        );
        Self {
            start,
            end,
            active: None,
        }
    }

    pub const fn active_bank(self) -> Option<BankId> {
        self.active
    }

    pub const fn start(self) -> GuestPc {
        self.start
    }

    pub const fn end(self) -> GuestPc {
        self.end
    }

    pub fn resolve(self, pc: GuestPc) -> Option<ExecutionKey> {
        if pc < self.start || pc >= self.end {
            return None;
        }
        self.active.map(|bank| ExecutionKey::new(bank, pc))
    }

    pub fn install(
        &mut self,
        program: &mut BlockProgram,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<Option<BankId>, GenerationError> {
        if code.vram_start() != self.start || code.vram_end() != self.end {
            return Err(GenerationError::RegionMismatch {
                region_start: self.start,
                region_end: self.end,
                bank_start: code.vram_start(),
                bank_end: code.vram_end(),
            });
        }
        let bank = code.id();
        if runner.bank() != bank {
            return Err(GenerationError::Program(ProgramError::RunnerBankMismatch {
                code_bank: bank,
                runner_bank: runner.bank(),
            }));
        }
        if program.code().bank(bank).is_some() {
            return Err(GenerationError::Program(ProgramError::DuplicateBank {
                bank,
            }));
        }

        let retired = self.active;
        if let Some(previous) = retired {
            assert!(
                program.unregister(previous),
                "active executable region referenced missing generation {previous}"
            );
        }
        program
            .register(code, runner)
            .map_err(GenerationError::Program)?;
        self.active = Some(bank);
        Ok(retired)
    }
}
