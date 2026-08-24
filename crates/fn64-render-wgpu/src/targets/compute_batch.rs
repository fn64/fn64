//! Typed admission for the first exact compute-raster program.
//!
//! This module owns no GPU execution yet. It turns the census-selected wire
//! state into a closed program key and seals ordered draw/journal/TMEM facts
//! into one move-only value. The later shader path can consume that value;
//! it cannot broaden admission by inspecting raw words again.

use fn64_render_ir::{AccessMode, AccessPurpose, ResourceAccess, ResourceRegion};

use crate::{CombineParams, CycleType, OtherMode, TmemSnapshotIdentity};

use super::{ColorTargetFormat, ColorTargetKey, TargetGeneration};

pub(crate) const HOT_COMBINE_LOW: u32 = 0xfc51_96a3;
pub(crate) const HOT_COMBINE_HIGH: u32 = 0x112c_fe7f;
pub(crate) const HOT_OTHER_MODE_HIGH: u32 = 0x0008_acef;
pub(crate) const HOT_OTHER_MODE_LOW: u32 = 0x0050_41c8;

/// The one complete RDP program admitted by the first compute prototype.
/// Raw halves are retained because a shader pipeline key must distinguish
/// every bit, including bits that happen to be dead for this program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ComputeRasterProgramKey {
    combine_low: u32,
    combine_high: u32,
    other_mode_high: u32,
    other_mode_low: u32,
}

impl ComputeRasterProgramKey {
    pub(crate) fn try_admit(
        target: ColorTargetKey,
        combine: CombineParams,
        other_mode: OtherMode,
        textured: bool,
    ) -> Result<Self, ComputeRasterAdmissionRefusal> {
        if target.format() != ColorTargetFormat::Rgba16 {
            return Err(ComputeRasterAdmissionRefusal::TargetFormat);
        }
        if !textured {
            return Err(ComputeRasterAdmissionRefusal::Untextured);
        }
        if !other_mode.texture_perspective() {
            return Err(ComputeRasterAdmissionRefusal::AffineTexture);
        }
        if other_mode.depth_compare_enabled() || other_mode.depth_update_enabled() {
            return Err(ComputeRasterAdmissionRefusal::Depth);
        }
        if other_mode.cycle_type() != CycleType::OneCycle {
            return Err(ComputeRasterAdmissionRefusal::CycleType);
        }
        if combine.low() != HOT_COMBINE_LOW
            || combine.high() != HOT_COMBINE_HIGH
            || other_mode.high() != HOT_OTHER_MODE_HIGH
            || other_mode.low() != HOT_OTHER_MODE_LOW
        {
            return Err(ComputeRasterAdmissionRefusal::ProgramBits([
                combine.low(),
                combine.high(),
                other_mode.high(),
                other_mode.low(),
            ]));
        }
        Ok(Self {
            combine_low: combine.low(),
            combine_high: combine.high(),
            other_mode_high: other_mode.high(),
            other_mode_low: other_mode.low(),
        })
    }

    pub(crate) const fn words(self) -> [u32; 4] {
        [
            self.combine_low,
            self.combine_high,
            self.other_mode_high,
            self.other_mode_low,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ComputeRasterAdmissionRefusal {
    TargetFormat,
    Untextured,
    AffineTexture,
    Depth,
    CycleType,
    ProgramBits([u32; 4]),
    EmptyAccesses,
    AccessMode,
    AccessPurpose,
    AccessRegion,
    AccessOutsideTarget,
    CommandOrder,
}

/// One draw's complete batch-facing authority. No raw command or mutable
/// state is retained: the program, TMEM snapshot, and exact journal accesses
/// are already resolved at this draw's stream position.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ComputeRasterDrawAdmission {
    command_index: u32,
    triangle_index: usize,
    program: ComputeRasterProgramKey,
    tmem: TmemSnapshotIdentity,
    accesses: Box<[ResourceAccess]>,
}

impl ComputeRasterDrawAdmission {
    pub(crate) fn try_new(
        target: ColorTargetKey,
        command_index: u32,
        triangle_index: usize,
        program: ComputeRasterProgramKey,
        tmem: TmemSnapshotIdentity,
        accesses: Vec<ResourceAccess>,
    ) -> Result<Self, ComputeRasterAdmissionRefusal> {
        if accesses.is_empty() {
            return Err(ComputeRasterAdmissionRefusal::EmptyAccesses);
        }
        for access in &accesses {
            if access.mode() != AccessMode::Write {
                return Err(ComputeRasterAdmissionRefusal::AccessMode);
            }
            if access.purpose() != AccessPurpose::RenderTarget {
                return Err(ComputeRasterAdmissionRefusal::AccessPurpose);
            }
            let ResourceRegion::Rdram { range, .. } = access.region() else {
                return Err(ComputeRasterAdmissionRefusal::AccessRegion);
            };
            if range.start().get() < target.range().start().get()
                || range.end() > target.range().end()
            {
                return Err(ComputeRasterAdmissionRefusal::AccessOutsideTarget);
            }
        }
        Ok(Self {
            command_index,
            triangle_index,
            program,
            tmem,
            accesses: accesses.into_boxed_slice(),
        })
    }

    pub(crate) const fn command_index(&self) -> u32 {
        self.command_index
    }

    pub(crate) const fn triangle_index(&self) -> usize {
        self.triangle_index
    }

    pub(crate) const fn program(&self) -> ComputeRasterProgramKey {
        self.program
    }

    pub(crate) const fn tmem(&self) -> TmemSnapshotIdentity {
        self.tmem
    }

    pub(crate) fn accesses(&self) -> &[ResourceAccess] {
        &self.accesses
    }
}

/// Move-only batch sealed to one target generation. There is deliberately no
/// `Clone`: target-generation and journal authority must have one consumer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ComputeRasterBatch {
    target: ColorTargetKey,
    generation: TargetGeneration,
    draws: Box<[ComputeRasterDrawAdmission]>,
}

impl ComputeRasterBatch {
    pub(crate) const fn target(&self) -> ColorTargetKey {
        self.target
    }

    pub(crate) const fn generation(&self) -> TargetGeneration {
        self.generation
    }

    pub(crate) fn draws(&self) -> &[ComputeRasterDrawAdmission] {
        &self.draws
    }
}

pub(crate) struct ComputeRasterBatchBuilder {
    target: ColorTargetKey,
    generation: TargetGeneration,
    draws: Vec<ComputeRasterDrawAdmission>,
}

impl ComputeRasterBatchBuilder {
    pub(crate) const fn new(target: ColorTargetKey, generation: TargetGeneration) -> Self {
        Self {
            target,
            generation,
            draws: Vec::new(),
        }
    }

    pub(crate) fn push(
        &mut self,
        draw: ComputeRasterDrawAdmission,
    ) -> Result<(), ComputeRasterAdmissionRefusal> {
        if self
            .draws
            .last()
            .is_some_and(|previous| previous.command_index() >= draw.command_index())
        {
            return Err(ComputeRasterAdmissionRefusal::CommandOrder);
        }
        self.draws.push(draw);
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<ComputeRasterBatch, ComputeRasterAdmissionRefusal> {
        if self.draws.is_empty() {
            return Err(ComputeRasterAdmissionRefusal::EmptyAccesses);
        }
        Ok(ComputeRasterBatch {
            target: self.target,
            generation: self.generation,
            draws: self.draws.into_boxed_slice(),
        })
    }
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        AccessMode, AccessPurpose, OperationId, PhysicalMemoryLayout, RdramResource,
        ResourceAccess, ResourceRegion,
    };

    use crate::{PhysicalTmemState, TmemByteSource};

    use super::*;
    use crate::targets::ColorTargetExtent;

    fn target(format: ColorTargetFormat) -> ColorTargetKey {
        let layout = PhysicalMemoryLayout::try_new(0x80_0000).unwrap();
        ColorTargetKey::try_new(
            layout.address(0x20_0000).unwrap(),
            ColorTargetExtent::try_new(320, 240).unwrap(),
            format,
        )
        .unwrap()
    }

    fn hot_program(target: ColorTargetKey) -> ComputeRasterProgramKey {
        ComputeRasterProgramKey::try_admit(
            target,
            CombineParams::from_wire(HOT_COMBINE_LOW, HOT_COMBINE_HIGH),
            OtherMode::from_wire(HOT_OTHER_MODE_HIGH, HOT_OTHER_MODE_LOW),
            true,
        )
        .unwrap()
    }

    fn access(target: ColorTargetKey, operation: u32) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: target
                    .address()
                    .layout()
                    .range(target.address().get(), target.address().get() + 640)
                    .unwrap(),
            },
        )
        .unwrap()
    }

    fn draw(target: ColorTargetKey, command: u32) -> ComputeRasterDrawAdmission {
        let tmem = PhysicalTmemState::try_new().unwrap();
        ComputeRasterDrawAdmission::try_new(
            target,
            command,
            command as usize,
            hot_program(target),
            tmem.snapshot(),
            vec![access(target, command)],
        )
        .unwrap()
    }

    #[test]
    fn only_the_complete_hottest_census_key_is_admitted() {
        let rgba16_target = target(ColorTargetFormat::Rgba16);
        let key = hot_program(rgba16_target);
        assert_eq!(
            key.words(),
            [
                HOT_COMBINE_LOW,
                HOT_COMBINE_HIGH,
                HOT_OTHER_MODE_HIGH,
                HOT_OTHER_MODE_LOW
            ]
        );
        assert_eq!(
            ComputeRasterProgramKey::try_admit(
                rgba16_target,
                CombineParams::from_wire(HOT_COMBINE_LOW ^ 1, HOT_COMBINE_HIGH),
                OtherMode::from_wire(HOT_OTHER_MODE_HIGH, HOT_OTHER_MODE_LOW),
                true,
            ),
            Err(ComputeRasterAdmissionRefusal::ProgramBits([
                HOT_COMBINE_LOW ^ 1,
                HOT_COMBINE_HIGH,
                HOT_OTHER_MODE_HIGH,
                HOT_OTHER_MODE_LOW,
            ]))
        );
        assert_eq!(
            ComputeRasterProgramKey::try_admit(
                target(ColorTargetFormat::Rgba32),
                CombineParams::from_wire(HOT_COMBINE_LOW, HOT_COMBINE_HIGH),
                OtherMode::from_wire(HOT_OTHER_MODE_HIGH, HOT_OTHER_MODE_LOW),
                true,
            ),
            Err(ComputeRasterAdmissionRefusal::TargetFormat)
        );
    }

    #[test]
    fn batch_seals_target_generation_order_tmem_and_exact_accesses() {
        let target = target(ColorTargetFormat::Rgba16);
        let mut builder = ComputeRasterBatchBuilder::new(target, TargetGeneration::FIRST);
        builder.push(draw(target, 4)).unwrap();
        builder.push(draw(target, 9)).unwrap();
        assert_eq!(
            builder.push(draw(target, 9)),
            Err(ComputeRasterAdmissionRefusal::CommandOrder)
        );
        let batch = builder.finish().unwrap();
        assert_eq!(batch.target(), target);
        assert_eq!(batch.generation(), TargetGeneration::FIRST);
        assert_eq!(batch.draws().len(), 2);
        assert_eq!(batch.draws()[0].command_index(), 4);
        assert_eq!(batch.draws()[1].triangle_index(), 9);
        assert!(batch.draws()[0].tmem().is_committed());
        assert_eq!(batch.draws()[0].accesses(), &[access(target, 4)]);
        assert_eq!(batch.draws()[0].program(), hot_program(target));
    }
}
