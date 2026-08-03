//! Content-addressed selection of immutable, precompiled executable images.
//!
//! All generations and native runners are registered before execution.  This
//! module owns only the live virtual mapping: an attempted fetch hashes the
//! completed guest image, selects an exact admitted digest, and publishes that
//! generation while retiring intersecting publications.  A retired AOT image
//! remains in the catalog, so an A -> B -> A materialization reuses the first
//! native artifact rather than recompiling it.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::{AotMiss, BankId, BlockProgram, ExecutionKey, GuestPc, Rdram};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationId(u64);

impl GenerationId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for GenerationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "generation:{:016X}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrecompiledShard {
    bank: BankId,
    start: GuestPc,
    end: GuestPc,
}

impl PrecompiledShard {
    pub fn new(bank: BankId, start: GuestPc, end: GuestPc) -> Result<Self, GenerationCatalogError> {
        if start >= end || !start.is_instruction_aligned() || !end.is_instruction_aligned() {
            return Err(GenerationCatalogError::InvalidRange { start, end });
        }
        Ok(Self { bank, start, end })
    }

    pub const fn bank(self) -> BankId {
        self.bank
    }

    pub const fn start(self) -> GuestPc {
        self.start
    }

    pub const fn end(self) -> GuestPc {
        self.end
    }

    fn contains(self, pc: GuestPc) -> bool {
        self.start <= pc && pc < self.end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompiledGeneration {
    id: GenerationId,
    image_start: GuestPc,
    image_end: GuestPc,
    invalidation_start: GuestPc,
    invalidation_end: GuestPc,
    expected_sha256: [u8; 32],
    shards: Vec<PrecompiledShard>,
}

impl PrecompiledGeneration {
    pub fn new(
        id: GenerationId,
        image_start: GuestPc,
        image_end: GuestPc,
        invalidation_start: GuestPc,
        invalidation_end: GuestPc,
        expected_sha256: [u8; 32],
        mut shards: Vec<PrecompiledShard>,
    ) -> Result<Self, GenerationCatalogError> {
        if image_start >= image_end
            || invalidation_start >= invalidation_end
            || !image_start.is_instruction_aligned()
            || !image_end.is_instruction_aligned()
            || !invalidation_start.is_instruction_aligned()
            || !invalidation_end.is_instruction_aligned()
        {
            return Err(GenerationCatalogError::InvalidRange {
                start: image_start,
                end: image_end,
            });
        }
        if invalidation_start > image_start || invalidation_end < image_end {
            return Err(GenerationCatalogError::InvalidationDoesNotContainImage {
                image_start,
                image_end,
                invalidation_start,
                invalidation_end,
            });
        }
        shards.sort_unstable_by_key(|shard| (shard.start, shard.end, shard.bank));
        let mut cursor = image_start;
        for shard in &shards {
            if shard.start != cursor || shard.end > image_end {
                return Err(GenerationCatalogError::ShardCoverage {
                    expected_start: cursor,
                    actual_start: shard.start,
                    actual_end: shard.end,
                });
            }
            cursor = shard.end;
        }
        if cursor != image_end {
            return Err(GenerationCatalogError::ShardCoverage {
                expected_start: cursor,
                actual_start: cursor,
                actual_end: cursor,
            });
        }
        Ok(Self {
            id,
            image_start,
            image_end,
            invalidation_start,
            invalidation_end,
            expected_sha256,
            shards,
        })
    }

    pub const fn id(&self) -> GenerationId {
        self.id
    }

    pub const fn image_start(&self) -> GuestPc {
        self.image_start
    }

    pub const fn image_end(&self) -> GuestPc {
        self.image_end
    }

    pub const fn invalidation_start(&self) -> GuestPc {
        self.invalidation_start
    }

    pub const fn invalidation_end(&self) -> GuestPc {
        self.invalidation_end
    }

    pub const fn expected_sha256(&self) -> [u8; 32] {
        self.expected_sha256
    }

    pub fn shards(&self) -> &[PrecompiledShard] {
        &self.shards
    }

    fn contains(&self, pc: GuestPc) -> bool {
        self.image_start <= pc && pc < self.image_end
    }

    fn byte_len(&self) -> u32 {
        self.image_end.get() - self.image_start.get()
    }

    fn key(&self, pc: GuestPc) -> ExecutionKey {
        let shard = self
            .shards
            .iter()
            .copied()
            .find(|shard| shard.contains(pc))
            .expect("validated shard union covers every image PC");
        ExecutionKey::new(shard.bank, pc)
    }

    fn intersects_invalidation(&self, other: &Self) -> bool {
        self.invalidation_start < other.invalidation_end
            && other.invalidation_start < self.invalidation_end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationCatalogError {
    InvalidRange {
        start: GuestPc,
        end: GuestPc,
    },
    InvalidationDoesNotContainImage {
        image_start: GuestPc,
        image_end: GuestPc,
        invalidation_start: GuestPc,
        invalidation_end: GuestPc,
    },
    ShardCoverage {
        expected_start: GuestPc,
        actual_start: GuestPc,
        actual_end: GuestPc,
    },
    DuplicateGeneration {
        id: GenerationId,
    },
    DuplicateBank {
        bank: BankId,
    },
    AmbiguousImageIdentity {
        first: GenerationId,
        second: GenerationId,
    },
    MissingShardBank {
        generation: GenerationId,
        bank: BankId,
    },
    ShardBankGeometry {
        generation: GenerationId,
        bank: BankId,
        expected_start: GuestPc,
        expected_end: GuestPc,
        actual_start: GuestPc,
        actual_end: GuestPc,
        actual_spans: usize,
    },
    StaticBankOverlapsGenerationOwnership {
        bank: BankId,
        span_start: GuestPc,
        span_end: GuestPc,
        generation: GenerationId,
        invalidation_start: GuestPc,
        invalidation_end: GuestPc,
    },
}

impl fmt::Display for GenerationCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid precompiled generation catalog: {self:?}"
        )
    }
}

impl std::error::Error for GenerationCatalogError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationLookupError {
    UnmappedPc {
        pc: GuestPc,
    },
    NoActiveGeneration {
        pc: GuestPc,
    },
    AmbiguousLiveImage {
        pc: GuestPc,
        first: GenerationId,
        second: GenerationId,
    },
    AotMiss(AotMiss),
}

impl fmt::Display for GenerationLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnmappedPc { pc } => write!(formatter, "no precompiled generation contains {pc}"),
            Self::NoActiveGeneration { pc } => {
                write!(formatter, "no precompiled generation is active at {pc}")
            }
            Self::AmbiguousLiveImage { pc, first, second } => write!(
                formatter,
                "live image at {pc} matches both {first} and {second}"
            ),
            Self::AotMiss(miss) => miss.fmt(formatter),
        }
    }
}

impl std::error::Error for GenerationLookupError {}

/// One contiguous piece of an explicitly admitted executable VA-to-physical
/// mapping. Multiple spans permit page-remapped images without reconstructing
/// KSEG addresses or assuming one VA/PA delta for the whole image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackedExecutableSpanV1 {
    virtual_start: GuestPc,
    physical_start: u32,
    byte_len: u32,
}

impl BackedExecutableSpanV1 {
    pub fn new(
        virtual_start: GuestPc,
        physical_start: u32,
        byte_len: u32,
    ) -> Result<Self, BackedGenerationCatalogErrorV1> {
        let virtual_end = virtual_start.get().checked_add(byte_len);
        let physical_end = physical_start.checked_add(byte_len);
        if byte_len == 0
            || !byte_len.is_multiple_of(4)
            || !virtual_start.is_instruction_aligned()
            || !physical_start.is_multiple_of(4)
            || virtual_end.is_none()
            || physical_end.is_none()
            || physical_end.is_some_and(|end| end > crate::runtime::RDRAM_LEN as u32)
        {
            return Err(BackedGenerationCatalogErrorV1::InvalidBackingSpan {
                virtual_start,
                physical_start,
                byte_len,
            });
        }
        Ok(Self {
            virtual_start,
            physical_start,
            byte_len,
        })
    }

    pub const fn virtual_start(self) -> GuestPc {
        self.virtual_start
    }

    pub const fn virtual_end(self) -> GuestPc {
        GuestPc::new(self.virtual_start.get() + self.byte_len)
    }

    pub const fn physical_start(self) -> u32 {
        self.physical_start
    }

    pub const fn physical_end(self) -> u32 {
        self.physical_start + self.byte_len
    }

    pub const fn byte_len(self) -> u32 {
        self.byte_len
    }

    fn physical_at(self, virtual_address: GuestPc) -> Option<u32> {
        (self.virtual_start <= virtual_address && virtual_address < self.virtual_end())
            .then(|| self.physical_start + (virtual_address.get() - self.virtual_start.get()))
    }
}

/// Exact segmented physical backing for one generation's complete virtual
/// invalidation interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompiledGenerationBackingV1 {
    generation: GenerationId,
    spans: Vec<BackedExecutableSpanV1>,
}

impl PrecompiledGenerationBackingV1 {
    pub fn new(
        generation: GenerationId,
        mut spans: Vec<BackedExecutableSpanV1>,
    ) -> Result<Self, BackedGenerationCatalogErrorV1> {
        spans.sort_unstable_by_key(|span| (span.virtual_start, span.virtual_end()));
        let Some(first) = spans.first().copied() else {
            return Err(BackedGenerationCatalogErrorV1::EmptyGenerationBacking { generation });
        };
        let mut cursor = first.virtual_start;
        for span in &spans {
            if span.virtual_start != cursor {
                return Err(BackedGenerationCatalogErrorV1::BackingCoverageGap {
                    generation,
                    expected_start: cursor,
                    actual_start: span.virtual_start,
                });
            }
            cursor = span.virtual_end();
        }
        Ok(Self { generation, spans })
    }

    pub const fn generation(&self) -> GenerationId {
        self.generation
    }

    pub fn spans(&self) -> &[BackedExecutableSpanV1] {
        &self.spans
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackedGenerationCatalogErrorV1 {
    InvalidBackingSpan {
        virtual_start: GuestPc,
        physical_start: u32,
        byte_len: u32,
    },
    EmptyGenerationBacking {
        generation: GenerationId,
    },
    BackingCoverageGap {
        generation: GenerationId,
        expected_start: GuestPc,
        actual_start: GuestPc,
    },
    DuplicateGenerationBacking {
        generation: GenerationId,
    },
    UnknownGenerationBacking {
        generation: GenerationId,
    },
    MissingGenerationBacking {
        generation: GenerationId,
    },
    BackingGeometryMismatch {
        generation: GenerationId,
        expected_start: GuestPc,
        expected_end: GuestPc,
        actual_start: GuestPc,
        actual_end: GuestPc,
    },
    InconsistentOverlappingMappings {
        first: GenerationId,
        second: GenerationId,
    },
    InvalidPhysicalWriteRange {
        physical_start: u32,
        physical_end: u32,
    },
}

impl fmt::Display for BackedGenerationCatalogErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid backed precompiled generation catalog: {self:?}"
        )
    }
}

impl std::error::Error for BackedGenerationCatalogErrorV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalInvalidationRangeV1 {
    physical_start: u32,
    physical_end: u32,
}

pub const BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1: &str =
    "fn64.backed-precompiled-generation-catalog.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompiledGenerationEvidenceV1 {
    pub generation: GenerationId,
    pub image_start: GuestPc,
    pub image_end: GuestPc,
    pub invalidation_start: GuestPc,
    pub invalidation_end: GuestPc,
    pub expected_sha256: [u8; 32],
    pub shards: Vec<PrecompiledShard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompiledGenerationBackingEvidenceV1 {
    pub generation: GenerationId,
    pub spans: Vec<BackedExecutableSpanV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackedGenerationCatalogEvidenceV1 {
    pub schema: String,
    pub generations: Vec<PrecompiledGenerationEvidenceV1>,
    pub backings: Vec<PrecompiledGenerationBackingEvidenceV1>,
    pub active_segments: Vec<ActiveGenerationSegment>,
}

impl PhysicalInvalidationRangeV1 {
    pub const fn physical_start(self) -> u32 {
        self.physical_start
    }

    pub const fn physical_end(self) -> u32 {
        self.physical_end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationResolution {
    pub entry: ExecutionKey,
    pub generation: GenerationId,
    pub newly_activated: bool,
    pub retired: Vec<GenerationId>,
}

/// One successful digest-selected activation from a catalog whose complete
/// executable image has validated physical backing.
///
/// This is ephemeral, trace-local observability. It does not prove that the
/// installed catalog is complete, that the generation is statically reachable,
/// or that any unobserved path is covered. A retained diagnostic must join it
/// to the installed catalog's canonical definition identity; this type does
/// not mint or serialize that authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackedGenerationActivationObservationV1 {
    pub requested_pc: GuestPc,
    pub generation: GenerationId,
    pub entry: ExecutionKey,
    /// Admitted image digest proven equal to the complete live digest read
    /// through the generation's validated physical backing.
    pub matched_image_sha256: [u8; 32],
    pub newly_activated: bool,
    pub retired: Vec<GenerationId>,
}

/// Host callback reached only after a physically backed catalog activation
/// has selected and published one exact admitted digest.
pub type BackedGenerationActivationObserverV1 = fn(&BackedGenerationActivationObservationV1);

thread_local! {
    static BACKED_GENERATION_ACTIVATION_OBSERVER_V1:
        std::cell::Cell<Option<BackedGenerationActivationObserverV1>> = const {
            std::cell::Cell::new(None)
        };
}

/// Install the current thread's physically backed generation-activation
/// observer, returning the prior observer for scoped replacement/restoration.
///
/// Notification runs after active-segment publication and before activation
/// returns. Observers are diagnostic hooks: they must not panic or recursively
/// activate a catalog through ambient host state. They may replace or clear
/// themselves because notification invokes a copied function pointer without
/// holding the thread-local slot.
pub fn set_backed_generation_activation_observer_v1(
    observer: Option<BackedGenerationActivationObserverV1>,
) -> Option<BackedGenerationActivationObserverV1> {
    BACKED_GENERATION_ACTIVATION_OBSERVER_V1.with(|slot| slot.replace(observer))
}

fn notify_backed_generation_activation_v1(observation: &BackedGenerationActivationObservationV1) {
    BACKED_GENERATION_ACTIVATION_OBSERVER_V1.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(observation);
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveSegment {
    start: GuestPc,
    end: GuestPc,
    generation: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveGenerationSegment {
    pub start: GuestPc,
    pub end: GuestPc,
    pub generation: GenerationId,
}

#[derive(Default)]
pub struct PrecompiledGenerationCatalog {
    generations: Vec<PrecompiledGeneration>,
    active: Vec<ActiveSegment>,
}

impl PrecompiledGenerationCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        generation: PrecompiledGeneration,
    ) -> Result<(), GenerationCatalogError> {
        if self
            .generations
            .iter()
            .any(|candidate| candidate.id == generation.id)
        {
            return Err(GenerationCatalogError::DuplicateGeneration { id: generation.id });
        }
        if let Some(bank) = generation.shards.iter().find_map(|shard| {
            self.generations
                .iter()
                .flat_map(|candidate| &candidate.shards)
                .any(|known| known.bank == shard.bank)
                .then_some(shard.bank)
        }) {
            return Err(GenerationCatalogError::DuplicateBank { bank });
        }
        if let Some(candidate) = self.generations.iter().find(|candidate| {
            candidate.image_start == generation.image_start
                && candidate.image_end == generation.image_end
                && candidate.expected_sha256 == generation.expected_sha256
        }) {
            return Err(GenerationCatalogError::AmbiguousImageIdentity {
                first: candidate.id,
                second: generation.id,
            });
        }
        self.generations.push(generation);
        self.generations
            .sort_unstable_by_key(|candidate| candidate.id);
        Ok(())
    }

    pub fn active_generations(&self) -> Vec<GenerationId> {
        let mut generations = self
            .active
            .iter()
            .map(|segment| self.generations[segment.generation].id)
            .collect::<Vec<_>>();
        generations.sort_unstable();
        generations.dedup();
        generations
    }

    pub fn generations(&self) -> &[PrecompiledGeneration] {
        &self.generations
    }

    /// Prove that every catalog shard names one already-installed generated
    /// bank with exactly the shard's contiguous executable range.
    ///
    /// Activation is allowed to change only virtual ownership. Validating the
    /// closed catalog before installation prevents a catalog-only identity
    /// from resolving to `UnknownBank` and being retried without progress.
    pub fn validate_program(&self, program: &BlockProgram) -> Result<(), GenerationCatalogError> {
        for generation in &self.generations {
            for shard in &generation.shards {
                let Some(bank) = program.code().bank(shard.bank) else {
                    return Err(GenerationCatalogError::MissingShardBank {
                        generation: generation.id,
                        bank: shard.bank,
                    });
                };
                if bank.spans().len() != 1
                    || bank.vram_start() != shard.start
                    || bank.vram_end() != shard.end
                {
                    return Err(GenerationCatalogError::ShardBankGeometry {
                        generation: generation.id,
                        bank: shard.bank,
                        expected_start: shard.start,
                        expected_end: shard.end,
                        actual_start: bank.vram_start(),
                        actual_end: bank.vram_end(),
                        actual_spans: bank.spans().len(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn active_segments(&self) -> Vec<ActiveGenerationSegment> {
        self.active
            .iter()
            .map(|segment| ActiveGenerationSegment {
                start: segment.start,
                end: segment.end,
                generation: self.generations[segment.generation].id,
            })
            .collect()
    }

    /// Resolve the currently published owner without inspecting mutable guest
    /// bytes. Generated runners preflight their own immutable bytes; an
    /// `ImageChanged` exit is the fetch boundary that calls
    /// [`Self::activate_for_fetch`].
    pub fn resolve_active(&self, pc: GuestPc) -> Result<ExecutionKey, GenerationLookupError> {
        if let Some(segment) = self
            .active
            .iter()
            .find(|segment| segment.start <= pc && pc < segment.end)
        {
            return Ok(self.generations[segment.generation].key(pc));
        }
        if self
            .generations
            .iter()
            .any(|generation| generation.contains(pc))
        {
            Err(GenerationLookupError::NoActiveGeneration { pc })
        } else {
            Err(GenerationLookupError::UnmappedPc { pc })
        }
    }

    /// Hash completed image candidates at an attempted fetch and publish the
    /// unique matching generation. Intersecting active intervals are split,
    /// preserving unaffected fragments and removing stale ownership exactly
    /// over the new generation's declared invalidation extent.
    pub fn activate_for_fetch(
        &mut self,
        pc: GuestPc,
        mem: &Rdram<'_>,
    ) -> Result<GenerationResolution, GenerationLookupError> {
        self.activate_for_fetch_with(pc, |vaddr| {
            mem.load_bu(0xffff_ffff_0000_0000u64 | u64::from(vaddr))
        })
    }

    /// Reader-parametric form for runtime seams that own RDRAM through a raw
    /// storage adapter while a suspended guest coroutine retains its checked
    /// `Rdram` view. The reader receives canonical 32-bit guest virtual byte
    /// addresses; it must apply the same KSEG/TLB mapping as instruction fetch.
    pub fn activate_for_fetch_with(
        &mut self,
        pc: GuestPc,
        mut read_virtual_byte: impl FnMut(u32) -> u8,
    ) -> Result<GenerationResolution, GenerationLookupError> {
        self.activate_for_fetch_with_digest(pc, |generation| {
            live_sha256_with(
                generation.image_start,
                generation.byte_len(),
                &mut read_virtual_byte,
            )
        })
    }

    fn activate_for_fetch_with_digest(
        &mut self,
        pc: GuestPc,
        mut live_digest: impl FnMut(&PrecompiledGeneration) -> [u8; 32],
    ) -> Result<GenerationResolution, GenerationLookupError> {
        let mut containing = self
            .generations
            .iter()
            .enumerate()
            .filter(|(_, generation)| generation.contains(pc))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        containing.sort_unstable_by_key(|index| {
            let generation = &self.generations[*index];
            (generation.byte_len(), generation.id)
        });
        if containing.is_empty() {
            return Err(GenerationLookupError::UnmappedPc { pc });
        }

        let mut matches = Vec::new();
        let mut first_miss = None;
        for index in containing {
            let generation = &self.generations[index];
            let actual_sha256 = live_digest(generation);
            if actual_sha256 == generation.expected_sha256 {
                matches.push(index);
            } else if first_miss.is_none() {
                first_miss = Some(AotMiss {
                    expected_bank: generation.key(pc).bank,
                    va_start: generation.image_start,
                    byte_len: generation.byte_len(),
                    expected_sha256: generation.expected_sha256,
                    actual_sha256,
                });
            }
        }
        let &selected = match matches.as_slice() {
            [] => return Err(GenerationLookupError::AotMiss(first_miss.unwrap())),
            [selected] => selected,
            [first, second, ..] => {
                return Err(GenerationLookupError::AmbiguousLiveImage {
                    pc,
                    first: self.generations[*first].id,
                    second: self.generations[*second].id,
                })
            }
        };

        let selected_generation = &self.generations[selected];
        let already_active = self.active.iter().any(|segment| {
            segment.generation == selected && segment.start <= pc && pc < segment.end
        });
        let mut retired = self
            .active
            .iter()
            .filter(|active| {
                self.generations[active.generation].intersects_invalidation(selected_generation)
            })
            .map(|active| self.generations[active.generation].id)
            .collect::<Vec<_>>();
        let mut replacement = Vec::with_capacity(self.active.len() + 2);
        for active in self.active.drain(..) {
            if active.end <= selected_generation.invalidation_start
                || active.start >= selected_generation.invalidation_end
            {
                replacement.push(active);
                continue;
            }
            if active.start < selected_generation.invalidation_start {
                replacement.push(ActiveSegment {
                    start: active.start,
                    end: selected_generation.invalidation_start,
                    generation: active.generation,
                });
            }
            if active.end > selected_generation.invalidation_end {
                replacement.push(ActiveSegment {
                    start: selected_generation.invalidation_end,
                    end: active.end,
                    generation: active.generation,
                });
            }
        }
        replacement.push(ActiveSegment {
            start: selected_generation.image_start,
            end: selected_generation.image_end,
            generation: selected,
        });
        replacement.sort_unstable_by_key(|segment| (segment.start, segment.end));
        self.active = replacement;
        retired.sort_unstable();
        retired.dedup();
        Ok(GenerationResolution {
            entry: selected_generation.key(pc),
            generation: selected_generation.id,
            newly_activated: !already_active,
            retired,
        })
    }
}

/// Closed generation inventory with an exact physical backing for every
/// generation invalidation interval.
pub struct BackedPrecompiledGenerationCatalogV1 {
    catalog: PrecompiledGenerationCatalog,
    backings: Vec<PrecompiledGenerationBackingV1>,
    reserved_banks: Vec<BankId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitialGenerationImageErrorV1 {
    UnrecognizedNonzeroByte { physical_address: u32, actual: u8 },
}

impl fmt::Display for InitialGenerationImageErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid initial precompiled-generation image: {self:?}"
        )
    }
}

impl std::error::Error for InitialGenerationImageErrorV1 {}

impl BackedPrecompiledGenerationCatalogV1 {
    pub fn new(
        catalog: PrecompiledGenerationCatalog,
        mut backings: Vec<PrecompiledGenerationBackingV1>,
    ) -> Result<Self, BackedGenerationCatalogErrorV1> {
        backings.sort_unstable_by_key(|backing| backing.generation);
        if let Some(pair) = backings
            .windows(2)
            .find(|pair| pair[0].generation == pair[1].generation)
        {
            return Err(BackedGenerationCatalogErrorV1::DuplicateGenerationBacking {
                generation: pair[0].generation,
            });
        }

        for backing in &backings {
            let generation = catalog
                .generations
                .binary_search_by_key(&backing.generation, |generation| generation.id)
                .ok()
                .map(|index| &catalog.generations[index])
                .ok_or(BackedGenerationCatalogErrorV1::UnknownGenerationBacking {
                    generation: backing.generation,
                })?;
            let actual_start = backing
                .spans
                .first()
                .expect("empty generation backing was rejected by its constructor")
                .virtual_start();
            let actual_end = backing
                .spans
                .last()
                .expect("empty generation backing was rejected by its constructor")
                .virtual_end();
            if (actual_start, actual_end)
                != (generation.invalidation_start, generation.invalidation_end)
            {
                return Err(BackedGenerationCatalogErrorV1::BackingGeometryMismatch {
                    generation: generation.id,
                    expected_start: generation.invalidation_start,
                    expected_end: generation.invalidation_end,
                    actual_start,
                    actual_end,
                });
            }
        }
        for generation in &catalog.generations {
            if backings
                .binary_search_by_key(&generation.id, |backing| backing.generation)
                .is_err()
            {
                return Err(BackedGenerationCatalogErrorV1::MissingGenerationBacking {
                    generation: generation.id,
                });
            }
        }

        let mut mappings = catalog
            .generations
            .iter()
            .map(|generation| {
                let backing = &backings[backings
                    .binary_search_by_key(&generation.id, |backing| backing.generation)
                    .expect("missing generation backing was checked above")];
                (generation, backing)
            })
            .collect::<Vec<_>>();
        mappings.sort_unstable_by_key(|(generation, _)| {
            (
                generation.invalidation_start,
                generation.invalidation_end,
                generation.id,
            )
        });
        for first_index in 0..mappings.len() {
            let (first_generation, first_backing) = mappings[first_index];
            for &(second_generation, second_backing) in &mappings[first_index + 1..] {
                if second_generation.invalidation_start >= first_generation.invalidation_end {
                    break;
                }
                for first_span in &first_backing.spans {
                    for second_span in &second_backing.spans {
                        let overlap_start = first_span.virtual_start.max(second_span.virtual_start);
                        let overlap_end = first_span.virtual_end().min(second_span.virtual_end());
                        if overlap_start < overlap_end
                            && first_span.physical_at(overlap_start)
                                != second_span.physical_at(overlap_start)
                        {
                            return Err(
                                BackedGenerationCatalogErrorV1::InconsistentOverlappingMappings {
                                    first: first_generation.id,
                                    second: second_generation.id,
                                },
                            );
                        }
                    }
                }
            }
        }

        let mut reserved_banks = catalog
            .generations
            .iter()
            .flat_map(|generation| generation.shards.iter().map(|shard| shard.bank))
            .collect::<Vec<_>>();
        reserved_banks.sort_unstable();
        reserved_banks.dedup();

        Ok(Self {
            catalog,
            backings,
            reserved_banks,
        })
    }

    pub fn generations(&self) -> &[PrecompiledGeneration] {
        self.catalog.generations()
    }

    pub fn backings(&self) -> &[PrecompiledGenerationBackingV1] {
        &self.backings
    }

    pub fn reserved_banks(&self) -> &[BankId] {
        &self.reserved_banks
    }

    pub fn evidence_snapshot(&self) -> BackedGenerationCatalogEvidenceV1 {
        BackedGenerationCatalogEvidenceV1 {
            schema: BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1.to_string(),
            generations: self
                .catalog
                .generations
                .iter()
                .map(|generation| PrecompiledGenerationEvidenceV1 {
                    generation: generation.id,
                    image_start: generation.image_start,
                    image_end: generation.image_end,
                    invalidation_start: generation.invalidation_start,
                    invalidation_end: generation.invalidation_end,
                    expected_sha256: generation.expected_sha256,
                    shards: generation.shards.clone(),
                })
                .collect(),
            backings: self
                .backings
                .iter()
                .map(|backing| PrecompiledGenerationBackingEvidenceV1 {
                    generation: backing.generation,
                    spans: backing.spans.clone(),
                })
                .collect(),
            active_segments: self.active_segments(),
        }
    }

    /// Hash the immutable definition of this closed generation inventory.
    ///
    /// The wire is a domain-separated, length-framed sequence in the same
    /// canonical order enforced by the constructors. Runtime activation state
    /// is deliberately absent: publishing or invalidating a generation does
    /// not change the identity of the admitted generations and their physical
    /// mappings.
    pub fn canonical_definition_sha256(&self) -> [u8; 32] {
        fn update_len(digest: &mut Sha256, len: usize) {
            let len = u64::try_from(len).expect("generation catalog length exceeds u64");
            digest.update(len.to_be_bytes());
        }

        let mut digest = Sha256::new();
        let schema = BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1.as_bytes();
        update_len(&mut digest, schema.len());
        digest.update(schema);

        update_len(&mut digest, self.catalog.generations.len());
        for generation in &self.catalog.generations {
            digest.update(generation.id.get().to_be_bytes());
            digest.update(generation.image_start.get().to_be_bytes());
            digest.update(generation.image_end.get().to_be_bytes());
            digest.update(generation.invalidation_start.get().to_be_bytes());
            digest.update(generation.invalidation_end.get().to_be_bytes());
            digest.update(generation.expected_sha256);
            update_len(&mut digest, generation.shards.len());
            for shard in &generation.shards {
                digest.update(shard.bank.get().to_be_bytes());
                digest.update(shard.start.get().to_be_bytes());
                digest.update(shard.end.get().to_be_bytes());
            }
        }

        update_len(&mut digest, self.backings.len());
        for backing in &self.backings {
            digest.update(backing.generation.get().to_be_bytes());
            update_len(&mut digest, backing.spans.len());
            for span in &backing.spans {
                digest.update(span.virtual_start().get().to_be_bytes());
                digest.update(span.physical_start().to_be_bytes());
                digest.update(span.byte_len().to_be_bytes());
            }
        }

        digest.finalize().into()
    }

    pub fn contains_reserved_bank(&self, bank: BankId) -> bool {
        self.reserved_banks.binary_search(&bank).is_ok()
    }

    pub fn active_segments(&self) -> Vec<ActiveGenerationSegment> {
        self.catalog.active_segments()
    }

    pub fn active_generations(&self) -> Vec<GenerationId> {
        self.catalog.active_generations()
    }

    pub fn physical_invalidation_ranges(&self) -> Vec<PhysicalInvalidationRangeV1> {
        let mut ranges = self
            .backings
            .iter()
            .flat_map(|backing| backing.spans.iter())
            .map(|span| PhysicalInvalidationRangeV1 {
                physical_start: span.physical_start(),
                physical_end: span.physical_end(),
            })
            .collect::<Vec<_>>();
        ranges.sort_unstable_by_key(|range| (range.physical_start, range.physical_end));
        let mut canonical: Vec<PhysicalInvalidationRangeV1> = Vec::new();
        for range in ranges {
            if let Some(previous) = canonical.last_mut() {
                if range.physical_start <= previous.physical_end {
                    previous.physical_end = previous.physical_end.max(range.physical_end);
                    continue;
                }
            }
            canonical.push(range);
        }
        canonical
    }

    /// Validate the initially published generation-image bytes without
    /// activating a generation. Zero bytes are an unloaded image. Every
    /// nonzero byte must belong to at least one complete image whose mapped
    /// physical digest exactly matches the immutable catalog.
    ///
    /// Checking coverage by the set of matching images, rather than requiring
    /// every candidate to match, preserves mutually exclusive A/B alternatives
    /// which intentionally share one physical backing.
    pub fn validate_initial_physical_images(
        &self,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) -> Result<Vec<GenerationId>, InitialGenerationImageErrorV1> {
        let matching = self
            .catalog
            .generations
            .iter()
            .filter_map(|generation| {
                let backing = &self.backings[self
                    .backings
                    .binary_search_by_key(&generation.id, |backing| backing.generation)
                    .expect("validated generation has no physical backing")];
                (live_sha256_mapped_with(generation, backing, &mut read_physical_byte)
                    == generation.expected_sha256)
                    .then_some(generation.id)
            })
            .collect::<Vec<_>>();

        let mut image_ranges = self
            .catalog
            .generations
            .iter()
            .flat_map(|generation| {
                let backing = &self.backings[self
                    .backings
                    .binary_search_by_key(&generation.id, |backing| backing.generation)
                    .expect("validated generation has no physical backing")];
                backing.spans.iter().filter_map(move |span| {
                    let start = span.virtual_start.max(generation.image_start);
                    let end = span.virtual_end().min(generation.image_end);
                    (start < end).then(|| {
                        let physical_start = span
                            .physical_at(start)
                            .expect("clipped generation image start left its backing span");
                        (physical_start, physical_start + (end.get() - start.get()))
                    })
                })
            })
            .collect::<Vec<_>>();
        image_ranges.sort_unstable();
        let mut image_union: Vec<(u32, u32)> = Vec::new();
        for (start, end) in image_ranges {
            if let Some(previous) = image_union.last_mut() {
                if start <= previous.1 {
                    previous.1 = previous.1.max(end);
                    continue;
                }
            }
            image_union.push((start, end));
        }

        let mut matched_ranges = matching
            .iter()
            .flat_map(|generation_id| {
                let generation = &self.catalog.generations[self
                    .catalog
                    .generations
                    .binary_search_by_key(generation_id, |generation| generation.id)
                    .expect("matching generation disappeared")];
                let backing = &self.backings[self
                    .backings
                    .binary_search_by_key(generation_id, |backing| backing.generation)
                    .expect("matching generation has no backing")];
                backing.spans.iter().filter_map(move |span| {
                    let virtual_start = span.virtual_start.max(generation.image_start);
                    let virtual_end = span.virtual_end().min(generation.image_end);
                    (virtual_start < virtual_end).then(|| {
                        let physical_start = span
                            .physical_at(virtual_start)
                            .expect("matching image left its backing span");
                        (
                            physical_start,
                            physical_start + (virtual_end.get() - virtual_start.get()),
                        )
                    })
                })
            })
            .collect::<Vec<_>>();
        matched_ranges.sort_unstable();
        let mut matched_union: Vec<(u32, u32)> = Vec::new();
        for (start, end) in matched_ranges {
            if let Some(previous) = matched_union.last_mut() {
                if start <= previous.1 {
                    previous.1 = previous.1.max(end);
                    continue;
                }
            }
            matched_union.push((start, end));
        }

        let mut matched_index = 0;
        for (start, end) in image_union {
            for physical_address in start..end {
                let actual = read_physical_byte(physical_address);
                if actual == 0 {
                    continue;
                }
                while matched_index < matched_union.len()
                    && matched_union[matched_index].1 <= physical_address
                {
                    matched_index += 1;
                }
                let covered_by_match = matched_union.get(matched_index).is_some_and(
                    |&(matched_start, matched_end)| {
                        matched_start <= physical_address && physical_address < matched_end
                    },
                );
                if !covered_by_match {
                    return Err(InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
                        physical_address,
                        actual,
                    });
                }
            }
        }
        Ok(matching)
    }

    /// Retire every active segment owned by a generation whose exact physical
    /// invalidation backing intersects the committed write. A split active
    /// generation is retired as one image because its digest is indivisible.
    pub fn invalidate_physical_write(
        &mut self,
        physical_start: u32,
        physical_end: u32,
    ) -> Result<Vec<GenerationId>, BackedGenerationCatalogErrorV1> {
        if physical_start >= physical_end || physical_end > crate::runtime::RDRAM_LEN as u32 {
            return Err(BackedGenerationCatalogErrorV1::InvalidPhysicalWriteRange {
                physical_start,
                physical_end,
            });
        }
        let mut invalidated = self
            .catalog
            .active
            .iter()
            .filter_map(|active| {
                let generation = &self.catalog.generations[active.generation];
                let backing = &self.backings[self
                    .backings
                    .binary_search_by_key(&generation.id, |backing| backing.generation)
                    .expect("active generation has no validated physical backing")];
                backing
                    .spans
                    .iter()
                    .any(|span| {
                        physical_start < span.physical_end() && physical_end > span.physical_start()
                    })
                    .then_some(generation.id)
            })
            .collect::<Vec<_>>();
        invalidated.sort_unstable();
        invalidated.dedup();
        if !invalidated.is_empty() {
            let generations = &self.catalog.generations;
            self.catalog.active.retain(|active| {
                invalidated
                    .binary_search(&generations[active.generation].id)
                    .is_err()
            });
        }
        Ok(invalidated)
    }

    pub fn resolve_active(&self, pc: GuestPc) -> Result<ExecutionKey, GenerationLookupError> {
        self.catalog.resolve_active(pc)
    }

    pub fn validate_program(&self, program: &BlockProgram) -> Result<(), GenerationCatalogError> {
        self.catalog.validate_program(program)?;
        for bank in program.code().banks() {
            if self.contains_reserved_bank(bank.id()) {
                continue;
            }
            for span in bank.spans() {
                if let Some(generation) = self.catalog.generations.iter().find(|generation| {
                    span.vram_start() < generation.invalidation_end
                        && span.vram_end() > generation.invalidation_start
                }) {
                    return Err(
                        GenerationCatalogError::StaticBankOverlapsGenerationOwnership {
                            bank: bank.id(),
                            span_start: span.vram_start(),
                            span_end: span.vram_end(),
                            generation: generation.id,
                            invalidation_start: generation.invalidation_start,
                            invalidation_end: generation.invalidation_end,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    pub fn activate_for_fetch_with_physical(
        &mut self,
        pc: GuestPc,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) -> Result<GenerationResolution, GenerationLookupError> {
        let backings = &self.backings;
        let resolution = self
            .catalog
            .activate_for_fetch_with_digest(pc, |generation| {
                let backing = &backings[backings
                    .binary_search_by_key(&generation.id, |backing| backing.generation)
                    .expect("validated generation has no physical backing")];
                live_sha256_mapped_with(generation, backing, &mut read_physical_byte)
            })?;
        let digest = self
            .catalog
            .generations
            .iter()
            .find(|generation| generation.id == resolution.generation)
            .expect("successful activation selected an unknown generation")
            .expected_sha256;
        notify_backed_generation_activation_v1(&BackedGenerationActivationObservationV1 {
            requested_pc: pc,
            generation: resolution.generation,
            entry: resolution.entry,
            matched_image_sha256: digest,
            newly_activated: resolution.newly_activated,
            retired: resolution.retired.clone(),
        });
        Ok(resolution)
    }

    pub fn activate_for_fetch(
        &mut self,
        pc: GuestPc,
        mem: &Rdram<'_>,
    ) -> Result<GenerationResolution, GenerationLookupError> {
        self.activate_for_fetch_with_physical(pc, |physical| mem.load_physical_bu(physical))
    }
}

fn live_sha256_mapped_with(
    generation: &PrecompiledGeneration,
    backing: &PrecompiledGenerationBackingV1,
    mut read_physical_byte: impl FnMut(u32) -> u8,
) -> [u8; 32] {
    const CHUNK_LEN: usize = 4096;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; CHUNK_LEN];
    let mut virtual_cursor = generation.image_start;
    for span in &backing.spans {
        let segment_start = span.virtual_start.max(generation.image_start);
        let segment_end = span.virtual_end().min(generation.image_end);
        if segment_start >= segment_end {
            continue;
        }
        assert_eq!(
            segment_start, virtual_cursor,
            "validated image backing stopped tiling the generation image"
        );
        let mut physical_cursor = span
            .physical_at(segment_start)
            .expect("mapped segment start is outside its backing span");
        let mut remaining = segment_end.get() - segment_start.get();
        while remaining > 0 {
            let chunk_len = usize::try_from(remaining.min(CHUNK_LEN as u32))
                .expect("SHA-256 chunk length exceeds usize");
            for (index, byte) in buffer[..chunk_len].iter_mut().enumerate() {
                *byte = read_physical_byte(
                    physical_cursor
                        + u32::try_from(index).expect("SHA-256 chunk index exceeds u32"),
                );
            }
            digest.update(&buffer[..chunk_len]);
            let consumed = u32::try_from(chunk_len).expect("SHA-256 chunk length exceeds u32");
            physical_cursor += consumed;
            remaining -= consumed;
        }
        virtual_cursor = segment_end;
    }
    assert_eq!(
        virtual_cursor, generation.image_end,
        "validated backing stopped before the generation image ended"
    );
    digest.finalize().into()
}

fn live_sha256_with(
    start: GuestPc,
    byte_len: u32,
    mut read_virtual_byte: impl FnMut(u32) -> u8,
) -> [u8; 32] {
    const CHUNK_LEN: usize = 4096;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; CHUNK_LEN];
    let mut offset = 0u32;
    while offset < byte_len {
        let chunk_len = usize::try_from((byte_len - offset).min(CHUNK_LEN as u32))
            .expect("SHA-256 chunk length exceeds usize");
        for (index, byte) in buffer[..chunk_len].iter_mut().enumerate() {
            *byte = read_virtual_byte(
                start.get() + offset + u32::try_from(index).expect("chunk index exceeds u32"),
            );
        }
        digest.update(&buffer[..chunk_len]);
        offset += u32::try_from(chunk_len).expect("SHA-256 chunk length exceeds u32");
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests;
