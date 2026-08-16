use core::num::{NonZeroU32, NonZeroUsize};

use sha2::{Digest, Sha256};

use crate::{ContentDigest, DmemRange, JournalIdentity, PhysicalRange, TmemRange, ValidationError};

pub const MAX_RESOURCE_ACCESSES: usize = 16_384;
pub const MAX_DECLARED_RESOURCE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(u32);

impl OperationId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessMode {
    Read,
    Write,
    ReadWrite,
}

impl AccessMode {
    pub const fn reads(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    pub const fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }

    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Read => 1,
            Self::Write => 2,
            Self::ReadWrite => 3,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, ValidationError> {
        match tag {
            1 => Ok(Self::Read),
            2 => Ok(Self::Write),
            3 => Ok(Self::ReadWrite),
            _ => Err(ValidationError::RecordInvalidTag {
                field: "resource access mode",
                tag,
            }),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::ReadWrite => "read-write",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessPurpose {
    CommandDecode,
    UploadSource,
    TmemLoadSource,
    TmemLoadDestination,
    RenderTarget,
    DepthTarget,
    CopySource,
    CopyDestination,
    ReinterpretSource,
    ReinterpretDestination,
    ViScanout,
    CaptureSource,
    CaptureDestination,
    GuestReadbackSource,
    GuestReadbackDestination,
}

impl AccessPurpose {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::CommandDecode => 1,
            Self::UploadSource => 2,
            Self::TmemLoadSource => 3,
            Self::TmemLoadDestination => 4,
            Self::RenderTarget => 5,
            Self::DepthTarget => 6,
            Self::CopySource => 7,
            Self::CopyDestination => 8,
            Self::ReinterpretSource => 9,
            Self::ReinterpretDestination => 10,
            Self::ViScanout => 11,
            Self::CaptureSource => 12,
            Self::CaptureDestination => 13,
            Self::GuestReadbackSource => 14,
            Self::GuestReadbackDestination => 15,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, ValidationError> {
        match tag {
            1 => Ok(Self::CommandDecode),
            2 => Ok(Self::UploadSource),
            3 => Ok(Self::TmemLoadSource),
            4 => Ok(Self::TmemLoadDestination),
            5 => Ok(Self::RenderTarget),
            6 => Ok(Self::DepthTarget),
            7 => Ok(Self::CopySource),
            8 => Ok(Self::CopyDestination),
            9 => Ok(Self::ReinterpretSource),
            10 => Ok(Self::ReinterpretDestination),
            11 => Ok(Self::ViScanout),
            12 => Ok(Self::CaptureSource),
            13 => Ok(Self::CaptureDestination),
            14 => Ok(Self::GuestReadbackSource),
            15 => Ok(Self::GuestReadbackDestination),
            _ => Err(ValidationError::RecordInvalidTag {
                field: "resource access purpose",
                tag,
            }),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::CommandDecode => "CommandDecode",
            Self::UploadSource => "UploadSource",
            Self::TmemLoadSource => "TmemLoadSource",
            Self::TmemLoadDestination => "TmemLoadDestination",
            Self::RenderTarget => "RenderTarget",
            Self::DepthTarget => "DepthTarget",
            Self::CopySource => "CopySource",
            Self::CopyDestination => "CopyDestination",
            Self::ReinterpretSource => "ReinterpretSource",
            Self::ReinterpretDestination => "ReinterpretDestination",
            Self::ViScanout => "ViScanout",
            Self::CaptureSource => "CaptureSource",
            Self::CaptureDestination => "CaptureDestination",
            Self::GuestReadbackSource => "GuestReadbackSource",
            Self::GuestReadbackDestination => "GuestReadbackDestination",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RdramResource {
    RawCommands,
    Buffer,
    ColorFramebuffer,
    DepthFramebuffer,
    ViSource,
}

impl RdramResource {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::RawCommands => 1,
            Self::Buffer => 2,
            Self::ColorFramebuffer => 3,
            Self::DepthFramebuffer => 4,
            Self::ViSource => 5,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, ValidationError> {
        match tag {
            1 => Ok(Self::RawCommands),
            2 => Ok(Self::Buffer),
            3 => Ok(Self::ColorFramebuffer),
            4 => Ok(Self::DepthFramebuffer),
            5 => Ok(Self::ViSource),
            _ => Err(ValidationError::RecordInvalidTag {
                field: "RDRAM resource",
                tag,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostResource {
    Presentation { id: u64, bytes: NonZeroU32 },
    Capture { id: u64, bytes: NonZeroU32 },
}

impl HostResource {
    pub fn presentation(id: u64, bytes: NonZeroU32) -> Self {
        Self::Presentation { id, bytes }
    }

    pub fn capture(id: u64, bytes: NonZeroU32) -> Self {
        Self::Capture { id, bytes }
    }

    pub const fn id(self) -> u64 {
        match self {
            Self::Presentation { id, .. } | Self::Capture { id, .. } => id,
        }
    }

    pub const fn bytes(self) -> NonZeroU32 {
        match self {
            Self::Presentation { bytes, .. } | Self::Capture { bytes, .. } => bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceRegion {
    Rdram {
        resource: RdramResource,
        range: PhysicalRange,
    },
    RspDmem(DmemRange),
    Tmem(TmemRange),
    Host(HostResource),
}

impl ResourceRegion {
    pub const fn declared_bytes(self) -> u32 {
        match self {
            Self::Rdram { range, .. } => range.len(),
            Self::RspDmem(range) => range.len(),
            Self::Tmem(range) => range.len(),
            Self::Host(resource) => resource.bytes().get(),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Rdram { .. } => "RDRAM",
            Self::RspDmem(_) => "RSP DMEM",
            Self::Tmem(_) => "TMEM",
            Self::Host(HostResource::Presentation { .. }) => "host presentation",
            Self::Host(HostResource::Capture { .. }) => "host capture",
        }
    }

    /// Mutable RDRAM is visible to the guest at the commit boundary. No
    /// currently admitted renderer purpose may write RSP DMEM; adding one must
    /// explicitly extend this classification and the commit receipt tests.
    pub const fn is_guest_visible(self) -> bool {
        matches!(self, Self::Rdram { .. } | Self::RspDmem(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceAccess {
    operation: OperationId,
    mode: AccessMode,
    purpose: AccessPurpose,
    region: ResourceRegion,
}

impl ResourceAccess {
    pub fn try_new(
        operation: OperationId,
        mode: AccessMode,
        purpose: AccessPurpose,
        region: ResourceRegion,
    ) -> Result<Self, ValidationError> {
        let valid_mode = match purpose {
            AccessPurpose::CommandDecode
            | AccessPurpose::UploadSource
            | AccessPurpose::TmemLoadSource
            | AccessPurpose::CopySource
            | AccessPurpose::ReinterpretSource
            | AccessPurpose::ViScanout
            | AccessPurpose::CaptureSource
            | AccessPurpose::GuestReadbackSource => mode == AccessMode::Read,
            AccessPurpose::TmemLoadDestination
            | AccessPurpose::CopyDestination
            | AccessPurpose::CaptureDestination
            | AccessPurpose::GuestReadbackDestination => mode == AccessMode::Write,
            AccessPurpose::RenderTarget
            | AccessPurpose::DepthTarget
            | AccessPurpose::ReinterpretDestination => {
                matches!(mode, AccessMode::Write | AccessMode::ReadWrite)
            }
        };
        if !valid_mode {
            return Err(ValidationError::InvalidAccessMode {
                purpose: purpose.name(),
                mode: mode.name(),
            });
        }
        let valid_resource = match purpose {
            AccessPurpose::CommandDecode => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    ..
                } | ResourceRegion::RspDmem(_)
            ),
            AccessPurpose::TmemLoadDestination => matches!(region, ResourceRegion::Tmem(_)),
            AccessPurpose::UploadSource | AccessPurpose::TmemLoadSource => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer
                        | RdramResource::ColorFramebuffer
                        | RdramResource::DepthFramebuffer
                        | RdramResource::ViSource,
                    ..
                } | ResourceRegion::RspDmem(_)
            ),
            AccessPurpose::RenderTarget => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    ..
                } | ResourceRegion::Host(HostResource::Presentation { .. })
            ),
            AccessPurpose::DepthTarget => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::DepthFramebuffer,
                    ..
                }
            ),
            AccessPurpose::ViScanout => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::ViSource | RdramResource::ColorFramebuffer,
                    ..
                }
            ),
            AccessPurpose::CaptureDestination => {
                matches!(region, ResourceRegion::Host(HostResource::Capture { .. }))
            }
            AccessPurpose::CopySource => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer
                        | RdramResource::ColorFramebuffer
                        | RdramResource::DepthFramebuffer
                        | RdramResource::ViSource,
                    ..
                } | ResourceRegion::Tmem(_)
                    | ResourceRegion::Host(HostResource::Presentation { .. })
                    | ResourceRegion::Host(HostResource::Capture { .. })
            ),
            AccessPurpose::CopyDestination => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer
                        | RdramResource::ColorFramebuffer
                        | RdramResource::DepthFramebuffer,
                    ..
                } | ResourceRegion::Host(HostResource::Presentation { .. })
            ),
            AccessPurpose::ReinterpretSource | AccessPurpose::ReinterpretDestination => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer | RdramResource::DepthFramebuffer,
                    ..
                }
            ),
            AccessPurpose::CaptureSource => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::ViSource | RdramResource::ColorFramebuffer,
                    ..
                } | ResourceRegion::Host(HostResource::Presentation { .. })
            ),
            AccessPurpose::GuestReadbackSource => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer | RdramResource::DepthFramebuffer,
                    ..
                }
            ),
            AccessPurpose::GuestReadbackDestination => matches!(
                region,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer
                        | RdramResource::ColorFramebuffer
                        | RdramResource::DepthFramebuffer
                        | RdramResource::ViSource,
                    ..
                }
            ),
        };
        if !valid_resource {
            return Err(ValidationError::InvalidAccessResource {
                purpose: purpose.name(),
                resource: region.name(),
            });
        }
        Ok(Self {
            operation,
            mode,
            purpose,
            region,
        })
    }

    pub const fn operation(self) -> OperationId {
        self.operation
    }

    pub const fn mode(self) -> AccessMode {
        self.mode
    }

    pub const fn purpose(self) -> AccessPurpose {
        self.purpose
    }

    pub const fn region(self) -> ResourceRegion {
        self.region
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceJournalLimits {
    max_accesses: NonZeroUsize,
    max_declared_bytes: NonZeroU32,
}

impl ResourceJournalLimits {
    pub fn try_new(max_accesses: usize, max_declared_bytes: u32) -> Result<Self, ValidationError> {
        let max_accesses =
            NonZeroUsize::new(max_accesses).ok_or(ValidationError::ZeroJournalLimit {
                field: "max_accesses",
            })?;
        let max_declared_bytes =
            NonZeroU32::new(max_declared_bytes).ok_or(ValidationError::ZeroJournalLimit {
                field: "max_declared_bytes",
            })?;
        if max_accesses.get() > MAX_RESOURCE_ACCESSES {
            return Err(ValidationError::JournalLimitTooLarge {
                field: "max_accesses",
                actual: max_accesses.get() as u64,
                maximum: MAX_RESOURCE_ACCESSES as u64,
            });
        }
        if u64::from(max_declared_bytes.get()) > MAX_DECLARED_RESOURCE_BYTES {
            return Err(ValidationError::JournalLimitTooLarge {
                field: "max_declared_bytes",
                actual: u64::from(max_declared_bytes.get()),
                maximum: MAX_DECLARED_RESOURCE_BYTES,
            });
        }
        Ok(Self {
            max_accesses,
            max_declared_bytes,
        })
    }

    pub const fn max_accesses(self) -> usize {
        self.max_accesses.get()
    }

    pub const fn max_declared_bytes(self) -> u32 {
        self.max_declared_bytes.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceJournal {
    limits: ResourceJournalLimits,
    accesses: Box<[ResourceAccess]>,
    identity: JournalIdentity,
    guest_write_identity: JournalIdentity,
    declared_bytes: u64,
}

impl ResourceJournal {
    pub fn try_new(
        limits: ResourceJournalLimits,
        accesses: Vec<ResourceAccess>,
    ) -> Result<Self, ValidationError> {
        if accesses.is_empty() {
            return Err(ValidationError::EmptyResourceJournal);
        }
        if accesses.len() > limits.max_accesses() {
            return Err(ValidationError::TooManyResourceAccesses {
                actual: accesses.len(),
                maximum: limits.max_accesses(),
            });
        }
        let declared_bytes = accesses.iter().try_fold(0_u64, |total, access| {
            total
                .checked_add(u64::from(access.region.declared_bytes()))
                .ok_or(ValidationError::DeclaredResourceBytesOverflow)
        })?;
        if declared_bytes > u64::from(limits.max_declared_bytes()) {
            return Err(ValidationError::DeclaredResourceBytesExceeded {
                actual: declared_bytes,
                maximum: u64::from(limits.max_declared_bytes()),
            });
        }

        let identity = hash_accesses(b"fn64.render-ir.resource-journal.v2\0", &accesses);
        let guest_writes: Vec<_> = accesses
            .iter()
            .copied()
            .filter(|access| access.mode.writes() && access.region.is_guest_visible())
            .collect();
        let guest_write_identity =
            hash_accesses(b"fn64.render-ir.guest-write-journal.v2\0", &guest_writes);
        Ok(Self {
            limits,
            accesses: accesses.into_boxed_slice(),
            identity,
            guest_write_identity,
            declared_bytes,
        })
    }

    pub const fn limits(&self) -> ResourceJournalLimits {
        self.limits
    }

    pub fn accesses(&self) -> &[ResourceAccess] {
        &self.accesses
    }

    pub const fn identity(&self) -> JournalIdentity {
        self.identity
    }

    pub const fn guest_write_identity(&self) -> JournalIdentity {
        self.guest_write_identity
    }

    pub const fn declared_bytes(&self) -> u64 {
        self.declared_bytes
    }

    pub(crate) fn command_reads(&self) -> impl Iterator<Item = &ResourceAccess> {
        self.accesses
            .iter()
            .filter(|access| access.purpose == AccessPurpose::CommandDecode)
    }

    pub(crate) fn guest_write_accesses(&self) -> impl Iterator<Item = ResourceAccess> + '_ {
        self.accesses
            .iter()
            .copied()
            .filter(|access| access.mode.writes() && access.region.is_guest_visible())
    }

    pub(crate) fn write_accesses(&self) -> impl Iterator<Item = ResourceAccess> + '_ {
        self.accesses
            .iter()
            .copied()
            .filter(|access| access.mode.writes())
    }

    pub(crate) fn matches_memory_layout(&self, layout: crate::PhysicalMemoryLayout) -> bool {
        self.accesses.iter().all(|access| match access.region {
            ResourceRegion::Rdram { range, .. } => range.layout() == layout,
            _ => true,
        })
    }
}

fn hash_accesses(domain: &[u8], accesses: &[ResourceAccess]) -> JournalIdentity {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update((accesses.len() as u32).to_be_bytes());
    for access in accesses {
        hash.update(access.operation.0.to_be_bytes());
        hash.update([access.mode.tag(), access.purpose.tag()]);
        hash_region(&mut hash, access.region);
    }
    JournalIdentity::new(ContentDigest::from_bytes(hash.finalize().into()))
}

pub(crate) fn hash_region(hash: &mut Sha256, region: ResourceRegion) {
    match region {
        ResourceRegion::Rdram { resource, range } => {
            hash.update([1, resource.tag()]);
            hash.update(range.layout().bytes().to_be_bytes());
            hash.update(range.start().get().to_be_bytes());
            hash.update(range.end().to_be_bytes());
        }
        ResourceRegion::RspDmem(range) => {
            hash.update([2]);
            hash.update(range.start().to_be_bytes());
            hash.update(range.end().to_be_bytes());
        }
        ResourceRegion::Tmem(range) => {
            hash.update([3]);
            hash.update(range.start().to_be_bytes());
            hash.update(range.end().to_be_bytes());
        }
        ResourceRegion::Host(HostResource::Presentation { id, bytes }) => {
            hash.update([4, 1]);
            hash.update(id.to_be_bytes());
            hash.update(bytes.get().to_be_bytes());
        }
        ResourceRegion::Host(HostResource::Capture { id, bytes }) => {
            hash.update([4, 2]);
            hash.update(id.to_be_bytes());
            hash.update(bytes.get().to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhysicalMemoryLayout;

    #[test]
    fn access_purpose_rejects_wrong_direction_and_resource_class() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let commands = ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: layout.range(0, 8).unwrap(),
        };
        assert!(matches!(
            ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Write,
                AccessPurpose::CommandDecode,
                commands
            ),
            Err(ValidationError::InvalidAccessMode { .. })
        ));
        assert!(matches!(
            ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::ReadWrite,
                AccessPurpose::CommandDecode,
                commands
            ),
            Err(ValidationError::InvalidAccessMode { .. })
        ));
        assert!(matches!(
            ResourceAccess::try_new(
                OperationId::new(2),
                AccessMode::Write,
                AccessPurpose::CopyDestination,
                ResourceRegion::RspDmem(DmemRange::try_new(0, 8).unwrap())
            ),
            Err(ValidationError::InvalidAccessResource { .. })
        ));
        assert!(matches!(
            ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Tmem(TmemRange::try_new(0, 8).unwrap())
            ),
            Err(ValidationError::InvalidAccessResource { .. })
        ));
        assert!(matches!(
            ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::RenderTarget,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range: layout.range(0x100, 0x108).unwrap(),
                }
            ),
            Err(ValidationError::InvalidAccessMode { .. })
        ));
    }

    #[test]
    fn journal_is_ordered_bounded_and_has_separate_guest_write_identity() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let read = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(0, 8).unwrap(),
            },
        )
        .unwrap();
        let write = ResourceAccess::try_new(
            OperationId::new(1),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(0x100, 0x200).unwrap(),
            },
        )
        .unwrap();
        let limits = ResourceJournalLimits::try_new(2, 0x108).unwrap();
        let journal = ResourceJournal::try_new(limits, vec![read, write]).unwrap();
        assert_eq!(journal.declared_bytes(), 0x108);
        assert_ne!(journal.identity(), journal.guest_write_identity());

        assert!(matches!(
            ResourceJournal::try_new(
                ResourceJournalLimits::try_new(1, 0x1000).unwrap(),
                vec![read, write]
            ),
            Err(ValidationError::TooManyResourceAccesses { .. })
        ));
    }
}
