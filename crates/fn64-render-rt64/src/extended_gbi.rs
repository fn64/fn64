//! Typed encoding for pinned RT64's public F3DEX2 Extended-GBI v1 protocol.
//!
//! This module is deliberately independent of [`crate::ReferenceBackend`]
//! and the RT64 C++ adapter. A cooperating game first submits [`Probe::command`]
//! in one completed graphics task, reads the zero-initialized return word on
//! the CPU, and only then uses the returned [`Version1`] to build later
//! display lists. The probe cannot conditionally enable Extended GBI inside
//! the task that contains it.
//!
//! Provenance: pinned MIT RT64
//! `include/rt64_extended_gbi.h` at
//! `f0728a2520d5aa735886240de3fee75cc805f6d6`.

use std::fmt;

const HOOK_OPCODE: u8 = 0xe0;
const HOOK_MAGIC: u32 = 0x0052_5464;
const VERSION_1: u32 = 1;
const MAX_SEGMENTED_ADDRESS: u32 = 0x0fff_ffff;

/// One public F3DEX2 command in host-independent word form.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Command {
    word0: u32,
    word1: u32,
}

impl Command {
    const fn new(word0: u32, word1: u32) -> Self {
        Self { word0, word1 }
    }

    pub const fn words(self) -> (u32, u32) {
        (self.word0, self.word1)
    }
}

/// A named protocol error rather than truncation or an assumed dialect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError(String);

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

/// Whether a build should omit cooperation, use it when recognized, or
/// require it. This is runtime policy: all three modes use the same binary.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    #[default]
    Disabled,
    IfAvailable,
    Required,
}

impl Policy {
    /// Build the optional probe. Disabled policy returns `None`, so callers
    /// cannot accidentally emit a capability query while claiming the base
    /// display-list path is independent.
    pub fn probe(self, return_address: u32) -> Result<Option<Probe>, ProtocolError> {
        match self {
            Self::Disabled => Ok(None),
            Self::IfAvailable | Self::Required => {
                Probe::new(return_address, self == Self::Required).map(Some)
            }
        }
    }
}

/// A pending capability query. Its return word must be initialized to
/// [`Probe::RETURN_WORD_INITIALIZER`] before the probe task is submitted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    return_address: u32,
    required: bool,
}

impl Probe {
    pub const RETURN_WORD_INITIALIZER: u32 = 0;

    fn new(return_address: u32, required: bool) -> Result<Self, ProtocolError> {
        if return_address > MAX_SEGMENTED_ADDRESS {
            return Err(ProtocolError::new(format!(
                "Extended-GBI GetVersion return address {return_address:#010x} exceeds the public 28-bit segmented field"
            )));
        }
        if !return_address.is_multiple_of(4) {
            return Err(ProtocolError::new(format!(
                "Extended-GBI GetVersion return address {return_address:#010x} is not word-aligned"
            )));
        }
        Ok(Self {
            return_address,
            required,
        })
    }

    pub const fn command(self) -> Command {
        Command::new(
            ((HOOK_OPCODE as u32) << 24) | HOOK_MAGIC,
            self.return_address,
        )
    }

    /// Resolve the CPU-visible word only after the probe task completes.
    /// Zero means the optional hook was not recognized; any nonzero version
    /// other than v1 is unknown and cannot be interpreted as v1.
    pub fn resolve(self, return_word: u32) -> Result<Availability, ProtocolError> {
        match return_word {
            Self::RETURN_WORD_INITIALIZER if self.required => Err(ProtocolError::new(
                "required Extended-GBI cooperation was not recognized",
            )),
            Self::RETURN_WORD_INITIALIZER => Ok(Availability::Unavailable),
            VERSION_1 => Ok(Availability::Version1(Version1 {
                opcode: ExtendedOpcode::DEFAULT,
            })),
            version => Err(ProtocolError::new(format!(
                "unknown Extended-GBI version {version}; refusing to assume v1"
            ))),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Unavailable,
    Version1(Version1),
}

impl Availability {
    pub fn require_v1(self) -> Result<Version1, ProtocolError> {
        match self {
            Self::Version1(version) => Ok(version),
            Self::Unavailable => Err(ProtocolError::new(
                "Extended-GBI v1 session requested after an unavailable optional probe",
            )),
        }
    }
}

/// The display-list opcode selected by a cooperating game.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExtendedOpcode(u8);

impl ExtendedOpcode {
    pub const DEFAULT: Self = Self(0x64);

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A recognized v1 protocol dialect. RT64 resets Extended-GBI state at a
/// workload boundary, so every cooperating display list must still begin
/// with [`Version1::enable_command`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Version1 {
    opcode: ExtendedOpcode,
}

impl Version1 {
    pub const fn opcode(self) -> ExtendedOpcode {
        self.opcode
    }

    pub const fn enable_command(self) -> Command {
        Command::new(
            ((HOOK_OPCODE as u32) << 24) | HOOK_MAGIC,
            (1 << 28) | self.opcode.0 as u32,
        )
    }

    pub const fn disable_command(self) -> Command {
        Command::new(((HOOK_OPCODE as u32) << 24) | HOOK_MAGIC, 2 << 28)
    }

    pub const fn set_rect_aspect(self, aspect: AspectMode) -> Command {
        self.extended_command(0x33, aspect as u32)
    }

    pub const fn set_rect_align(self, alignment: RectAlignment) -> [Command; 2] {
        [
            self.extended_command(
                0x06,
                alignment.left_origin.bits() | (alignment.right_origin.bits() << 12),
            ),
            Command::new(
                pack_i16(alignment.left_offset, alignment.top_offset),
                pack_i16(alignment.right_offset, alignment.bottom_offset),
            ),
        ]
    }

    pub fn set_refresh_rate(self, refresh_rate: u16) -> Result<Command, ProtocolError> {
        if refresh_rate == 0 {
            return Err(ProtocolError::new(
                "Extended-GBI refresh rate must be nonzero",
            ));
        }
        Ok(self.extended_command(0x09, u32::from(refresh_rate)))
    }

    pub const fn begin_vertex_z_test(self, vertex_index: u8) -> Command {
        self.extended_command(0x0a, vertex_index as u32)
    }

    pub const fn end_vertex_z_test(self) -> Command {
        self.extended_command(0x0b, 0)
    }

    pub const fn matrix_group(self, group: MatrixGroup) -> [Command; 2] {
        [
            self.extended_command(0x0c, group.id),
            Command::new(group.selectors_word(), 0),
        ]
    }

    const fn extended_command(self, id: u32, word1: u32) -> Command {
        Command::new(((self.opcode.0 as u32) << 24) | id, word1)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum AspectMode {
    #[default]
    Auto = 0,
    Stretch = 1,
    Adjust = 2,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    #[default]
    None,
    Left,
    Center,
    Right,
}

impl Origin {
    const fn bits(self) -> u32 {
        match self {
            Self::None => 0x800,
            Self::Left => 0,
            Self::Center => 0x200,
            Self::Right => 0x400,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RectAlignment {
    pub left_origin: Origin,
    pub right_origin: Origin,
    pub left_offset: i16,
    pub top_offset: i16,
    pub right_offset: i16,
    pub bottom_offset: i16,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Component {
    #[default]
    Skip = 0,
    Interpolate = 1,
    Auto = 2,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum MatrixMode {
    #[default]
    Simple = 0,
    Decompose = 1,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum MatrixOrder {
    #[default]
    Linear = 0,
    Auto = 1,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MatrixGroup {
    pub id: u32,
    pub push: bool,
    pub projection: bool,
    pub mode: MatrixMode,
    pub position: Component,
    pub rotation: Component,
    pub scale: Component,
    pub skew: Component,
    pub perspective: Component,
    pub vertex: Component,
    pub tile: Component,
    pub order: MatrixOrder,
    pub editable: bool,
    pub aspect: AspectMode,
    pub texcoord: Component,
    pub look_at: Component,
}

impl MatrixGroup {
    const fn selectors_word(self) -> u32 {
        bool_bit(self.push, 0)
            | bool_bit(self.projection, 1)
            | ((self.mode as u32) << 2)
            | ((self.position as u32) << 3)
            | ((self.rotation as u32) << 5)
            | ((self.scale as u32) << 7)
            | ((self.skew as u32) << 9)
            | ((self.perspective as u32) << 11)
            | ((self.vertex as u32) << 13)
            | ((self.tile as u32) << 15)
            | ((self.order as u32) << 17)
            | bool_bit(self.editable, 19)
            | ((self.aspect as u32) << 20)
            | ((self.texcoord as u32) << 22)
            | ((self.look_at as u32) << 24)
    }
}

const fn bool_bit(value: bool, shift: u32) -> u32 {
    (value as u32) << shift
}

const fn pack_i16(upper: i16, lower: i16) -> u32 {
    ((upper as u16 as u32) << 16) | (lower as u16 as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn required_v1() -> Version1 {
        Policy::Required
            .probe(0x0000_1000)
            .unwrap()
            .unwrap()
            .resolve(1)
            .unwrap()
            .require_v1()
            .unwrap()
    }

    #[test]
    fn policy_keeps_disabled_and_optional_paths_distinct() {
        assert_eq!(Policy::Disabled.probe(u32::MAX).unwrap(), None);
        let optional = Policy::IfAvailable.probe(0x1000).unwrap().unwrap();
        assert_eq!(optional.resolve(0).unwrap(), Availability::Unavailable);
        assert!(Policy::Required
            .probe(0x1000)
            .unwrap()
            .unwrap()
            .resolve(0)
            .unwrap_err()
            .to_string()
            .contains("required"));
    }

    #[test]
    fn unknown_versions_and_addresses_fail_loudly() {
        assert!(Policy::Required.probe(0x1000_0000).is_err());
        assert!(Policy::Required.probe(0x1002).is_err());
        assert!(Policy::IfAvailable
            .probe(0x1000)
            .unwrap()
            .unwrap()
            .resolve(2)
            .unwrap_err()
            .to_string()
            .contains("unknown Extended-GBI version 2"));
        assert_eq!(ExtendedOpcode::DEFAULT.get(), 0x64);
    }

    #[test]
    fn public_header_vectors_are_exact() {
        let probe = Policy::Required.probe(0x0234_5678).unwrap().unwrap();
        assert_eq!(probe.command().words(), (0xe052_5464, 0x0234_5678));
        let v1 = required_v1();
        assert_eq!(v1.enable_command().words(), (0xe052_5464, 0x1000_0064));
        assert_eq!(v1.disable_command().words(), (0xe052_5464, 0x2000_0000));
        assert_eq!(
            v1.set_rect_aspect(AspectMode::Adjust).words(),
            (0x6400_0033, 2)
        );
        assert_eq!(
            v1.set_rect_align(RectAlignment {
                left_origin: Origin::Left,
                right_origin: Origin::Right,
                left_offset: 16,
                right_offset: 16,
                ..RectAlignment::default()
            })
            .map(Command::words),
            [(0x6400_0006, 0x0040_0000), (0x0010_0000, 0x0010_0000)]
        );
        assert_eq!(
            v1.set_refresh_rate(120).unwrap().words(),
            (0x6400_0009, 120)
        );
        assert_eq!(v1.begin_vertex_z_test(3).words(), (0x6400_000a, 3));
        assert_eq!(v1.end_vertex_z_test().words(), (0x6400_000b, 0));
    }

    #[test]
    fn matrix_group_vector_and_refresh_invariants_are_exact() {
        let v1 = required_v1();
        let commands = v1.matrix_group(MatrixGroup {
            id: 7,
            mode: MatrixMode::Decompose,
            position: Component::Interpolate,
            rotation: Component::Interpolate,
            editable: true,
            aspect: AspectMode::Adjust,
            ..MatrixGroup::default()
        });
        let selectors = (1 << 2) | (1 << 3) | (1 << 5) | (1 << 19) | (2 << 20);
        assert_eq!(
            commands.map(Command::words),
            [(0x6400_000c, 7), (selectors, 0)]
        );
        assert!(v1.set_refresh_rate(0).is_err());
    }
}
