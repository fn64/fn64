//! Bounded decoding for the first admitted raw-DPC command subset.

use core::fmt;

use fn64_render::raw_rdp_command_width;
use fn64_render_ir::{
    AccessMode, AccessPurpose, DmemRange, FullSyncOccurrence, OperationId, RawCommandStream,
    RawStreamIdentity, RdramResource, ResourceAccess, ResourceRegion, SubmittedTicket,
    ValidationError, WorkloadAdmission, WorkloadIdentity, MAX_RESOURCE_ACCESSES,
};

use crate::state::{
    ColorImage, CycleType, FillColor, ImageFormat, OtherMode, PixelSize, RdpState, RdpStateDelta,
    StagedRdpState,
};

const SET_OTHER_MODE: u8 = 0x2f;
const SET_COLOR_IMAGE: u8 = 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;
const FULL_SYNC: u8 = 0x29;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDpcCommandLocation {
    workload: WorkloadIdentity,
    stream: RawStreamIdentity,
    stream_index: u32,
    chunk_index: u32,
    stream_byte_offset: u32,
    source_byte_offset: u32,
    wire_opcode: u8,
}

impl RawDpcCommandLocation {
    pub const fn workload(self) -> WorkloadIdentity {
        self.workload
    }

    pub const fn stream(self) -> RawStreamIdentity {
        self.stream
    }

    pub const fn stream_index(self) -> u32 {
        self.stream_index
    }

    pub const fn chunk_index(self) -> u32 {
        self.chunk_index
    }

    pub const fn stream_byte_offset(self) -> u32 {
        self.stream_byte_offset
    }

    pub const fn source_byte_offset(self) -> u32 {
        self.source_byte_offset
    }

    pub const fn wire_opcode(self) -> u8 {
        self.wire_opcode
    }
}

impl fmt::Display for RawDpcCommandLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workload {} stream {} (index {}) chunk {} source byte offset {:#x} stream byte offset {:#x} wire opcode {:#04x}",
            self.workload,
            self.stream,
            self.stream_index,
            self.chunk_index,
            self.source_byte_offset,
            self.stream_byte_offset,
            self.wire_opcode,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillRectangle {
    upper_left_x: u16,
    upper_left_y: u16,
    lower_right_x: u16,
    lower_right_y: u16,
}

impl FillRectangle {
    pub const fn upper_left_x(self) -> u16 {
        self.upper_left_x
    }

    pub const fn upper_left_y(self) -> u16 {
        self.upper_left_y
    }

    pub const fn lower_right_x(self) -> u16 {
        self.lower_right_x
    }

    pub const fn lower_right_y(self) -> u16 {
        self.lower_right_y
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawDpcCommandKind {
    NoOp { variant: u8 },
    SetOtherMode(OtherMode),
    SetColorImage(ColorImage),
    SetFillColor(FillColor),
    FillRectangle(FillRectangle),
    FullSync(FullSyncOccurrence),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedRawDpcCommand {
    location: RawDpcCommandLocation,
    kind: RawDpcCommandKind,
}

impl DecodedRawDpcCommand {
    pub const fn location(self) -> RawDpcCommandLocation {
        self.location
    }

    pub const fn kind(self) -> RawDpcCommandKind {
        self.kind
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RawDpcResourcePlan {
    accesses: Box<[ResourceAccess]>,
}

impl RawDpcResourcePlan {
    pub fn accesses(&self) -> &[ResourceAccess] {
        &self.accesses
    }
}

#[derive(Debug)]
pub struct DecodedRawDpc {
    submitted: SubmittedTicket,
    commands: Box<[DecodedRawDpcCommand]>,
    state_delta: RdpStateDelta,
    staged_state: StagedRdpState,
    resource_plan: RawDpcResourcePlan,
}

impl DecodedRawDpc {
    pub const fn submitted(&self) -> &SubmittedTicket {
        &self.submitted
    }

    pub fn commands(&self) -> &[DecodedRawDpcCommand] {
        &self.commands
    }

    pub const fn state_delta(&self) -> &RdpStateDelta {
        &self.state_delta
    }

    pub const fn staged_state(&self) -> &StagedRdpState {
        &self.staged_state
    }

    pub const fn resource_plan(&self) -> &RawDpcResourcePlan {
        &self.resource_plan
    }

    pub fn into_staged_state(self) -> StagedRdpState {
        self.staged_state
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RawDpcDecodeError {
    UnsupportedAdmission {
        workload: WorkloadIdentity,
    },
    StagedStateMismatch {
        workload: WorkloadIdentity,
        reason: &'static str,
    },
    UnknownCommandWidth {
        location: RawDpcCommandLocation,
    },
    TruncatedCommand {
        location: RawDpcCommandLocation,
        width: u32,
        available: u32,
    },
    UnsupportedCommand {
        location: RawDpcCommandLocation,
        decoded_opcode: u8,
        width: u32,
    },
    InvalidCommand {
        location: RawDpcCommandLocation,
        reason: &'static str,
    },
    ResourcePlanOverflow {
        workload: WorkloadIdentity,
    },
    JournalMismatch {
        workload: WorkloadIdentity,
        expected: Box<[ResourceAccess]>,
        actual: Box<[ResourceAccess]>,
    },
    Ir(ValidationError),
}

impl fmt::Display for RawDpcDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedAdmission { workload } => {
                write!(formatter, "workload {workload} is not admitted as raw DPC")
            }
            Self::StagedStateMismatch { workload, reason } => {
                write!(formatter, "workload {workload} cannot chain staged RDP state: {reason}")
            }
            Self::UnknownCommandWidth { location } => {
                write!(formatter, "{location}: command has no admitted public width")
            }
            Self::TruncatedCommand {
                location,
                width,
                available,
            } => write!(
                formatter,
                "{location}: {width}-byte command is truncated with {available} bytes available"
            ),
            Self::UnsupportedCommand {
                location,
                decoded_opcode,
                width,
            } => write!(
                formatter,
                "{location}: decoded opcode {decoded_opcode:#04x} has public width {width} but is outside the M3.2 subset"
            ),
            Self::InvalidCommand { location, reason } => {
                write!(formatter, "{location}: state-invalid command: {reason}")
            }
            Self::ResourcePlanOverflow { workload } => {
                write!(formatter, "workload {workload} resource-plan operation IDs overflow u32")
            }
            Self::JournalMismatch {
                workload,
                expected,
                actual,
            } => write!(
                formatter,
                "workload {workload} resource journal differs from exact decoder plan: expected {} accesses, found {}",
                expected.len(),
                actual.len()
            ),
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RawDpcDecodeError {}

impl From<ValidationError> for RawDpcDecodeError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

pub fn decode_raw_dpc(
    submitted: SubmittedTicket,
    durable_state: &RdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    decode_from_state(submitted, durable_state.fork_for_decode())
}

pub fn decode_raw_dpc_after(
    submitted: SubmittedTicket,
    staged_state: StagedRdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    let workload = submitted.packet().identity();
    let (state, queue, prior_ordinal, prior_sequence) = staged_state.into_parts();
    if submitted.queue() != queue {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "submission queue differs from the staged-state owner",
        });
    }
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = submitted.packet().admission()
    else {
        return Err(RawDpcDecodeError::UnsupportedAdmission { workload });
    };
    if prior_ordinal.checked_add(1) != Some(submitted.ordinal()) {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "semantic submission is not the immediate successor",
        });
    }
    if prior_sequence.checked_add(1) != Some(transaction_sequence) {
        return Err(RawDpcDecodeError::StagedStateMismatch {
            workload,
            reason: "raw-DPC transaction sequence is not the immediate successor",
        });
    }
    decode_from_state(submitted, state)
}

fn decode_from_state(
    submitted: SubmittedTicket,
    mut state: RdpState,
) -> Result<DecodedRawDpc, RawDpcDecodeError> {
    let packet = submitted.packet();
    let workload = packet.identity();
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = packet.admission()
    else {
        return Err(RawDpcDecodeError::UnsupportedAdmission { workload });
    };
    let queue = submitted.queue();
    let submission_ordinal = submitted.ordinal();

    let mut planned = Vec::new();
    for stream in packet.streams() {
        let (start, end) = stream.source_bounds();
        let region = match stream {
            RawCommandStream::Dram(_) => ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: packet.memory_layout().range(start, end)?,
            },
            RawCommandStream::Xbus(_) => ResourceRegion::RspDmem(DmemRange::try_new(start, end)?),
        };
        push_access(
            workload,
            &mut planned,
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            region,
        )?;
    }

    let mut delta = RdpStateDelta::default();
    let mut commands = Vec::new();
    for (stream_index, stream) in packet.streams().iter().enumerate() {
        let flattened = FlattenedStream::new(workload, stream_index, stream);
        decode_stream(
            &flattened,
            packet.memory_layout(),
            &mut state,
            &mut delta,
            &mut planned,
            &mut commands,
        )?;
    }

    let actual = packet.journal().accesses();
    if actual != planned {
        return Err(RawDpcDecodeError::JournalMismatch {
            workload,
            expected: planned.into_boxed_slice(),
            actual: actual.to_vec().into_boxed_slice(),
        });
    }
    let resource_accesses = actual.to_vec().into_boxed_slice();
    let staged_state =
        StagedRdpState::from_transaction(state, queue, submission_ordinal, transaction_sequence);

    Ok(DecodedRawDpc {
        submitted,
        commands: commands.into_boxed_slice(),
        state_delta: delta,
        staged_state,
        resource_plan: RawDpcResourcePlan {
            accesses: resource_accesses,
        },
    })
}

#[derive(Clone, Copy)]
struct ChunkSpan {
    index: u32,
    stream_start: u32,
    stream_end: u32,
    source_start: u32,
}

struct FlattenedStream {
    workload: WorkloadIdentity,
    stream: RawStreamIdentity,
    stream_index: u32,
    bytes: Vec<u8>,
    chunks: Vec<ChunkSpan>,
    full_syncs: Vec<FullSyncOccurrence>,
}

impl FlattenedStream {
    fn new(workload: WorkloadIdentity, stream_index: usize, stream: &RawCommandStream) -> Self {
        let mut bytes = Vec::with_capacity(stream.byte_len() as usize);
        let mut chunks = Vec::with_capacity(stream.chunk_count());
        let mut stream_start = 0;
        match stream {
            RawCommandStream::Dram(stream) => {
                for (index, chunk) in stream.chunks().iter().enumerate() {
                    bytes.extend(chunk.words().iter().flat_map(|word| word.to_be_bytes()));
                    let stream_end = stream_start + chunk.range().len();
                    chunks.push(ChunkSpan {
                        index: index as u32,
                        stream_start,
                        stream_end,
                        source_start: chunk.range().start().get(),
                    });
                    stream_start = stream_end;
                }
            }
            RawCommandStream::Xbus(stream) => {
                for (index, chunk) in stream.chunks().iter().enumerate() {
                    bytes.extend_from_slice(chunk.bytes());
                    let stream_end = stream_start + chunk.range().len();
                    chunks.push(ChunkSpan {
                        index: index as u32,
                        stream_start,
                        stream_end,
                        source_start: chunk.range().start(),
                    });
                    stream_start = stream_end;
                }
            }
        }
        Self {
            workload,
            stream: stream.identity(),
            stream_index: stream_index as u32,
            bytes,
            chunks,
            full_syncs: stream.full_sync_occurrences().to_vec(),
        }
    }

    fn location(&self, stream_byte_offset: u32, wire_opcode: u8) -> RawDpcCommandLocation {
        let chunk = self
            .chunks
            .iter()
            .find(|chunk| stream_byte_offset < chunk.stream_end)
            .or_else(|| self.chunks.last())
            .expect("IR command streams are nonempty");
        let within = stream_byte_offset.saturating_sub(chunk.stream_start);
        RawDpcCommandLocation {
            workload: self.workload,
            stream: self.stream,
            stream_index: self.stream_index,
            chunk_index: chunk.index,
            stream_byte_offset,
            source_byte_offset: chunk.source_start + within,
            wire_opcode,
        }
    }
}

fn decode_stream(
    stream: &FlattenedStream,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &mut RdpState,
    delta: &mut RdpStateDelta,
    planned: &mut Vec<ResourceAccess>,
    commands: &mut Vec<DecodedRawDpcCommand>,
) -> Result<(), RawDpcDecodeError> {
    let mut offset = 0usize;
    while offset < stream.bytes.len() {
        let wire_opcode = stream.bytes[offset];
        let location = stream.location(offset as u32, wire_opcode);
        let Some(width) = raw_rdp_command_width(wire_opcode) else {
            return Err(RawDpcDecodeError::UnknownCommandWidth { location });
        };
        let available = stream.bytes.len() - offset;
        if available < width as usize {
            return Err(RawDpcDecodeError::TruncatedCommand {
                location,
                width,
                available: available as u32,
            });
        }
        let command = &stream.bytes[offset..offset + width as usize];
        let w0 = u32::from_be_bytes(command[..4].try_into().expect("width is at least 8"));
        let w1 = u32::from_be_bytes(command[4..8].try_into().expect("width is at least 8"));
        let opcode = wire_opcode & 0x3f;
        let kind = match opcode {
            0x00..=0x07 => RawDpcCommandKind::NoOp { variant: opcode },
            SET_OTHER_MODE => {
                let value = OtherMode::from_wire(w0 & 0x00ff_ffff, w1);
                if value.cycle_type() != CycleType::Fill {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "SetOtherMode does not select fill cycle",
                    });
                }
                delta.set_other_mode(value);
                state.apply(delta);
                RawDpcCommandKind::SetOtherMode(value)
            }
            SET_COLOR_IMAGE => {
                let format = match (w0 >> 21) & 0x7 {
                    0 => ImageFormat::Rgba,
                    1 => ImageFormat::Yuv,
                    2 => ImageFormat::ColorIndex,
                    3 => ImageFormat::IntensityAlpha,
                    4 => ImageFormat::Intensity,
                    _ => {
                        return Err(RawDpcDecodeError::InvalidCommand {
                            location,
                            reason: "SetColorImage format is reserved",
                        });
                    }
                };
                let size = match (w0 >> 19) & 0x3 {
                    0 => PixelSize::Bits4,
                    1 => PixelSize::Bits8,
                    2 => PixelSize::Bits16,
                    _ => PixelSize::Bits32,
                };
                let address = w1 & 0x00ff_ffff;
                if !address.is_multiple_of(64) {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "SetColorImage address is not 64-byte aligned",
                    });
                }
                let address =
                    layout
                        .address(address)
                        .map_err(|_| RawDpcDecodeError::InvalidCommand {
                            location,
                            reason: "SetColorImage address is outside installed RDRAM",
                        })?;
                let value = ColorImage::from_wire(format, size, (w0 & 0x0fff) + 1, address);
                delta.set_color_image(value);
                state.apply(delta);
                RawDpcCommandKind::SetColorImage(value)
            }
            SET_FILL_COLOR => {
                let value = FillColor::from_wire(w1);
                delta.set_fill_color(value);
                state.apply(delta);
                RawDpcCommandKind::SetFillColor(value)
            }
            FILL_RECTANGLE => {
                let rectangle = FillRectangle {
                    upper_left_x: ((w1 >> 12) & 0x0fff) as u16,
                    upper_left_y: (w1 & 0x0fff) as u16,
                    lower_right_x: ((w0 >> 12) & 0x0fff) as u16,
                    lower_right_y: (w0 & 0x0fff) as u16,
                };
                plan_fill(location, rectangle, layout, state, planned)?;
                RawDpcCommandKind::FillRectangle(rectangle)
            }
            FULL_SYNC => {
                let Some(sync) = stream
                    .full_syncs
                    .iter()
                    .find(|sync| sync.stream_byte_offset == offset as u32)
                    .copied()
                else {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "FullSync lacks the matching capture observation",
                    });
                };
                if sync.chunk_index != location.chunk_index
                    || sync.source_address != location.source_byte_offset
                {
                    return Err(RawDpcDecodeError::InvalidCommand {
                        location,
                        reason: "FullSync capture observation has different chunk/source identity",
                    });
                }
                RawDpcCommandKind::FullSync(sync)
            }
            _ => {
                return Err(RawDpcDecodeError::UnsupportedCommand {
                    location,
                    decoded_opcode: opcode,
                    width,
                });
            }
        };
        commands.push(DecodedRawDpcCommand { location, kind });
        offset += width as usize;
    }
    Ok(())
}

fn plan_fill(
    location: RawDpcCommandLocation,
    rectangle: FillRectangle,
    layout: fn64_render_ir::PhysicalMemoryLayout,
    state: &RdpState,
    planned: &mut Vec<ResourceAccess>,
) -> Result<(), RawDpcDecodeError> {
    if state.other_mode().map(OtherMode::cycle_type) != Some(CycleType::Fill) {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged fill-cycle OtherMode",
        });
    }
    let Some(image) = state.color_image() else {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged SetColorImage",
        });
    };
    if state.fill_color().is_none() {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle requires staged SetFillColor",
        });
    }
    if image.format() != ImageFormat::Rgba
        || !matches!(image.size(), PixelSize::Bits16 | PixelSize::Bits32)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle supports only RGBA16 or RGBA32 color images",
        });
    }
    if [
        rectangle.upper_left_x,
        rectangle.upper_left_y,
        rectangle.lower_right_x,
        rectangle.lower_right_y,
    ]
    .iter()
    .any(|coordinate| coordinate & 0x3 != 0)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle coordinates must be whole pixels in the bounded slice",
        });
    }
    let x0 = u32::from(rectangle.upper_left_x) >> 2;
    let y0 = u32::from(rectangle.upper_left_y) >> 2;
    let x1 = u32::from(rectangle.lower_right_x) >> 2;
    let y1 = u32::from(rectangle.lower_right_y) >> 2;
    if x0 > x1 || y0 > y1 {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle coordinates are reversed",
        });
    }
    if x1 >= image.width() {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle exceeds the staged color-image width",
        });
    }
    let bytes_per_pixel = image
        .size()
        .bytes_per_pixel()
        .expect("RGBA16/32 are byte-addressed");
    let row_bytes =
        (x1 - x0 + 1)
            .checked_mul(bytes_per_pixel)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle row byte count overflows",
            })?;
    let planned_rows = if x0 == 0 && x1 + 1 == image.width() {
        1
    } else {
        usize::try_from(y1 - y0 + 1).expect("12-bit coordinates fit usize")
    };
    if planned
        .len()
        .checked_add(planned_rows)
        .is_none_or(|accesses| accesses > MAX_RESOURCE_ACCESSES)
    {
        return Err(RawDpcDecodeError::InvalidCommand {
            location,
            reason: "FillRectangle exceeds the bounded resource-plan access count",
        });
    }
    let rows: Box<dyn Iterator<Item = (u32, u32)>> = if planned_rows == 1 {
        Box::new(core::iter::once((y0, y1)))
    } else {
        Box::new((y0..=y1).map(|y| (y, y)))
    };
    for (first_y, last_y) in rows {
        let pixel = first_y
            .checked_mul(image.width())
            .and_then(|value| value.checked_add(x0))
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle pixel offset overflows",
            })?;
        let start = image
            .address()
            .get()
            .checked_add(pixel.checked_mul(bytes_per_pixel).ok_or(
                RawDpcDecodeError::InvalidCommand {
                    location,
                    reason: "FillRectangle byte offset overflows",
                },
            )?)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle address overflows",
            })?;
        let rows = last_y - first_y + 1;
        let bytes = row_bytes
            .checked_mul(rows)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle byte count overflows",
            })?;
        let end = start
            .checked_add(bytes)
            .ok_or(RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle range end overflows",
            })?;
        let range = layout
            .range(start, end)
            .map_err(|_| RawDpcDecodeError::InvalidCommand {
                location,
                reason: "FillRectangle writes outside installed RDRAM",
            })?;
        push_access(
            location.workload,
            planned,
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range,
            },
        )?;
    }
    Ok(())
}

fn push_access(
    workload: WorkloadIdentity,
    accesses: &mut Vec<ResourceAccess>,
    mode: AccessMode,
    purpose: AccessPurpose,
    region: ResourceRegion,
) -> Result<(), RawDpcDecodeError> {
    let id = u32::try_from(accesses.len())
        .map_err(|_| RawDpcDecodeError::ResourcePlanOverflow { workload })?;
    accesses.push(ResourceAccess::try_new(
        OperationId::new(id),
        mode,
        purpose,
        region,
    )?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        AccessMode, AccessPurpose, DecodedTicket, DmemRange, DpInterruptState, DramCommandChunk,
        DramCommandStream, FullSyncBoundary, PhysicalMemoryLayout, RawCommandStream, RdramResource,
        ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
        WorkloadAdmission, WorkloadPacket, XbusCommandChunk, XbusCommandStream,
    };

    use super::*;

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    fn word(prefix: u8, opcode: u8, payload: u32) -> u32 {
        u32::from(prefix | opcode) << 24 | payload
    }

    fn state_words(prefix: u8) -> Vec<u32> {
        vec![
            word(prefix, SET_OTHER_MODE, 3 << 20),
            0,
            word(prefix, SET_COLOR_IMAGE, 3 << 19 | 1),
            0,
            word(prefix, SET_FILL_COLOR, 0),
            0x213c_4d59,
        ]
    }

    fn fixture_words(prefix: u8) -> Vec<u32> {
        let mut words = state_words(prefix);
        words.extend([
            word(prefix, FILL_RECTANGLE, 4 << 12 | 4),
            0,
            word(prefix, FULL_SYNC, 0),
            0,
        ]);
        words
    }

    fn command_access(layout: PhysicalMemoryLayout, bytes: u32, operation: u32) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(COMMAND_START, COMMAND_START + bytes).unwrap(),
            },
        )
        .unwrap()
    }

    fn effect_access(
        layout: PhysicalMemoryLayout,
        operation: u32,
        start: u32,
        end: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(start, end).unwrap(),
            },
        )
        .unwrap()
    }

    fn packet(
        transaction_sequence: u64,
        words: Vec<u32>,
        effect_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let bytes = u32::try_from(words.len() * 4).unwrap();
        let command_range = layout.range(COMMAND_START, COMMAND_START + bytes).unwrap();
        let sync_count = words
            .chunks_exact(2)
            .filter(|command| ((command[0] >> 24) as u8 & 0x3f) == FULL_SYNC)
            .count();
        let full_syncs = (0..sync_count)
            .map(|ordinal| {
                FullSyncBoundary::new(
                    2 + ordinal as u64 * 2,
                    3 + ordinal as u64 * 2,
                    DpInterruptState::Clear,
                    DpInterruptState::Asserted,
                )
            })
            .collect();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                full_syncs,
            )
            .unwrap()])
            .unwrap(),
        );
        let mut accesses = vec![command_access(layout, bytes, 0)];
        accesses.extend(
            effect_ranges
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| effect_access(layout, index as u32 + 1, start, end)),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(64, LAYOUT_BYTES).unwrap(),
            accesses,
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }

    fn submit(packet: WorkloadPacket) -> SubmittedTicket {
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        queue.submit(DecodedTicket::new(packet)).unwrap()
    }

    fn decode(words: Vec<u32>) -> Result<DecodedRawDpc, RawDpcDecodeError> {
        let submitted = submit(packet(7, words, &[]));
        decode_raw_dpc(submitted, &RdpState::default())
    }

    #[test]
    fn every_public_width_is_used_before_subset_rejection() {
        for (opcode, width) in [
            (0x08, 32),
            (0x09, 48),
            (0x0a, 96),
            (0x0b, 112),
            (0x0c, 96),
            (0x0d, 112),
            (0x0e, 160),
            (0x0f, 176),
            (0x24, 16),
            (0x25, 16),
            (0x26, 8),
            (0x3e, 8),
        ] {
            let mut words = vec![0; width / 4];
            words[0] = word(0x80, opcode, 0);
            let error = decode(words).unwrap_err();
            assert!(matches!(
                error,
                RawDpcDecodeError::UnsupportedCommand {
                    location,
                    decoded_opcode,
                    width: actual,
                } if location.wire_opcode() == 0x80 | opcode
                    && decoded_opcode == opcode
                    && actual == width as u32
            ));
        }
    }

    #[test]
    fn truncation_table_reports_exact_context_for_every_width_class() {
        let submitted = submit(packet(7, vec![0, 0], &[]));
        let packet = submitted.packet();
        let mut stream = FlattenedStream::new(packet.identity(), 0, &packet.streams()[0]);
        for (opcode, width) in [(0x00, 8), (0x24, 16), (0x08, 32), (0x0f, 176)] {
            stream.bytes = vec![0; width - 1];
            stream.bytes[0] = 0xc0 | opcode;
            let mut state = RdpState::default();
            let mut delta = RdpStateDelta::default();
            let mut planned = Vec::new();
            let mut commands = Vec::new();
            let error = decode_stream(
                &stream,
                packet.memory_layout(),
                &mut state,
                &mut delta,
                &mut planned,
                &mut commands,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                RawDpcDecodeError::TruncatedCommand {
                    location,
                    width: actual_width,
                    available,
                } if location.workload() == packet.identity()
                    && location.stream() == packet.streams()[0].identity()
                    && location.chunk_index() == 0
                    && location.source_byte_offset() == COMMAND_START
                    && location.wire_opcode() == 0xc0 | opcode
                    && actual_width == width as u32
                    && available == (width - 1) as u32
            ));
        }
    }

    #[test]
    fn unknown_width_is_loud_with_complete_location() {
        let submitted = submit(packet(7, vec![0, 0], &[]));
        let packet = submitted.packet();
        let mut stream = FlattenedStream::new(packet.identity(), 0, &packet.streams()[0]);
        stream.bytes[0] = 0x50;
        let error = decode_stream(
            &stream,
            packet.memory_layout(),
            &mut RdpState::default(),
            &mut RdpStateDelta::default(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_err();
        let RawDpcDecodeError::UnknownCommandWidth { location } = error else {
            panic!("expected unknown command width");
        };
        assert_eq!(location.workload(), packet.identity());
        assert_eq!(location.stream(), packet.streams()[0].identity());
        assert_eq!(location.chunk_index(), 0);
        assert_eq!(location.source_byte_offset(), COMMAND_START);
        assert_eq!(location.wire_opcode(), 0x50);
    }

    #[test]
    fn unsupported_offset_table_is_byte_exact() {
        for noops in [0usize, 1, 3, 7] {
            let mut words = vec![0; noops * 2];
            words.extend([word(0x40, 0x24, 0), 0, 0, 0]);
            let submitted = submit(packet(7, words, &[]));
            let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
            let rendered = error.to_string();
            let RawDpcDecodeError::UnsupportedCommand { location, .. } = error else {
                panic!("expected unsupported command");
            };
            let offset = u32::try_from(noops * 8).unwrap();
            assert_eq!(location.stream_byte_offset(), offset);
            assert_eq!(location.source_byte_offset(), COMMAND_START + offset);
            assert_eq!(location.wire_opcode(), 0x64);
            assert!(rendered.contains("chunk 0 source byte offset"));
        }
    }

    #[test]
    fn source_offset_selects_the_exact_chunk() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let first_range = layout.range(COMMAND_START, COMMAND_START + 8).unwrap();
        let second_range = layout.range(COMMAND_START + 8, COMMAND_START + 40).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![
                DramCommandChunk::try_new(
                    first_range,
                    vec![0, 0],
                    TemporalBoundary::new(1, DpInterruptState::Clear),
                    Vec::new(),
                )
                .unwrap(),
                DramCommandChunk::try_new(
                    second_range,
                    {
                        let mut words = vec![0; 8];
                        words[0] = word(0x80, 0x08, 0);
                        words
                    },
                    TemporalBoundary::new(2, DpInterruptState::Clear),
                    Vec::new(),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(4, 64).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: layout.range(COMMAND_START, COMMAND_START + 40).unwrap(),
                },
            )
            .unwrap()],
        )
        .unwrap();
        let submitted = submit(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap(),
        );
        let RawDpcDecodeError::UnsupportedCommand { location, .. } =
            decode_raw_dpc(submitted, &RdpState::default()).unwrap_err()
        else {
            panic!("expected unsupported command");
        };
        assert_eq!(location.chunk_index(), 1);
        assert_eq!(location.stream_byte_offset(), 8);
        assert_eq!(location.source_byte_offset(), COMMAND_START + 8);
        assert_eq!(location.wire_opcode(), 0x88);
    }

    #[test]
    fn xbus_noop_has_an_exact_rsp_dmem_command_plan() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let range = DmemRange::try_new(0x100, 0x108).unwrap();
        let stream = RawCommandStream::Xbus(
            XbusCommandStream::try_new(vec![XbusCommandChunk::try_new(
                range,
                vec![0xc0, 0, 0, 0, 0, 0, 0, 0],
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap()])
            .unwrap(),
        );
        let access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::RspDmem(range),
        )
        .unwrap();
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(1, 8).unwrap(), vec![access])
                .unwrap();
        let submitted = submit(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap(),
        );
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.resource_plan().accesses(), [access]);
        assert_eq!(decoded.commands()[0].location().source_byte_offset(), 0x100);
    }

    #[test]
    fn all_low_noop_variants_and_four_prefixes_are_admitted() {
        let mut words = Vec::new();
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            for variant in 0..=7 {
                words.extend([word(prefix, variant, 0x005a_5a5a), 0xa5a5_a5a5]);
            }
        }
        let submitted = submit(packet(7, words, &[]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(decoded.commands().len(), 32);
        for (index, command) in decoded.commands().iter().enumerate() {
            assert_eq!(
                command.kind(),
                RawDpcCommandKind::NoOp {
                    variant: (index % 8) as u8
                }
            );
        }
        assert_eq!(decoded.state_delta(), &RdpStateDelta::default());
    }

    #[test]
    fn every_admitted_state_command_accepts_all_four_wire_prefixes() {
        for prefix in [0x00, 0x40, 0x80, 0xc0] {
            let submitted = submit(packet(7, fixture_words(prefix), &[(0, 16)]));
            let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
            assert_eq!(decoded.commands().len(), 5);
            assert_eq!(
                decoded.staged_state().fill_color().unwrap().rgba32(),
                [0x21, 0x3c, 0x4d, 0x59]
            );
            for command in decoded.commands() {
                assert_eq!(command.location().wire_opcode() & 0xc0, prefix);
            }
        }
    }

    #[test]
    fn state_order_table_rejects_each_missing_precondition() {
        let cases = [
            ("OtherMode", vec![word(0, FILL_RECTANGLE, 0), 0]),
            (
                "SetColorImage",
                vec![
                    word(0, SET_OTHER_MODE, 3 << 20),
                    0,
                    word(0, FILL_RECTANGLE, 0),
                    0,
                ],
            ),
            (
                "SetFillColor",
                vec![
                    word(0, SET_OTHER_MODE, 3 << 20),
                    0,
                    word(0, SET_COLOR_IMAGE, 3 << 19 | 1),
                    0,
                    word(0, FILL_RECTANGLE, 0),
                    0,
                ],
            ),
        ];
        for (expected, words) in cases {
            let error = decode(words).unwrap_err();
            let RawDpcDecodeError::InvalidCommand { location, reason } = error else {
                panic!("expected state-invalid command");
            };
            assert!(reason.contains(expected));
            assert_eq!(location.wire_opcode() & 0x3f, FILL_RECTANGLE);
        }
    }

    #[test]
    fn hostile_state_mutation_table_is_loud_and_located() {
        let mut cases = vec![
            ("non-fill cycle", vec![word(0, SET_OTHER_MODE, 2 << 20), 0]),
            (
                "reserved image format",
                vec![word(0, SET_COLOR_IMAGE, 7 << 21 | 3 << 19 | 1), 0],
            ),
            (
                "unaligned image",
                vec![word(0, SET_COLOR_IMAGE, 3 << 19 | 1), 4],
            ),
            (
                "image outside RDRAM",
                vec![word(0, SET_COLOR_IMAGE, 3 << 19 | 1), LAYOUT_BYTES],
            ),
        ];
        let mut unsupported_image = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 1),
            0,
            word(0, SET_FILL_COLOR, 0),
            0,
        ];
        unsupported_image.extend([word(0, FILL_RECTANGLE, 0), 0]);
        cases.push(("unsupported image size", unsupported_image));
        let mut fractional = state_words(0);
        fractional.extend([word(0, FILL_RECTANGLE, 1), 0]);
        cases.push(("fractional rectangle", fractional));
        let mut reversed = state_words(0);
        reversed.extend([word(0, FILL_RECTANGLE, 0), 4 << 12]);
        cases.push(("reversed rectangle", reversed));
        let mut too_wide = state_words(0);
        too_wide.extend([word(0, FILL_RECTANGLE, 8 << 12), 0]);
        cases.push(("wide rectangle", too_wide));
        for (name, words) in cases {
            let submitted = submit(packet(7, words, &[]));
            let workload = submitted.packet().identity();
            let stream = submitted.packet().streams()[0].identity();
            let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
            let RawDpcDecodeError::InvalidCommand { location, .. } = &error else {
                panic!("{name}: expected state-invalid command, got {error:?}");
            };
            assert_eq!(location.workload(), workload, "{name}");
            assert_eq!(location.stream(), stream, "{name}");
            let rendered = error.to_string();
            assert!(
                rendered.contains("source byte offset"),
                "{name}: {rendered}"
            );
            assert!(rendered.contains("wire opcode"), "{name}: {rendered}");
        }
    }

    #[test]
    fn full_sync_preserves_unassigned_nonzero_payload_bits() {
        let words = vec![word(0x80, FULL_SYNC, 0x005a_a55a), 0xa5a5_5a5a];
        let decoded = decode_raw_dpc(submit(packet(7, words, &[])), &RdpState::default()).unwrap();
        assert!(matches!(
            decoded.commands()[0].kind(),
            RawDpcCommandKind::FullSync(_)
        ));
    }

    #[test]
    fn resource_plan_growth_is_bounded_at_the_command_location() {
        let mut words = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 2),
            0,
            word(0, SET_FILL_COLOR, 0),
            0,
        ];
        for _ in 0..17 {
            words.extend([word(0, FILL_RECTANGLE, 4 << 12 | (1023 * 4)), 0]);
        }
        let submitted = submit(packet(7, words, &[]));
        let error = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err();
        assert!(matches!(
            error,
            RawDpcDecodeError::InvalidCommand { location, reason }
                if location.wire_opcode() & 0x3f == FILL_RECTANGLE
                    && reason.contains("bounded resource-plan")
        ));
    }

    #[test]
    fn exact_journal_equality_includes_order_identity_and_cardinality() {
        let valid = packet(7, fixture_words(0), &[(0, 16)]);
        let expected = valid.journal().accesses().to_vec();
        for accesses in [
            vec![expected[0]],
            vec![
                ResourceAccess::try_new(
                    OperationId::new(0),
                    expected[1].mode(),
                    expected[1].purpose(),
                    expected[1].region(),
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(1),
                    expected[0].mode(),
                    expected[0].purpose(),
                    expected[0].region(),
                )
                .unwrap(),
            ],
            vec![
                expected[0],
                ResourceAccess::try_new(
                    OperationId::new(9),
                    expected[1].mode(),
                    expected[1].purpose(),
                    expected[1].region(),
                )
                .unwrap(),
            ],
            vec![
                expected[0],
                expected[1],
                ResourceAccess::try_new(
                    OperationId::new(2),
                    AccessMode::Read,
                    AccessPurpose::UploadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: valid.memory_layout().range(0x200, 0x208).unwrap(),
                    },
                )
                .unwrap(),
            ],
        ] {
            let journal = ResourceJournal::try_new(
                ResourceJournalLimits::try_new(64, LAYOUT_BYTES).unwrap(),
                accesses.clone(),
            )
            .unwrap();
            let altered = WorkloadPacket::try_new(
                valid.memory_layout(),
                valid.admission(),
                valid.streams().to_vec(),
                journal,
            )
            .unwrap();
            let submitted = submit(altered);
            let RawDpcDecodeError::JournalMismatch {
                expected: planned,
                actual,
                ..
            } = decode_raw_dpc(submitted, &RdpState::default()).unwrap_err()
            else {
                panic!("expected exact journal mismatch");
            };
            assert_eq!(&*planned, expected);
            assert_eq!(&*actual, accesses);
        }
    }

    #[test]
    fn non_contiguous_rectangle_rows_have_one_exact_access_each() {
        let mut words = vec![
            word(0, SET_OTHER_MODE, 3 << 20),
            0,
            word(0, SET_COLOR_IMAGE, 3 << 19 | 3),
            0,
            word(0, SET_FILL_COLOR, 0),
            0x0102_0304,
        ];
        words.extend([word(0, FILL_RECTANGLE, 8 << 12 | 4), 4 << 12]);
        let submitted = submit(packet(7, words, &[(4, 12), (20, 28)]));
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        assert_eq!(
            decoded.resource_plan().accesses(),
            decoded.submitted().packet().journal().accesses()
        );
    }

    #[test]
    fn two_packet_chaining_is_explicit_move_only_and_does_not_mutate_baseline() {
        let durable = RdpState::default();
        let first_packet = packet(7, state_words(0), &[]);
        let second_packet = packet(
            8,
            vec![
                word(0, FILL_RECTANGLE, 4 << 12 | 4),
                0,
                word(0, FULL_SYNC, 0),
                0,
            ],
            &[(0, 16)],
        );
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue.submit(DecodedTicket::new(first_packet)).unwrap();
        let second = queue.submit(DecodedTicket::new(second_packet)).unwrap();
        let first = decode_raw_dpc(first, &durable).unwrap();
        assert_eq!(durable, RdpState::default());
        assert_eq!(first.staged_state().transaction_sequence(), 7);
        let second = decode_raw_dpc_after(second, first.into_staged_state()).unwrap();
        assert_eq!(second.state_delta(), &RdpStateDelta::default());
        assert_eq!(
            second.staged_state().fill_color().unwrap().value(),
            0x213c_4d59
        );
        assert_eq!(durable, RdpState::default());
    }

    #[test]
    fn stale_or_cross_queue_staged_state_is_rejected_before_decode() {
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let stale = queue
            .submit(DecodedTicket::new(packet(7, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(stale, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let gap = queue
            .submit(DecodedTicket::new(packet(9, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(gap, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let maximum = queue
            .submit(DecodedTicket::new(packet(u64::MAX, state_words(0), &[])))
            .unwrap();
        let wrapped = queue
            .submit(DecodedTicket::new(packet(0, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(maximum, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(wrapped, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("transaction sequence")
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let _intervening = queue
            .submit(DecodedTicket::new(packet(8, vec![0, 0], &[])))
            .unwrap();
        let skipped = queue
            .submit(DecodedTicket::new(packet(9, vec![0, 0], &[])))
            .unwrap();
        let staged = decode_raw_dpc(first, &RdpState::default())
            .unwrap()
            .into_staged_state();
        assert!(matches!(
            decode_raw_dpc_after(skipped, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("semantic submission")
        ));

        let (mut first_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let source = first_queue
            .submit(DecodedTicket::new(packet(7, state_words(0), &[])))
            .unwrap();
        let staged = decode_raw_dpc(source, &RdpState::default())
            .unwrap()
            .into_staged_state();
        let (mut other_queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let other = other_queue
            .submit(DecodedTicket::new(packet(8, vec![0, 0], &[])))
            .unwrap();
        assert!(matches!(
            decode_raw_dpc_after(other, staged),
            Err(RawDpcDecodeError::StagedStateMismatch { reason, .. })
                if reason.contains("queue")
        ));
    }
}
