//! Deterministic, content-free F3DZEX2 point-light characterization vectors.
//!
//! The two controls and six point-light policy rows use only the common public
//! F3DEX2 command contract: matrices, viewport, geometry mode,
//! `G_MW_NUMLIGHT`, `G_MV_LIGHT`, vertices, one triangle, `G_RDPFULLSYNC`, and
//! `G_ENDDL`. Candidate cases deliberately describe raw record byte lanes and
//! transfer widths rather than assigning proprietary meanings to them. Their
//! zero/axis/distance placement uses an explicit experiment coordinate-frame
//! hypothesis. Every point-light row executes at all three candidate transfer
//! widths; a non-response at one width cannot become a false negative. A
//! response can justify the next batch; the generator itself never promotes a
//! response into an activation, position, attenuation, coordinate-space, or
//! arithmetic claim.
//!
//! Provenance: public libultra `gbi.h` and the public F3DEX2 Programming Manual
//! command/structure contracts, as summarized in the checked-in
//! `fn64-render-reference/F3DEX2-CONCEPTS.md`. The existence of point-light
//! capability in F3DZEX2 2.08I/J is checked-in software-parity evidence in
//! `F3DEX2-VARIANTS.md`. The activation bit comes from pinned MIT RT64
//! `src/shared/rt64_f3d_defines.h`; its conjunction with the typed capability
//! comes from `src/hle/rt64_rsp.cpp`.

use std::error::Error;
use std::fmt;

use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};

pub const RDRAM_BYTES: usize = 8 * 1024 * 1024;
pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 48;
pub const DISPLAY_LIST_ADDRESS: u32 = 0x0003_0000;
pub const OUTPUT_ADDRESS: u32 = 0x0040_0000;
pub const DEPTH_ADDRESS: u32 = 0x0040_2000;
pub const GUARD_WORD: u32 = 0xa31f_7c59;

const DISPLAY_LIST_BYTES: u32 = 0x800;
const VERTICES_ADDRESS: u32 = 0x0003_1000;
const VERTICES_BYTES: u32 = 3 * 16;
const VIEWPORT_ADDRESS: u32 = 0x0003_1100;
const VIEWPORT_BYTES: u32 = 16;
const PROJECTION_ADDRESS: u32 = 0x0003_1200;
const MODELVIEW_ADDRESS: u32 = 0x0003_1280;
const MATRIX_BYTES: u32 = 64;
const CANDIDATE_LIGHT_ADDRESS: u32 = 0x0003_1400;
const AMBIENT_LIGHT_ADDRESS: u32 = 0x0003_1500;
const LIGHT_REGION_BYTES: u32 = 32;
const FRAMEBUFFER_BYTES: u32 = WIDTH * HEIGHT * 2;
const _: () = {
    assert!(DISPLAY_LIST_ADDRESS < 0x0080_0000);
    assert!(OUTPUT_ADDRESS < 0x0080_0000);
    assert!(DEPTH_ADDRESS < 0x0080_0000);
};

const G_VTX: u8 = 0x01;
const G_TRI1: u8 = 0x05;
const G_GEOMETRYMODE: u8 = 0xd9;
const G_MTX: u8 = 0xda;
const G_MOVEWORD: u8 = 0xdb;
const G_MOVEMEM: u8 = 0xdc;
const G_ENDDL: u8 = 0xdf;
const G_RDPPIPESYNC: u8 = 0xe7;
const G_RDPFULLSYNC: u8 = 0xe9;
const G_SETSCISSOR: u8 = 0xed;
const G_RDPSETOTHERMODE: u8 = 0xef;
const G_FILLRECT: u8 = 0xf6;
const G_SETFILLCOLOR: u8 = 0xf7;
const G_SETCOMBINE: u8 = 0xfc;
const G_SETZIMG: u8 = 0xfe;
const G_SETCIMG: u8 = 0xff;
const G_MW_NUMLIGHT: u32 = 0x02;
const G_MV_VIEWPORT: u32 = 0x08;
const G_MV_LIGHT: u32 = 0x0a;
const G_SHADE: u32 = 0x0000_0004;
const G_LIGHTING: u32 = 0x0002_0000;
const G_SHADING_SMOOTH: u32 = 0x0020_0000;
const G_POINT_LIGHTING: u32 = 0x0040_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub name: &'static str,
    pub start: u32,
    pub len: u32,
}

impl MemoryRegion {
    pub const fn end(self) -> u32 {
        self.start + self.len
    }

    pub const fn guarded_start(self) -> u32 {
        self.start - 4
    }

    pub const fn guarded_end(self) -> u32 {
        self.end() + 4
    }
}

pub const MEMORY_REGIONS: [MemoryRegion; 9] = [
    MemoryRegion {
        name: "display-list",
        start: DISPLAY_LIST_ADDRESS,
        len: DISPLAY_LIST_BYTES,
    },
    MemoryRegion {
        name: "vertices",
        start: VERTICES_ADDRESS,
        len: VERTICES_BYTES,
    },
    MemoryRegion {
        name: "viewport",
        start: VIEWPORT_ADDRESS,
        len: VIEWPORT_BYTES,
    },
    MemoryRegion {
        name: "projection-matrix",
        start: PROJECTION_ADDRESS,
        len: MATRIX_BYTES,
    },
    MemoryRegion {
        name: "modelview-matrix",
        start: MODELVIEW_ADDRESS,
        len: MATRIX_BYTES,
    },
    MemoryRegion {
        name: "candidate-light-record",
        start: CANDIDATE_LIGHT_ADDRESS,
        len: LIGHT_REGION_BYTES,
    },
    MemoryRegion {
        name: "ambient-light-record",
        start: AMBIENT_LIGHT_ADDRESS,
        len: LIGHT_REGION_BYTES,
    },
    MemoryRegion {
        name: "rgba16-output",
        start: OUTPUT_ADDRESS,
        len: FRAMEBUFFER_BYTES,
    },
    MemoryRegion {
        name: "depth-image",
        start: DEPTH_ADDRESS,
        len: FRAMEBUFFER_BYTES,
    },
];

/// The exact policy denominator. The order is lexical and therefore also the
/// canonical report order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequiredCase {
    DirectionalLightControl,
    LightingDisabledControl,
    PointLightFarDistance,
    PointLightNearDistance,
    PointLightNegativeAxis,
    PointLightPositiveAxis,
    PointLightRecordBoundary,
    PointLightZeroDistance,
}

/// Alternate policy-facing name for the same closed denominator type.
pub type DenominatorCase = RequiredCase;

impl RequiredCase {
    pub const ALL: [Self; 8] = [
        Self::DirectionalLightControl,
        Self::LightingDisabledControl,
        Self::PointLightFarDistance,
        Self::PointLightNearDistance,
        Self::PointLightNegativeAxis,
        Self::PointLightPositiveAxis,
        Self::PointLightRecordBoundary,
        Self::PointLightZeroDistance,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::DirectionalLightControl => "directional-light-control",
            Self::LightingDisabledControl => "lighting-disabled-control",
            Self::PointLightFarDistance => "point-light-far-distance",
            Self::PointLightNearDistance => "point-light-near-distance",
            Self::PointLightNegativeAxis => "point-light-negative-axis",
            Self::PointLightPositiveAxis => "point-light-positive-axis",
            Self::PointLightRecordBoundary => "point-light-record-boundary",
            Self::PointLightZeroDistance => "point-light-zero-distance",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PointCase {
    FarDistance,
    NearDistance,
    NegativeAxis,
    PositiveAxis,
    RecordBoundary,
    ZeroDistance,
}

impl PointCase {
    pub const ALL: [Self; 6] = [
        Self::FarDistance,
        Self::NearDistance,
        Self::NegativeAxis,
        Self::PositiveAxis,
        Self::RecordBoundary,
        Self::ZeroDistance,
    ];

    pub const fn denominator_case(self) -> RequiredCase {
        match self {
            Self::FarDistance => RequiredCase::PointLightFarDistance,
            Self::NearDistance => RequiredCase::PointLightNearDistance,
            Self::NegativeAxis => RequiredCase::PointLightNegativeAxis,
            Self::PositiveAxis => RequiredCase::PointLightPositiveAxis,
            Self::RecordBoundary => RequiredCase::PointLightRecordBoundary,
            Self::ZeroDistance => RequiredCase::PointLightZeroDistance,
        }
    }
}

/// Candidate DMA envelope. Only `Bytes16` is the public common light-record
/// size. The larger sizes are explicitly black-box hypotheses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransferWidth {
    Bytes16,
    Bytes24,
    Bytes32,
}

impl TransferWidth {
    pub const ALL: [Self; 3] = [Self::Bytes16, Self::Bytes24, Self::Bytes32];

    pub const fn bytes(self) -> u8 {
        match self {
            Self::Bytes16 => 16,
            Self::Bytes24 => 24,
            Self::Bytes32 => 32,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Bytes16 => "16",
            Self::Bytes24 => "24",
            Self::Bytes32 => "32",
        }
    }

    const fn contains(self, group: RecordFieldGroup) -> bool {
        group.offset() + 4 <= self.bytes()
    }
}

/// Four-byte lanes are observational partitions, not semantic fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordFieldGroup {
    Bytes00To03,
    Bytes04To07,
    Bytes08To11,
    Bytes12To15,
    Bytes16To19,
    Bytes20To23,
    Bytes24To27,
    Bytes28To31,
}

impl RecordFieldGroup {
    pub const ALL: [Self; 8] = [
        Self::Bytes00To03,
        Self::Bytes04To07,
        Self::Bytes08To11,
        Self::Bytes12To15,
        Self::Bytes16To19,
        Self::Bytes20To23,
        Self::Bytes24To27,
        Self::Bytes28To31,
    ];

    pub const fn offset(self) -> u8 {
        match self {
            Self::Bytes00To03 => 0,
            Self::Bytes04To07 => 4,
            Self::Bytes08To11 => 8,
            Self::Bytes12To15 => 12,
            Self::Bytes16To19 => 16,
            Self::Bytes20To23 => 20,
            Self::Bytes24To27 => 24,
            Self::Bytes28To31 => 28,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Bytes00To03 => "bytes00-03",
            Self::Bytes04To07 => "bytes04-07",
            Self::Bytes08To11 => "bytes08-11",
            Self::Bytes12To15 => "bytes12-15",
            Self::Bytes16To19 => "bytes16-19",
            Self::Bytes20To23 => "bytes20-23",
            Self::Bytes24To27 => "bytes24-27",
            Self::Bytes28To31 => "bytes28-31",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProbeLevel {
    Low,
    SignBoundary,
    High,
}

impl ProbeLevel {
    pub const ALL: [Self; 3] = [Self::Low, Self::SignBoundary, Self::High];

    const fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::SignBoundary => "sign-boundary",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaseId {
    DirectionalLightControl,
    LightingDisabledControl,
    Point {
        point: PointCase,
        width: TransferWidth,
    },
    Knockout {
        width: TransferWidth,
        group: RecordFieldGroup,
    },
    Refinement {
        width: TransferWidth,
        group: RecordFieldGroup,
        level: ProbeLevel,
    },
}

impl CaseId {
    pub fn name(self) -> String {
        match self {
            Self::DirectionalLightControl => RequiredCase::DirectionalLightControl.name().into(),
            Self::LightingDisabledControl => RequiredCase::LightingDisabledControl.name().into(),
            Self::Point { point, width } => format!(
                "{}-transfer-{}",
                point.denominator_case().name(),
                width.name()
            ),
            Self::Knockout { width, group } => format!(
                "candidate-transfer-{}-without-{}",
                width.name(),
                group.name()
            ),
            Self::Refinement {
                width,
                group,
                level,
            } => format!(
                "candidate-transfer-{}-{}-{}",
                width.name(),
                group.name(),
                level.name()
            ),
        }
    }

    pub const fn stage(self) -> Stage {
        match self {
            Self::DirectionalLightControl | Self::LightingDisabledControl | Self::Point { .. } => {
                Stage::RequiredDenominator
            }
            Self::Knockout { .. } => Stage::FieldKnockout,
            Self::Refinement { .. } => Stage::FieldRefinement,
        }
    }

    /// Adaptive envelope and byte-lane cases are evidence for the one policy
    /// row that asks which candidate record boundary is behavior-bearing.
    pub const fn denominator_case(self) -> DenominatorCase {
        match self {
            Self::DirectionalLightControl => RequiredCase::DirectionalLightControl,
            Self::LightingDisabledControl => RequiredCase::LightingDisabledControl,
            Self::Point { point, .. } => point.denominator_case(),
            Self::Knockout { .. } | Self::Refinement { .. } => {
                RequiredCase::PointLightRecordBoundary
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    RequiredDenominator,
    FieldKnockout,
    FieldRefinement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResponsiveField {
    pub width: TransferWidth,
    pub group: RecordFieldGroup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorError {
    RdramTooSmall {
        actual: usize,
        required: usize,
    },
    FieldOutsideEnvelope {
        width: TransferWidth,
        group: RecordFieldGroup,
    },
}

impl fmt::Display for VectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RdramTooSmall { actual, required } => {
                write!(
                    f,
                    "characterization RDRAM has {actual} bytes; requires {required}"
                )
            }
            Self::FieldOutsideEnvelope { width, group } => write!(
                f,
                "candidate group {} is outside the {}-byte transfer envelope",
                group.name(),
                width.bytes()
            ),
        }
    }
}

impl Error for VectorError {}

#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterizationSuite;

impl CharacterizationSuite {
    /// The exact eight policy rows expanded into the complete initial
    /// executable batch: one case per public control and all three candidate
    /// transfer widths for every point-light row.
    pub fn initial_cases(self) -> Vec<VectorCase> {
        [
            VectorCase::new(CaseId::DirectionalLightControl),
            VectorCase::new(CaseId::LightingDisabledControl),
        ]
        .into_iter()
        .chain(PointCase::ALL.into_iter().flat_map(|point| {
            TransferWidth::ALL
                .into_iter()
                .map(move |width| VectorCase::new(CaseId::Point { point, width }))
        }))
        .collect()
    }

    /// Select one-field knockouts only for transfer widths whose cross-variant
    /// observation was responsive. Duplicate observations do not duplicate
    /// cases, and order is canonical.
    pub fn knockout_cases(self, responsive_widths: &[TransferWidth]) -> Vec<VectorCase> {
        TransferWidth::ALL
            .into_iter()
            .filter(|width| responsive_widths.contains(width))
            .flat_map(|width| {
                RecordFieldGroup::ALL
                    .into_iter()
                    .filter(move |group| width.contains(*group))
                    .map(move |group| VectorCase::new(CaseId::Knockout { width, group }))
            })
            .collect()
    }

    /// Refine only externally identified responsive lanes. Each accepted lane
    /// receives low, signed-boundary, and high raw patterns; those labels do
    /// not assign a numeric representation to the proprietary field.
    pub fn refinement_cases(
        self,
        responsive_fields: &[ResponsiveField],
    ) -> Result<Vec<VectorCase>, VectorError> {
        for response in responsive_fields {
            if !response.width.contains(response.group) {
                return Err(VectorError::FieldOutsideEnvelope {
                    width: response.width,
                    group: response.group,
                });
            }
        }
        let mut cases = Vec::new();
        for width in TransferWidth::ALL {
            for group in RecordFieldGroup::ALL {
                let response = ResponsiveField { width, group };
                if responsive_fields.contains(&response) {
                    cases.extend(ProbeLevel::ALL.into_iter().map(|level| {
                        VectorCase::new(CaseId::Refinement {
                            width,
                            group,
                            level,
                        })
                    }));
                }
            }
        }
        Ok(cases)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorCase {
    id: CaseId,
}

impl VectorCase {
    const fn new(id: CaseId) -> Self {
        Self { id }
    }

    pub const fn id(&self) -> CaseId {
        self.id
    }

    pub fn commands(&self) -> Result<Vec<(u32, u32)>, VectorError> {
        let setup = SceneSetup::for_case(self.id)?;
        Ok(build_commands(setup))
    }

    /// Installs only the declared repository-owned regions; private microcode
    /// windows elsewhere in RDRAM are not cleared or inspected.
    pub fn install(&self, rdram: &mut [u8]) -> Result<InstalledVector, VectorError> {
        if rdram.len() < RDRAM_BYTES {
            return Err(VectorError::RdramTooSmall {
                actual: rdram.len(),
                required: RDRAM_BYTES,
            });
        }
        let setup = SceneSetup::for_case(self.id)?;
        let commands = build_commands(setup);
        assert!(commands.len() * 8 <= DISPLAY_LIST_BYTES as usize);

        let mut view = RdramViewMut::from_storage(rdram);
        for region in MEMORY_REGIONS {
            view.write_logical_bytes(
                addr(region.start),
                &vec![0; usize::try_from(region.len).expect("region length fits usize")],
            );
        }
        write_vertices(&mut view, setup.vertices, setup.normal);
        write_viewport(&mut view);
        write_matrix_diagonal(&mut view, PROJECTION_ADDRESS, [2731, 3277, 2048, 65536]);
        write_modelview(&mut view, setup.model_translation);
        view.write_logical_bytes(addr(CANDIDATE_LIGHT_ADDRESS), &setup.candidate_record);
        view.write_logical_bytes(addr(AMBIENT_LIGHT_ADDRESS), &ambient_record());
        for pixel in 0..WIDTH * HEIGHT {
            view.write_u16(addr(DEPTH_ADDRESS + pixel * 2), 0xfffc);
        }
        for (index, (word0, word1)) in commands.iter().copied().enumerate() {
            let command = DISPLAY_LIST_ADDRESS + u32::try_from(index * 8).unwrap();
            view.write_u32(addr(command), word0);
            view.write_u32(addr(command + 4), word1);
        }
        for region in MEMORY_REGIONS {
            view.write_u32(addr(region.guarded_start()), GUARD_WORD);
            view.write_u32(addr(region.end()), GUARD_WORD);
        }

        Ok(InstalledVector {
            case: self.id,
            command_count: commands.len(),
            display_list_bytes: u32::try_from(commands.len() * 8).unwrap(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstalledVector {
    pub case: CaseId,
    pub command_count: usize,
    pub display_list_bytes: u32,
}

impl InstalledVector {
    pub fn guards_unchanged(self, rdram: &[u8]) -> Result<bool, VectorError> {
        if rdram.len() < RDRAM_BYTES {
            return Err(VectorError::RdramTooSmall {
                actual: rdram.len(),
                required: RDRAM_BYTES,
            });
        }
        let view = RdramView::from_storage(rdram);
        Ok(MEMORY_REGIONS.into_iter().all(|region| {
            view.read_u32(addr(region.guarded_start())) == GUARD_WORD
                && view.read_u32(addr(region.end())) == GUARD_WORD
        }))
    }
}

#[derive(Clone, Copy)]
enum LightMode {
    Disabled,
    Directional,
    Candidate(TransferWidth),
}

#[derive(Clone, Copy)]
struct SceneSetup {
    light_mode: LightMode,
    normal: [i8; 3],
    vertices: [[i16; 3]; 3],
    model_translation: [i16; 3],
    candidate_record: [u8; 32],
}

impl SceneSetup {
    fn for_case(id: CaseId) -> Result<Self, VectorError> {
        let mut candidate_record = candidate_signature();
        let mut vertices = origin_vertices();
        let mut model_translation = [0; 3];
        let (light_mode, normal) = match id {
            CaseId::LightingDisabledControl => {
                candidate_record = directional_record();
                (LightMode::Disabled, [0x40, 0x20, 0x10])
            }
            CaseId::DirectionalLightControl => {
                candidate_record = directional_record();
                (LightMode::Directional, [0, 0, 127])
            }
            CaseId::Point { point, width } => {
                match point {
                    PointCase::ZeroDistance | PointCase::RecordBoundary => {}
                    PointCase::PositiveAxis => model_translation = [8, 0, 0],
                    PointCase::NegativeAxis => model_translation = [-8, 0, 0],
                    PointCase::NearDistance => {
                        vertices = translated_local_vertices([0, 0, 1]);
                        model_translation = [0, 0, 3];
                    }
                    PointCase::FarDistance => {
                        vertices = translated_local_vertices([0, 0, 8]);
                        model_translation = [0, 0, 12];
                    }
                }
                (LightMode::Candidate(width), [0, 0, 127])
            }
            CaseId::Knockout { width, group } => {
                require_group(width, group)?;
                candidate_record[group_range(group)].fill(0);
                (LightMode::Candidate(width), [0, 0, 127])
            }
            CaseId::Refinement {
                width,
                group,
                level,
            } => {
                require_group(width, group)?;
                candidate_record[group_range(group)].copy_from_slice(&probe_pattern(group, level));
                (LightMode::Candidate(width), [0, 0, 127])
            }
        };
        Ok(Self {
            light_mode,
            normal,
            vertices,
            model_translation,
            candidate_record,
        })
    }
}

/// The first vertex is at the hypothesized candidate-light origin. This is an
/// experiment coordinate frame, not an admitted F3DZEX2 record interpretation.
const fn origin_vertices() -> [[i16; 3]; 3] {
    [[0, 0, 0], [-6, -4, 0], [6, -4, 0]]
}

fn translated_local_vertices(delta: [i16; 3]) -> [[i16; 3]; 3] {
    origin_vertices().map(|position| {
        [
            position[0] + delta[0],
            position[1] + delta[1],
            position[2] + delta[2],
        ]
    })
}

fn require_group(width: TransferWidth, group: RecordFieldGroup) -> Result<(), VectorError> {
    width
        .contains(group)
        .then_some(())
        .ok_or(VectorError::FieldOutsideEnvelope { width, group })
}

fn group_range(group: RecordFieldGroup) -> std::ops::Range<usize> {
    let start = usize::from(group.offset());
    start..start + 4
}

fn candidate_signature() -> [u8; 32] {
    [
        0xd1, 0x83, 0x47, 0x11, 0xd1, 0x83, 0x47, 0x22, 0x00, 0x00, 0x7f, 0x33, 0x01, 0x00, 0x20,
        0x44, 0x7f, 0x00, 0x80, 0x55, 0x00, 0x01, 0xff, 0x66, 0x12, 0x34, 0x56, 0x77, 0xfe, 0xdc,
        0xba, 0x88,
    ]
}

fn directional_record() -> [u8; 32] {
    let mut record = [0; 32];
    record[..8].copy_from_slice(&[0xc0, 0xc0, 0xc0, 0, 0xc0, 0xc0, 0xc0, 0]);
    record[8..12].copy_from_slice(&[0, 0, 127, 0]);
    record
}

fn ambient_record() -> [u8; 32] {
    let mut record = [0; 32];
    record[..8].copy_from_slice(&[0x10, 0x10, 0x10, 0, 0x10, 0x10, 0x10, 0]);
    record
}

fn probe_pattern(group: RecordFieldGroup, level: ProbeLevel) -> [u8; 4] {
    match (group, level) {
        (RecordFieldGroup::Bytes00To03 | RecordFieldGroup::Bytes04To07, ProbeLevel::Low) => {
            [1, 2, 3, 0]
        }
        (
            RecordFieldGroup::Bytes00To03 | RecordFieldGroup::Bytes04To07,
            ProbeLevel::SignBoundary,
        ) => [0x7f, 0x80, 0xff, 0],
        (RecordFieldGroup::Bytes00To03 | RecordFieldGroup::Bytes04To07, ProbeLevel::High) => {
            [0xff, 0xfe, 0xfd, 0xff]
        }
        (RecordFieldGroup::Bytes08To11, ProbeLevel::Low) => [1, 0, 0, 0],
        (RecordFieldGroup::Bytes08To11, ProbeLevel::SignBoundary) => [0x7f, 0x80, 0, 0],
        (RecordFieldGroup::Bytes08To11, ProbeLevel::High) => [0xff, 0xff, 0x81, 0xff],
        (_, ProbeLevel::Low) => [0, 0, 0, 1],
        (_, ProbeLevel::SignBoundary) => [0x7f, 0xff, 0x80, 0],
        (_, ProbeLevel::High) => [0xff; 4],
    }
}

fn build_commands(setup: SceneSetup) -> Vec<(u32, u32)> {
    let op = |opcode: u8| u32::from(opcode) << 24;
    let mut commands = vec![
        (op(G_MOVEMEM) | (1 << 19) | G_MV_VIEWPORT, VIEWPORT_ADDRESS),
        (op(G_SETZIMG), DEPTH_ADDRESS),
        (op(G_SETCIMG) | (2 << 19) | (WIDTH - 1), OUTPUT_ADDRESS),
        (op(G_SETSCISSOR), ((WIDTH * 4) << 12) | (HEIGHT * 4)),
        (op(G_RDPSETOTHERMODE) | 0x0030_00f0, 0),
        (op(G_SETFILLCOLOR), 0x0001_0001),
        (
            op(G_FILLRECT) | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
            0,
        ),
        (op(G_RDPPIPESYNC), 0),
        (op(G_MTX) | (7 << 19) | 0x07, PROJECTION_ADDRESS),
        (op(G_MTX) | (7 << 19) | 0x03, MODELVIEW_ADDRESS),
    ];

    let geometry_mode = G_SHADE
        | G_SHADING_SMOOTH
        | match setup.light_mode {
            LightMode::Disabled => 0,
            LightMode::Directional => G_LIGHTING,
            LightMode::Candidate(_) => G_LIGHTING | G_POINT_LIGHTING,
        };
    commands.push((op(G_GEOMETRYMODE), geometry_mode));

    match setup.light_mode {
        LightMode::Disabled => {}
        LightMode::Directional => {
            commands.push((op(G_MOVEWORD) | (G_MW_NUMLIGHT << 16), 24));
            commands.push((
                movemem_light_word(TransferWidth::Bytes16, 6),
                CANDIDATE_LIGHT_ADDRESS,
            ));
            commands.push((
                movemem_light_word(TransferWidth::Bytes16, 9),
                AMBIENT_LIGHT_ADDRESS,
            ));
        }
        LightMode::Candidate(width) => {
            commands.push((op(G_MOVEWORD) | (G_MW_NUMLIGHT << 16), 24));
            commands.push((movemem_light_word(width, 6), CANDIDATE_LIGHT_ADDRESS));
            commands.push((
                movemem_light_word(TransferWidth::Bytes16, 9),
                AMBIENT_LIGHT_ADDRESS,
            ));
        }
    }

    commands.extend([
        shade_combine_command(),
        (op(G_RDPSETOTHERMODE) | 0x0000_00f0, 0),
        (op(G_VTX) | (3 << 12) | (3 << 1), VERTICES_ADDRESS),
        (op(G_TRI1) | (1 << 9) | (2 << 1), 0),
        (op(G_RDPFULLSYNC), 0),
        (op(G_ENDDL), 0),
    ]);
    commands
}

fn movemem_light_word(width: TransferWidth, offset_div8: u32) -> u32 {
    let length_div8_minus_one = u32::from(width.bytes() / 8 - 1);
    (u32::from(G_MOVEMEM) << 24) | (length_div8_minus_one << 19) | (offset_div8 << 8) | G_MV_LIGHT
}

fn shade_combine_command() -> (u32, u32) {
    // (ZERO - ZERO) * ZERO + SHADE in both RGB/alpha and both cycles.
    combine_command([15, 15, 31, 4], [7, 7, 7, 4], [15, 15, 31, 4], [7, 7, 7, 4])
}

fn combine_command(
    rgb0: [u32; 4],
    alpha0: [u32; 4],
    rgb1: [u32; 4],
    alpha1: [u32; 4],
) -> (u32, u32) {
    let w0 = (u32::from(G_SETCOMBINE) << 24)
        | ((rgb0[0] & 0x0f) << 20)
        | ((rgb0[2] & 0x1f) << 15)
        | ((alpha0[0] & 0x07) << 12)
        | ((alpha0[2] & 0x07) << 9)
        | ((rgb1[0] & 0x0f) << 5)
        | (rgb1[2] & 0x1f);
    let w1 = ((rgb0[1] & 0x0f) << 28)
        | ((rgb1[1] & 0x0f) << 24)
        | ((alpha1[0] & 0x07) << 21)
        | ((alpha1[2] & 0x07) << 18)
        | ((rgb0[3] & 0x07) << 15)
        | ((alpha0[1] & 0x07) << 12)
        | ((alpha0[3] & 0x07) << 9)
        | ((rgb1[3] & 0x07) << 6)
        | ((alpha1[1] & 0x07) << 3)
        | (alpha1[3] & 0x07);
    (w0, w1)
}

fn write_vertices(view: &mut RdramViewMut<'_>, vertices: [[i16; 3]; 3], normal: [i8; 3]) {
    for (index, position) in vertices.into_iter().enumerate() {
        let base = VERTICES_ADDRESS + u32::try_from(index * 16).unwrap();
        for (axis, value) in position.into_iter().enumerate() {
            view.write_u16(addr(base + u32::try_from(axis * 2).unwrap()), value as u16);
        }
        for (axis, value) in normal.into_iter().enumerate() {
            view.write_u8(addr(base + 12 + u32::try_from(axis).unwrap()), value as u8);
        }
        view.write_u8(addr(base + 15), 0xff);
    }
}

fn write_viewport(view: &mut RdramViewMut<'_>) {
    for (index, value) in [
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        view.write_u16(
            addr(VIEWPORT_ADDRESS + u32::try_from(index * 2).unwrap()),
            value as u16,
        );
    }
}

fn write_matrix_diagonal(view: &mut RdramViewMut<'_>, base: u32, diagonal: [i32; 4]) {
    for (row, diagonal_value) in diagonal.into_iter().enumerate() {
        for column in 0..4 {
            let value = if row == column { diagonal_value } else { 0 };
            let index = u32::try_from(row * 4 + column).unwrap();
            view.write_u16(addr(base + index * 2), (value >> 16) as u16);
            view.write_u16(addr(base + 32 + index * 2), value as u16);
        }
    }
}

fn write_modelview(view: &mut RdramViewMut<'_>, translation: [i16; 3]) {
    let mut elements = [0_i32; 16];
    elements[0] = 65536;
    elements[5] = 65536;
    elements[10] = 65536;
    elements[15] = 65536;
    for (axis, value) in translation.into_iter().enumerate() {
        elements[12 + axis] = i32::from(value) * 65536;
    }
    for (index, value) in elements.into_iter().enumerate() {
        let index = u32::try_from(index).unwrap();
        view.write_u16(addr(MODELVIEW_ADDRESS + index * 2), (value >> 16) as u16);
        view.write_u16(addr(MODELVIEW_ADDRESS + 32 + index * 2), value as u16);
    }
}

const fn addr(offset: u32) -> RdramAddr {
    RdramAddr::from_offset(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logical_region(rdram: &[u8], region: MemoryRegion) -> Vec<u8> {
        let mut bytes = vec![0; region.len as usize];
        RdramView::from_storage(rdram).copy_logical_bytes(addr(region.start), &mut bytes);
        bytes
    }

    fn owned_bytes(rdram: &[u8]) -> Vec<u8> {
        MEMORY_REGIONS
            .into_iter()
            .flat_map(|region| logical_region(rdram, region))
            .collect()
    }

    #[test]
    fn exact_sorted_denominator_and_adaptive_batches_have_canonical_coverage() {
        let suite = CharacterizationSuite;
        assert_eq!(
            RequiredCase::ALL.map(RequiredCase::name),
            [
                "directional-light-control",
                "lighting-disabled-control",
                "point-light-far-distance",
                "point-light-near-distance",
                "point-light-negative-axis",
                "point-light-positive-axis",
                "point-light-record-boundary",
                "point-light-zero-distance",
            ]
        );
        let initial: Vec<_> = suite
            .initial_cases()
            .into_iter()
            .map(|case| case.id().name())
            .collect();
        assert_eq!(
            initial,
            [
                "directional-light-control",
                "lighting-disabled-control",
                "point-light-far-distance-transfer-16",
                "point-light-far-distance-transfer-24",
                "point-light-far-distance-transfer-32",
                "point-light-near-distance-transfer-16",
                "point-light-near-distance-transfer-24",
                "point-light-near-distance-transfer-32",
                "point-light-negative-axis-transfer-16",
                "point-light-negative-axis-transfer-24",
                "point-light-negative-axis-transfer-32",
                "point-light-positive-axis-transfer-16",
                "point-light-positive-axis-transfer-24",
                "point-light-positive-axis-transfer-32",
                "point-light-record-boundary-transfer-16",
                "point-light-record-boundary-transfer-24",
                "point-light-record-boundary-transfer-32",
                "point-light-zero-distance-transfer-16",
                "point-light-zero-distance-transfer-24",
                "point-light-zero-distance-transfer-32",
            ]
        );
        let mut mapped = suite
            .initial_cases()
            .into_iter()
            .map(|case| case.id().denominator_case())
            .collect::<Vec<_>>();
        mapped.sort_unstable();
        mapped.dedup();
        assert_eq!(mapped, RequiredCase::ALL);

        let knockouts = suite.knockout_cases(&[
            TransferWidth::Bytes24,
            TransferWidth::Bytes16,
            TransferWidth::Bytes24,
        ]);
        assert_eq!(knockouts.len(), 10);
        assert_eq!(
            knockouts.first().unwrap().id().name(),
            "candidate-transfer-16-without-bytes00-03"
        );
        assert_eq!(
            knockouts.last().unwrap().id().name(),
            "candidate-transfer-24-without-bytes20-23"
        );
        assert!(knockouts.iter().all(|case| {
            case.id().denominator_case() == RequiredCase::PointLightRecordBoundary
        }));

        let fields = [
            ResponsiveField {
                width: TransferWidth::Bytes24,
                group: RecordFieldGroup::Bytes20To23,
            },
            ResponsiveField {
                width: TransferWidth::Bytes16,
                group: RecordFieldGroup::Bytes08To11,
            },
        ];
        let refinements = suite.refinement_cases(&fields).unwrap();
        assert_eq!(refinements.len(), 6);
        assert!(refinements.iter().all(|case| {
            case.id().denominator_case() == RequiredCase::PointLightRecordBoundary
        }));
        assert_eq!(
            refinements
                .into_iter()
                .map(|case| case.id().name())
                .collect::<Vec<_>>(),
            [
                "candidate-transfer-16-bytes08-11-low",
                "candidate-transfer-16-bytes08-11-sign-boundary",
                "candidate-transfer-16-bytes08-11-high",
                "candidate-transfer-24-bytes20-23-low",
                "candidate-transfer-24-bytes20-23-sign-boundary",
                "candidate-transfer-24-bytes20-23-high",
            ]
        );
    }

    #[test]
    fn invalid_adaptive_field_is_rejected_loudly() {
        let error = CharacterizationSuite
            .refinement_cases(&[ResponsiveField {
                width: TransferWidth::Bytes16,
                group: RecordFieldGroup::Bytes16To19,
            }])
            .unwrap_err();
        assert!(matches!(error, VectorError::FieldOutsideEnvelope { .. }));
        assert!(VectorCase::new(CaseId::Knockout {
            width: TransferWidth::Bytes16,
            group: RecordFieldGroup::Bytes28To31,
        })
        .commands()
        .is_err());
    }

    #[test]
    fn every_declared_case_generates_identical_bytes_on_repetition() {
        let suite = CharacterizationSuite;
        let fields: Vec<_> = TransferWidth::ALL
            .into_iter()
            .flat_map(|width| {
                RecordFieldGroup::ALL
                    .into_iter()
                    .filter(move |group| width.contains(*group))
                    .map(move |group| ResponsiveField { width, group })
            })
            .collect();
        let cases = suite
            .initial_cases()
            .into_iter()
            .chain(suite.knockout_cases(&TransferWidth::ALL))
            .chain(suite.refinement_cases(&fields).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 20 + 18 + 54);

        for case in cases {
            let mut first = vec![0x5a; RDRAM_BYTES];
            let mut second = vec![0x5a; RDRAM_BYTES];
            let installed_first = case.install(&mut first).unwrap();
            let installed_second = case.install(&mut second).unwrap();
            assert_eq!(installed_first, installed_second, "{}", case.id().name());
            assert_eq!(
                owned_bytes(&first),
                owned_bytes(&second),
                "{}",
                case.id().name()
            );
            assert!(installed_first.guards_unchanged(&first).unwrap());
        }
    }

    #[test]
    fn regions_and_guards_are_aligned_bounded_and_disjoint() {
        for (index, region) in MEMORY_REGIONS.into_iter().enumerate() {
            assert!(region.start.is_multiple_of(8), "{}", region.name);
            assert!(region.len.is_multiple_of(8), "{}", region.name);
            assert!(region.start >= 4, "{}", region.name);
            assert!(
                region.guarded_end() as usize <= RDRAM_BYTES,
                "{}",
                region.name
            );
            for other in MEMORY_REGIONS.into_iter().skip(index + 1) {
                assert!(
                    region.guarded_end() <= other.guarded_start()
                        || other.guarded_end() <= region.guarded_start(),
                    "guarded regions {} and {} overlap",
                    region.name,
                    other.name
                );
            }
        }
        assert_eq!(FRAMEBUFFER_BYTES, WIDTH * HEIGHT * 2);
    }

    #[test]
    fn every_raster_case_has_exactly_one_fullsync_then_one_enddl() {
        let suite = CharacterizationSuite;
        let all_fields: Vec<_> = TransferWidth::ALL
            .into_iter()
            .flat_map(|width| {
                RecordFieldGroup::ALL
                    .into_iter()
                    .filter(move |group| width.contains(*group))
                    .map(move |group| ResponsiveField { width, group })
            })
            .collect();
        let cases = suite
            .initial_cases()
            .into_iter()
            .chain(suite.knockout_cases(&TransferWidth::ALL))
            .chain(suite.refinement_cases(&all_fields).unwrap());

        for case in cases {
            let commands = case.commands().unwrap();
            let opcodes: Vec<_> = commands
                .iter()
                .map(|(word0, _)| (word0 >> 24) as u8)
                .collect();
            assert_eq!(opcodes.iter().filter(|&&op| op == G_RDPFULLSYNC).count(), 1);
            assert_eq!(opcodes.iter().filter(|&&op| op == G_ENDDL).count(), 1);
            assert_eq!(
                commands[commands.len() - 2],
                (u32::from(G_RDPFULLSYNC) << 24, 0)
            );
            assert_eq!(commands.last(), Some(&(u32::from(G_ENDDL) << 24, 0)));
        }
    }

    #[test]
    fn point_light_geometry_mode_is_exactly_scoped_to_candidate_cases() {
        let suite = CharacterizationSuite;
        let all_fields: Vec<_> = TransferWidth::ALL
            .into_iter()
            .flat_map(|width| {
                RecordFieldGroup::ALL
                    .into_iter()
                    .filter(move |group| width.contains(*group))
                    .map(move |group| ResponsiveField { width, group })
            })
            .collect();
        let cases = suite
            .initial_cases()
            .into_iter()
            .chain(suite.knockout_cases(&TransferWidth::ALL))
            .chain(suite.refinement_cases(&all_fields).unwrap());

        for case in cases {
            let mode = case
                .commands()
                .unwrap()
                .into_iter()
                .find_map(|(word0, word1)| ((word0 >> 24) as u8 == G_GEOMETRYMODE).then_some(word1))
                .expect("every vector has one geometry-mode command");
            let expected = match case.id() {
                CaseId::LightingDisabledControl => G_SHADE | G_SHADING_SMOOTH,
                CaseId::DirectionalLightControl => G_SHADE | G_SHADING_SMOOTH | G_LIGHTING,
                CaseId::Point { .. } | CaseId::Knockout { .. } | CaseId::Refinement { .. } => {
                    G_SHADE | G_SHADING_SMOOTH | G_LIGHTING | G_POINT_LIGHTING
                }
            };
            assert_eq!(mode, expected, "{}", case.id().name());
        }
    }

    #[test]
    fn every_initial_subcase_installs_one_fullsync_and_enddl() {
        for case in CharacterizationSuite.initial_cases() {
            let required = case.id().denominator_case();
            let commands = case.commands().unwrap();
            assert_eq!(
                commands
                    .iter()
                    .filter(|(word0, _)| (word0 >> 24) as u8 == G_RDPFULLSYNC)
                    .count(),
                1,
                "{}",
                required.name()
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|(word0, _)| (word0 >> 24) as u8 == G_ENDDL)
                    .count(),
                1,
                "{}",
                required.name()
            );
            let mut rdram = vec![0; RDRAM_BYTES];
            let installed = case.install(&mut rdram).unwrap();
            assert!(installed.guards_unchanged(&rdram).unwrap());
        }
    }

    #[test]
    fn spatial_hypotheses_hold_candidate_record_fixed_and_move_geometry() {
        let points = [
            PointCase::ZeroDistance,
            PointCase::PositiveAxis,
            PointCase::NegativeAxis,
            PointCase::NearDistance,
            PointCase::FarDistance,
        ];
        let mut candidate_record = None;
        let mut placements = Vec::new();
        for point in points {
            let mut width_placement = None;
            for width in TransferWidth::ALL {
                let case = CharacterizationSuite
                    .initial_cases()
                    .into_iter()
                    .find(|case| case.id() == CaseId::Point { point, width })
                    .unwrap();
                let mut rdram = vec![0; RDRAM_BYTES];
                case.install(&mut rdram).unwrap();
                let record = logical_region(&rdram, MEMORY_REGIONS[5]);
                if let Some(expected) = &candidate_record {
                    assert_eq!(
                        &record,
                        expected,
                        "{} changed the candidate record",
                        point.denominator_case().name()
                    );
                } else {
                    candidate_record = Some(record);
                }
                let placement = (
                    logical_region(&rdram, MEMORY_REGIONS[1]),
                    logical_region(&rdram, MEMORY_REGIONS[4]),
                );
                if let Some(expected) = &width_placement {
                    assert_eq!(
                        &placement,
                        expected,
                        "{} placement changed across transfer widths",
                        point.denominator_case().name()
                    );
                } else {
                    width_placement = Some(placement);
                }
            }
            placements.push(width_placement.unwrap());
        }
        for (index, placement) in placements.iter().enumerate() {
            assert!(
                placements
                    .iter()
                    .enumerate()
                    .all(|(other_index, other)| other_index == index || other != placement),
                "spatial hypotheses must have distinct vertex/model placement bytes"
            );
        }
    }

    #[test]
    fn guard_regions_detect_both_sides_of_every_owned_region() {
        let case = CharacterizationSuite.initial_cases().remove(0);
        let mut rdram = vec![0; RDRAM_BYTES];
        let installed = case.install(&mut rdram).unwrap();
        assert!(installed.guards_unchanged(&rdram).unwrap());

        for region in MEMORY_REGIONS {
            let mut before = rdram.clone();
            RdramViewMut::from_storage(&mut before).write_u32(addr(region.guarded_start()), 0);
            assert!(
                !installed.guards_unchanged(&before).unwrap(),
                "{} before",
                region.name
            );

            let mut after = rdram.clone();
            RdramViewMut::from_storage(&mut after).write_u32(addr(region.end()), 0);
            assert!(
                !installed.guards_unchanged(&after).unwrap(),
                "{} after",
                region.name
            );
        }
    }

    #[test]
    fn staging_does_not_touch_bytes_outside_owned_regions_and_guards() {
        let mut rdram = vec![0x5a; RDRAM_BYTES];
        let case = CharacterizationSuite
            .initial_cases()
            .into_iter()
            .find(|case| {
                case.id()
                    == CaseId::Point {
                        point: PointCase::RecordBoundary,
                        width: TransferWidth::Bytes32,
                    }
            })
            .unwrap();
        case.install(&mut rdram).unwrap();
        for address in [0, 0x10000, 0x20000, 0x2_0000, RDRAM_BYTES - 1] {
            assert_eq!(rdram[address], 0x5a, "storage byte {address:#x} changed");
        }
    }
}
