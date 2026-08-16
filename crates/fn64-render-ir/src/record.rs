use core::num::NonZeroU32;

use sha2::{Digest, Sha256};

use crate::{
    AccessMode, AccessPurpose, CmdEndOccurrence, ContentDigest, DmemRange, DpInterruptState,
    FullSyncOccurrence, HostResource, JournalIdentity, MicrocodeAdmissionIdentity, OperationId,
    PhysicalMemoryLayout, RawCommandStream, RawStreamIdentity, RawStreamKind, RdramResource,
    RecordIdentity, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    TmemRange, ValidationError, WorkloadAdmission, WorkloadIdentity, WorkloadPacket,
};

pub const WORKLOAD_RECORD_SCHEMA: &str = "fn64.render-ir.record.v2";
pub const MAX_WORKLOAD_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"F64RIR02";
const VERSION: u16 = 2;
const INTEGRITY_BYTES: usize = 32;
const CMD_END_RECORD_BYTES: usize = 4 + 8 + 4 + 1;
const FULL_SYNC_RECORD_BYTES: usize = 5 * 4 + 2 * 8 + 2;
const MIN_RAW_STREAM_RECORD_BYTES: usize = 1 + 32 + 3 * 4 + 4 + CMD_END_RECORD_BYTES + 4;

/// Content-silent metadata for one raw stream. Payload bytes are represented
/// solely by `identity` and must be supplied separately to replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawStreamRecord {
    kind: RawStreamKind,
    identity: RawStreamIdentity,
    start: u32,
    end: u32,
    byte_len: u32,
    cmd_ends: Box<[CmdEndOccurrence]>,
    full_syncs: Box<[FullSyncOccurrence]>,
}

impl RawStreamRecord {
    fn from_stream(stream: &RawCommandStream) -> Self {
        let (start, end) = stream.source_bounds();
        Self {
            kind: stream.kind(),
            identity: stream.identity(),
            start,
            end,
            byte_len: stream.byte_len(),
            cmd_ends: stream.cmd_end_occurrences().into(),
            full_syncs: stream.full_sync_occurrences().into(),
        }
    }

    pub const fn kind(&self) -> RawStreamKind {
        self.kind
    }

    pub const fn identity(&self) -> RawStreamIdentity {
        self.identity
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn end(&self) -> u32 {
        self.end
    }

    pub const fn byte_len(&self) -> u32 {
        self.byte_len
    }

    pub fn cmd_end_occurrences(&self) -> &[CmdEndOccurrence] {
        &self.cmd_ends
    }

    pub fn full_sync_occurrences(&self) -> &[FullSyncOccurrence] {
        &self.full_syncs
    }

    fn matches(&self, stream: &RawCommandStream) -> bool {
        self.kind == stream.kind()
            && self.identity == stream.identity()
            && (self.start, self.end) == stream.source_bounds()
            && self.byte_len == stream.byte_len()
            && self.cmd_ends.as_ref() == stream.cmd_end_occurrences()
            && self.full_syncs.as_ref() == stream.full_sync_occurrences()
    }

    fn validate(&self, memory_layout: PhysicalMemoryLayout) -> Result<(), ValidationError> {
        if self.start >= self.end || self.end - self.start != self.byte_len {
            return Err(invalid(
                "raw stream bounds",
                format!(
                    "[{:#x}, {:#x}) does not describe {} bytes",
                    self.start, self.end, self.byte_len
                ),
            ));
        }
        match self.kind {
            RawStreamKind::Dram => {
                memory_layout
                    .range(self.start, self.end)?
                    .require_alignment(8)?;
            }
            RawStreamKind::Xbus => {
                DmemRange::try_new(self.start, self.end)?.require_alignment(8)?;
            }
        }
        if self.cmd_ends.is_empty() {
            return Err(invalid("raw stream CMD_END list", "list is empty"));
        }
        for (index, boundary) in self.cmd_ends.iter().enumerate() {
            if boundary.chunk_index as usize != index {
                return Err(invalid(
                    "raw stream CMD_END index",
                    format!("entry {index} names chunk {}", boundary.chunk_index),
                ));
            }
            if boundary.source_address <= self.start || boundary.source_address > self.end {
                return Err(invalid(
                    "raw stream CMD_END address",
                    format!("{:#x} is outside recorded stream", boundary.source_address),
                ));
            }
            if let Some(prior) = index.checked_sub(1).map(|prior| self.cmd_ends[prior]) {
                if boundary.sequence <= prior.sequence
                    || boundary.source_address <= prior.source_address
                {
                    return Err(invalid(
                        "raw stream CMD_END order",
                        "sequence and source address must be strictly increasing",
                    ));
                }
            }
        }
        if self.cmd_ends.last().expect("nonempty").source_address != self.end {
            return Err(invalid(
                "raw stream final CMD_END",
                "final boundary does not equal stream end",
            ));
        }
        for (index, sync) in self.full_syncs.iter().enumerate() {
            if sync.chunk_index as usize >= self.cmd_ends.len() {
                return Err(invalid(
                    "FullSync occurrence",
                    format!("entry {index} names absent chunk {}", sync.chunk_index),
                ));
            }
            let chunk_start = if sync.chunk_index == 0 {
                self.start
            } else {
                self.cmd_ends[sync.chunk_index as usize - 1].source_address
            };
            let chunk_end = self.cmd_ends[sync.chunk_index as usize].source_address;
            if sync.ordinal as usize != index
                || sync.stream_byte_offset >= self.byte_len
                || sync.source_address != self.start + sync.stream_byte_offset
                || sync.source_address < chunk_start
                || sync.source_address + 8 > chunk_end
                || sync.chunk_byte_offset != sync.source_address - chunk_start
                || !sync.stream_byte_offset.is_multiple_of(8)
            {
                return Err(invalid(
                    "FullSync occurrence",
                    format!("entry {index} has inconsistent ordinal, offset, or chunk"),
                ));
            }
            if index > 0
                && (sync.stream_byte_offset <= self.full_syncs[index - 1].stream_byte_offset
                    || sync.chunk_index < self.full_syncs[index - 1].chunk_index)
            {
                return Err(invalid(
                    "FullSync occurrence order",
                    format!("entry {index} is not in decoded command order"),
                ));
            }
            let cmd_end = self.cmd_ends[sync.chunk_index as usize];
            let prior = self.full_syncs[..index]
                .iter()
                .rev()
                .find(|prior| prior.chunk_index == sync.chunk_index);
            let prior_sequence = prior.map_or(cmd_end.sequence, |prior| prior.interrupt_sequence);
            let prior_interrupt = prior.map_or(cmd_end.interrupt, |prior| prior.interrupt_after);
            if sync.sequence <= prior_sequence || sync.interrupt_sequence <= sync.sequence {
                return Err(invalid(
                    "FullSync temporal order",
                    format!("entry {index} is not ordered after CMD_END/prior observation"),
                ));
            }
            if sync.interrupt_before != prior_interrupt
                || matches!(
                    (sync.interrupt_before, sync.interrupt_after),
                    (DpInterruptState::Asserted, DpInterruptState::Clear)
                )
            {
                return Err(invalid(
                    "FullSync interrupt observation",
                    format!("entry {index} has a discontinuous or clearing transition"),
                ));
            }
        }
        for chunk_index in 1..self.cmd_ends.len() {
            let prior_boundary = self.cmd_ends[chunk_index - 1];
            let prior_last = self
                .full_syncs
                .iter()
                .rev()
                .find(|sync| sync.chunk_index as usize == chunk_index - 1)
                .map_or(prior_boundary.sequence, |sync| sync.interrupt_sequence);
            if self.cmd_ends[chunk_index].sequence <= prior_last {
                return Err(invalid(
                    "raw stream packet-global temporal order",
                    format!("chunk {chunk_index} CMD_END is not after every prior chunk event"),
                ));
            }
        }
        Ok(())
    }
}

/// Deterministic, content-silent workload evidence and replay recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkloadRecord {
    workload: WorkloadIdentity,
    memory_layout: PhysicalMemoryLayout,
    admission: WorkloadAdmission,
    streams: Box<[RawStreamRecord]>,
    journal: ResourceJournal,
}

impl WorkloadRecord {
    pub fn from_packet(packet: &WorkloadPacket) -> Self {
        Self {
            workload: packet.identity(),
            memory_layout: packet.memory_layout(),
            admission: packet.admission(),
            streams: packet
                .streams()
                .iter()
                .map(RawStreamRecord::from_stream)
                .collect(),
            journal: packet.journal().clone(),
        }
    }

    pub const fn workload_identity(&self) -> WorkloadIdentity {
        self.workload
    }

    pub const fn memory_layout(&self) -> PhysicalMemoryLayout {
        self.memory_layout
    }

    pub const fn admission(&self) -> WorkloadAdmission {
        self.admission
    }

    pub fn streams(&self) -> &[RawStreamRecord] {
        &self.streams
    }

    pub const fn journal(&self) -> &ResourceJournal {
        &self.journal
    }

    /// Encode no command bytes: only source geometry, temporal boundaries,
    /// semantic occurrences, resource declarations, and content identities.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(MAGIC);
        put_u16(&mut body, VERSION);
        body.extend_from_slice(&self.workload.as_bytes());
        put_u32(&mut body, self.memory_layout.bytes());
        encode_admission(&mut body, self.admission);
        put_u32(&mut body, self.streams.len() as u32);
        for stream in &self.streams {
            body.push(stream.kind.tag());
            body.extend_from_slice(&stream.identity.as_bytes());
            put_u32(&mut body, stream.start);
            put_u32(&mut body, stream.end);
            put_u32(&mut body, stream.byte_len);
            put_u32(&mut body, stream.cmd_ends.len() as u32);
            for boundary in &stream.cmd_ends {
                put_u32(&mut body, boundary.chunk_index);
                put_u64(&mut body, boundary.sequence);
                put_u32(&mut body, boundary.source_address);
                body.push(boundary.interrupt.tag());
            }
            put_u32(&mut body, stream.full_syncs.len() as u32);
            for sync in &stream.full_syncs {
                put_u32(&mut body, sync.ordinal);
                put_u32(&mut body, sync.stream_byte_offset);
                put_u32(&mut body, sync.source_address);
                put_u32(&mut body, sync.chunk_index);
                put_u32(&mut body, sync.chunk_byte_offset);
                put_u64(&mut body, sync.sequence);
                put_u64(&mut body, sync.interrupt_sequence);
                body.push(sync.interrupt_before.tag());
                body.push(sync.interrupt_after.tag());
            }
        }
        encode_journal(&mut body, &self.journal);
        let integrity = record_digest(&body);
        body.extend_from_slice(&integrity.as_bytes());
        body
    }

    pub fn decode(encoded: &[u8]) -> Result<Self, ValidationError> {
        if encoded.len() > MAX_WORKLOAD_RECORD_BYTES {
            return Err(ValidationError::RecordTooLarge {
                actual: encoded.len(),
                maximum: MAX_WORKLOAD_RECORD_BYTES,
            });
        }
        if encoded.len() < MAGIC.len() + 2 + INTEGRITY_BYTES {
            return Err(ValidationError::RecordTruncated { field: "header" });
        }
        let (body, encoded_integrity) = encoded.split_at(encoded.len() - INTEGRITY_BYTES);
        let expected_integrity = record_digest(body).as_bytes();
        if encoded_integrity != expected_integrity {
            return Err(ValidationError::RecordIntegrityMismatch);
        }

        let mut reader = Reader::new(body);
        if reader.take(MAGIC.len(), "magic")? != MAGIC {
            return Err(ValidationError::RecordMagic);
        }
        let version = reader.u16("version")?;
        if version != VERSION {
            return Err(ValidationError::RecordVersion { actual: version });
        }
        let workload = WorkloadIdentity::new(ContentDigest::from_bytes(
            reader.array("workload identity")?,
        ));
        let memory_layout = PhysicalMemoryLayout::try_new(reader.u32("physical memory layout")?)?;
        let admission = decode_admission(&mut reader)?;
        let stream_count = reader.count_sized(
            "stream count",
            crate::MAX_PACKET_STREAMS,
            MIN_RAW_STREAM_RECORD_BYTES,
        )?;
        if stream_count == 0 {
            return Err(ValidationError::EmptyWorkload);
        }
        let mut streams = Vec::with_capacity(stream_count);
        let mut aggregate_bytes = 0_usize;
        let mut aggregate_chunks = 0_usize;
        let mut aggregate_events = 0_usize;
        let mut prior_stream_last_sequence = None;
        for _ in 0..stream_count {
            let kind = RawStreamKind::from_tag(reader.u8("raw stream kind")?)?;
            let identity = RawStreamIdentity::new(ContentDigest::from_bytes(
                reader.array("raw stream identity")?,
            ));
            let start = reader.u32("raw stream start")?;
            let end = reader.u32("raw stream end")?;
            let byte_len = reader.u32("raw stream length")?;
            validate_stream_geometry(kind, start, end, byte_len, memory_layout)?;
            aggregate_bytes = checked_record_total(
                "aggregate command bytes",
                aggregate_bytes,
                byte_len as usize,
                crate::MAX_PACKET_COMMAND_BYTES,
            )?;
            let geometry_event_maximum = byte_len as usize / 8;
            let boundary_count = reader.count_sized(
                "CMD_END count",
                crate::MAX_COMMAND_CHUNKS.min(geometry_event_maximum),
                CMD_END_RECORD_BYTES,
            )?;
            aggregate_chunks = checked_record_total(
                "aggregate command chunks",
                aggregate_chunks,
                boundary_count,
                crate::MAX_PACKET_COMMAND_CHUNKS,
            )?;
            let mut cmd_ends = Vec::with_capacity(boundary_count);
            for _ in 0..boundary_count {
                cmd_ends.push(CmdEndOccurrence {
                    chunk_index: reader.u32("CMD_END chunk index")?,
                    sequence: reader.u64("CMD_END sequence")?,
                    source_address: reader.u32("CMD_END source address")?,
                    interrupt: DpInterruptState::from_tag(reader.u8("DP interrupt state")?)?,
                });
            }
            let sync_count = reader.count_sized(
                "FullSync count",
                geometry_event_maximum,
                FULL_SYNC_RECORD_BYTES,
            )?;
            aggregate_events = checked_record_total(
                "aggregate timeline events",
                aggregate_events,
                boundary_count.saturating_add(sync_count.saturating_mul(2)),
                crate::MAX_PACKET_TIMELINE_EVENTS,
            )?;
            let mut full_syncs = Vec::with_capacity(sync_count);
            for _ in 0..sync_count {
                full_syncs.push(FullSyncOccurrence {
                    ordinal: reader.u32("FullSync ordinal")?,
                    stream_byte_offset: reader.u32("FullSync stream offset")?,
                    source_address: reader.u32("FullSync source address")?,
                    chunk_index: reader.u32("FullSync chunk index")?,
                    chunk_byte_offset: reader.u32("FullSync chunk offset")?,
                    sequence: reader.u64("FullSync sequence")?,
                    interrupt_sequence: reader.u64("FullSync interrupt sequence")?,
                    interrupt_before: DpInterruptState::from_tag(
                        reader.u8("FullSync interrupt-before state")?,
                    )?,
                    interrupt_after: DpInterruptState::from_tag(
                        reader.u8("FullSync interrupt-after state")?,
                    )?,
                });
            }
            let stream = RawStreamRecord {
                kind,
                identity,
                start,
                end,
                byte_len,
                cmd_ends: cmd_ends.into_boxed_slice(),
                full_syncs: full_syncs.into_boxed_slice(),
            };
            stream.validate(memory_layout)?;
            let stream_first_sequence = stream.cmd_ends[0].sequence;
            let stream_last_sequence = stream.full_syncs.last().map_or_else(
                || stream.cmd_ends.last().expect("nonempty").sequence,
                |sync| {
                    sync.interrupt_sequence
                        .max(stream.cmd_ends.last().expect("nonempty").sequence)
                },
            );
            if let Some(prior) = prior_stream_last_sequence {
                if stream_first_sequence <= prior {
                    return Err(ValidationError::NonMonotonicPacketEventSequence {
                        prior,
                        next: stream_first_sequence,
                    });
                }
            }
            prior_stream_last_sequence = Some(stream_last_sequence);
            streams.push(stream);
        }
        let journal = decode_journal(&mut reader, memory_layout)?;
        if reader.remaining() != 0 {
            return Err(ValidationError::RecordTrailingBytes {
                bytes: reader.remaining(),
            });
        }
        Ok(Self {
            workload,
            memory_layout,
            admission,
            streams: streams.into_boxed_slice(),
            journal,
        })
    }

    /// Rebuild the immutable packet using caller-owned payload streams. This
    /// is the only replay path: a content-silent record cannot manufacture
    /// private command bytes.
    pub fn replay(
        &self,
        streams: Vec<RawCommandStream>,
    ) -> Result<WorkloadPacket, ValidationError> {
        if streams.len() != self.streams.len() {
            return Err(ValidationError::ReplayStreamCount {
                expected: self.streams.len(),
                actual: streams.len(),
            });
        }
        for (index, (record, stream)) in self.streams.iter().zip(&streams).enumerate() {
            if !record.matches(stream) {
                return Err(ValidationError::ReplayStreamMismatch { index });
            }
        }
        let packet = WorkloadPacket::try_new(
            self.memory_layout,
            self.admission,
            streams,
            self.journal.clone(),
        )?;
        if packet.identity() != self.workload {
            return Err(ValidationError::RecordIdentityMismatch {
                expected: self.workload,
                actual: packet.identity(),
            });
        }
        Ok(packet)
    }

    pub fn record_identity(&self) -> RecordIdentity {
        let encoded = self.encode();
        RecordIdentity::new(record_digest(&encoded[..encoded.len() - INTEGRITY_BYTES]))
    }
}

fn encode_admission(output: &mut Vec<u8>, admission: WorkloadAdmission) {
    output.push(admission.tag());
    match admission {
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        } => put_u64(output, transaction_sequence),
        WorkloadAdmission::GraphicsTask(identity) => {
            put_u64(output, identity.generation());
            output.extend_from_slice(identity.text_sha256().as_ref());
            put_u32(output, identity.data_bytes());
            output.extend_from_slice(identity.data_sha256().as_ref());
        }
    }
}

fn decode_admission(reader: &mut Reader<'_>) -> Result<WorkloadAdmission, ValidationError> {
    match reader.u8("admission kind")? {
        1 => Ok(WorkloadAdmission::RawDpc {
            transaction_sequence: reader.u64("raw transaction sequence")?,
        }),
        2 => Ok(WorkloadAdmission::GraphicsTask(
            MicrocodeAdmissionIdentity::new(
                reader.u64("microcode generation")?,
                ContentDigest::from_bytes(reader.array("microcode text digest")?),
                reader.u32("microcode data length")?,
                ContentDigest::from_bytes(reader.array("microcode data digest")?),
            ),
        )),
        tag => Err(ValidationError::RecordInvalidTag {
            field: "admission kind",
            tag,
        }),
    }
}

fn encode_journal(output: &mut Vec<u8>, journal: &ResourceJournal) {
    put_u32(output, journal.limits().max_accesses() as u32);
    put_u32(output, journal.limits().max_declared_bytes());
    output.extend_from_slice(&journal.identity().as_bytes());
    output.extend_from_slice(&journal.guest_write_identity().as_bytes());
    put_u64(output, journal.declared_bytes());
    put_u32(output, journal.accesses().len() as u32);
    for access in journal.accesses() {
        put_u32(output, access.operation().get());
        output.push(access.mode().tag());
        output.push(access.purpose().tag());
        encode_region(output, access.region());
    }
}

fn decode_journal(
    reader: &mut Reader<'_>,
    memory_layout: PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    let max_accesses = reader.u32("journal max accesses")? as usize;
    let max_declared_bytes = reader.u32("journal max declared bytes")?;
    let recorded_identity =
        JournalIdentity::new(ContentDigest::from_bytes(reader.array("journal identity")?));
    let recorded_guest_identity = JournalIdentity::new(ContentDigest::from_bytes(
        reader.array("guest write identity")?,
    ));
    let recorded_declared_bytes = reader.u64("journal declared bytes")?;
    let limits = ResourceJournalLimits::try_new(max_accesses, max_declared_bytes)?;
    // Every access has operation/mode/purpose plus at least a region tag and
    // two u32 range fields. Preflight that minimum before reserving capacity;
    // individual region decoding performs the exact larger check.
    let access_count = reader.count_sized(
        "resource access count",
        limits.max_accesses(),
        4 + 1 + 1 + 1 + 4 + 4,
    )?;
    let mut accesses = Vec::with_capacity(access_count);
    for _ in 0..access_count {
        accesses.push(ResourceAccess::try_new(
            OperationId::new(reader.u32("resource operation id")?),
            AccessMode::from_tag(reader.u8("resource access mode")?)?,
            AccessPurpose::from_tag(reader.u8("resource access purpose")?)?,
            decode_region(reader, memory_layout)?,
        )?);
    }
    let journal = ResourceJournal::try_new(limits, accesses)?;
    if journal.identity() != recorded_identity
        || journal.guest_write_identity() != recorded_guest_identity
        || journal.declared_bytes() != recorded_declared_bytes
    {
        return Err(invalid(
            "resource journal identities",
            "decoded declarations do not reproduce recorded identities and byte count",
        ));
    }
    Ok(journal)
}

fn encode_region(output: &mut Vec<u8>, region: ResourceRegion) {
    match region {
        ResourceRegion::Rdram { resource, range } => {
            output.extend_from_slice(&[1, resource.tag()]);
            put_u32(output, range.start().get());
            put_u32(output, range.end());
        }
        ResourceRegion::RspDmem(range) => {
            output.push(2);
            put_u32(output, range.start());
            put_u32(output, range.end());
        }
        ResourceRegion::Tmem(range) => {
            output.push(3);
            put_u32(output, range.start());
            put_u32(output, range.end());
        }
        ResourceRegion::Host(HostResource::Presentation { id, bytes }) => {
            output.extend_from_slice(&[4, 1]);
            put_u64(output, id);
            put_u32(output, bytes.get());
        }
        ResourceRegion::Host(HostResource::Capture { id, bytes }) => {
            output.extend_from_slice(&[4, 2]);
            put_u64(output, id);
            put_u32(output, bytes.get());
        }
    }
}

fn decode_region(
    reader: &mut Reader<'_>,
    memory_layout: PhysicalMemoryLayout,
) -> Result<ResourceRegion, ValidationError> {
    match reader.u8("resource region kind")? {
        1 => Ok(ResourceRegion::Rdram {
            resource: RdramResource::from_tag(reader.u8("RDRAM resource kind")?)?,
            range: memory_layout.range(
                reader.u32("RDRAM range start")?,
                reader.u32("RDRAM range end")?,
            )?,
        }),
        2 => Ok(ResourceRegion::RspDmem(DmemRange::try_new(
            reader.u32("DMEM range start")?,
            reader.u32("DMEM range end")?,
        )?)),
        3 => Ok(ResourceRegion::Tmem(TmemRange::try_new(
            reader.u32("TMEM range start")?,
            reader.u32("TMEM range end")?,
        )?)),
        4 => {
            let kind = reader.u8("host resource kind")?;
            let id = reader.u64("host resource id")?;
            let byte_count = reader.u32("host resource bytes")?;
            let bytes = NonZeroU32::new(byte_count)
                .ok_or_else(|| invalid("host resource bytes", "byte capacity is zero"))?;
            match kind {
                1 => Ok(ResourceRegion::Host(HostResource::presentation(id, bytes))),
                2 => Ok(ResourceRegion::Host(HostResource::capture(id, bytes))),
                tag => Err(ValidationError::RecordInvalidTag {
                    field: "host resource kind",
                    tag,
                }),
            }
        }
        tag => Err(ValidationError::RecordInvalidTag {
            field: "resource region kind",
            tag,
        }),
    }
}

fn record_digest(body: &[u8]) -> ContentDigest {
    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.record-integrity.v2\0");
    hash.update(body);
    ContentDigest::from_bytes(hash.finalize().into())
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ValidationError {
    ValidationError::RecordInvalidField {
        field,
        reason: reason.into(),
    }
}

fn validate_stream_geometry(
    kind: RawStreamKind,
    start: u32,
    end: u32,
    byte_len: u32,
    memory_layout: PhysicalMemoryLayout,
) -> Result<(), ValidationError> {
    if start >= end || end - start != byte_len {
        return Err(invalid(
            "raw stream bounds",
            format!("[{start:#x}, {end:#x}) does not describe {byte_len} bytes"),
        ));
    }
    match kind {
        RawStreamKind::Dram => {
            memory_layout.range(start, end)?.require_alignment(8)?;
        }
        RawStreamKind::Xbus => {
            DmemRange::try_new(start, end)?.require_alignment(8)?;
        }
    }
    Ok(())
}

fn checked_record_total(
    field: &'static str,
    current: usize,
    added: usize,
    maximum: usize,
) -> Result<usize, ValidationError> {
    let total = current
        .checked_add(added)
        .ok_or_else(|| invalid(field, "count overflowed"))?;
    if total > maximum {
        return Err(invalid(
            field,
            format!("count {total} exceeds hard bound {maximum}"),
        ));
    }
    Ok(total)
}

fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.cursor
    }

    fn take(&mut self, bytes: usize, field: &'static str) -> Result<&'a [u8], ValidationError> {
        let end = self
            .cursor
            .checked_add(bytes)
            .ok_or(ValidationError::RecordTruncated { field })?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ValidationError::RecordTruncated { field })?;
        self.cursor = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], ValidationError> {
        self.take(N, field)?
            .try_into()
            .map_err(|_| ValidationError::RecordTruncated { field })
    }

    fn u8(&mut self, field: &'static str) -> Result<u8, ValidationError> {
        Ok(self.take(1, field)?[0])
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, ValidationError> {
        Ok(u16::from_be_bytes(self.array(field)?))
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, ValidationError> {
        Ok(u32::from_be_bytes(self.array(field)?))
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, ValidationError> {
        Ok(u64::from_be_bytes(self.array(field)?))
    }

    fn count(&mut self, field: &'static str, maximum: usize) -> Result<usize, ValidationError> {
        let count = self.u32(field)? as usize;
        if count > maximum {
            return Err(invalid(
                field,
                format!("count {count} exceeds hard bound {maximum}"),
            ));
        }
        Ok(count)
    }

    fn count_sized(
        &mut self,
        field: &'static str,
        maximum: usize,
        encoded_item_bytes: usize,
    ) -> Result<usize, ValidationError> {
        let count = self.count(field, maximum)?;
        let required = count
            .checked_mul(encoded_item_bytes)
            .ok_or_else(|| invalid(field, "encoded byte requirement overflowed"))?;
        if required > self.remaining() {
            return Err(ValidationError::RecordTruncated { field });
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::tests_support::packet;

    #[test]
    fn record_round_trip_is_deterministic_content_silent_and_replayable() {
        let packet = packet(0xe9, 77);
        let record = WorkloadRecord::from_packet(&packet);
        let encoded = record.encode();
        assert_eq!(encoded, record.encode());
        assert!(!encoded
            .windows(8)
            .any(|window| window == [0xe9, 0, 0, 0, 0, 0, 0, 0]));

        let decoded = WorkloadRecord::decode(&encoded).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.record_identity(), record.record_identity());
        assert_eq!(
            record.record_identity().to_string(),
            "4e6c0e4f444e2215e526dbf50925834ff2efd01687084aa5acf63f179c5845d9"
        );
        let replayed = decoded.replay(packet.streams().to_vec()).unwrap();
        assert_eq!(replayed.identity(), packet.identity());
    }

    #[test]
    fn replay_rejects_payload_with_a_different_content_identity() {
        let original = packet(0xe9, 77);
        let other = packet(0xe6, 77);
        let record = WorkloadRecord::from_packet(&original);
        assert_eq!(
            record.replay(other.streams().to_vec()).unwrap_err(),
            ValidationError::ReplayStreamMismatch { index: 0 }
        );
    }

    #[test]
    fn record_corruption_is_loud_before_metadata_is_used() {
        let mut encoded = WorkloadRecord::from_packet(&packet(0xe9, 77)).encode();
        encoded[20] ^= 0x80;
        assert_eq!(
            WorkloadRecord::decode(&encoded).unwrap_err(),
            ValidationError::RecordIntegrityMismatch
        );
    }

    #[test]
    fn record_metadata_rejects_source_bounds_before_replay() {
        let packet = packet(0xe9, 77);
        let mut stream = RawStreamRecord::from_stream(&packet.streams()[0]);
        stream.kind = RawStreamKind::Xbus;
        stream.end = crate::RSP_DMEM_BYTES + 8;
        stream.byte_len = stream.end - stream.start;
        assert!(matches!(
            stream.validate(PhysicalMemoryLayout::try_new(0x1000).unwrap()),
            Err(ValidationError::RangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn replay_retains_the_recorded_installed_memory_bound() {
        let packet = packet(0xe9, 77);
        let record = WorkloadRecord::from_packet(&packet);
        assert_eq!(record.memory_layout().bytes(), 0x1000);
        let wider = PhysicalMemoryLayout::try_new(0x2000).unwrap();
        let stream = RawCommandStream::Dram(
            crate::DramCommandStream::try_new(vec![crate::DramCommandChunk::try_new(
                wider.range(0x100, 0x108).unwrap(),
                vec![0xe900_0000, 0],
                crate::TemporalBoundary::new(1, DpInterruptState::Clear),
                vec![crate::FullSyncBoundary::new(
                    2,
                    3,
                    DpInterruptState::Clear,
                    DpInterruptState::Asserted,
                )],
            )
            .unwrap()])
            .unwrap(),
        );
        assert_eq!(
            record.replay(vec![stream]).unwrap_err(),
            ValidationError::MemoryLayoutMismatch { expected: 0x1000 }
        );
    }

    #[test]
    fn hostile_counts_and_truncation_are_rejected_before_capacity_allocation() {
        let encoded = WorkloadRecord::from_packet(&packet(0xe9, 77)).encode();
        let sync_count_offset =
            MAGIC.len() + 2 + 32 + 4 + 1 + 8 + 4 + 1 + 32 + 3 * 4 + 4 + CMD_END_RECORD_BYTES;

        let mut hostile = encoded.clone();
        hostile[sync_count_offset..sync_count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        resign(&mut hostile);
        assert!(matches!(
            WorkloadRecord::decode(&hostile),
            Err(ValidationError::RecordInvalidField {
                field: "FullSync count",
                ..
            })
        ));

        let mut truncated_body = encoded[..sync_count_offset + 4].to_vec();
        truncated_body[sync_count_offset..sync_count_offset + 4]
            .copy_from_slice(&1_u32.to_be_bytes());
        let integrity = record_digest(&truncated_body).as_bytes();
        truncated_body.extend_from_slice(&integrity);
        assert_eq!(
            WorkloadRecord::decode(&truncated_body).unwrap_err(),
            ValidationError::RecordTruncated {
                field: "FullSync count"
            }
        );
    }

    #[test]
    fn oversized_record_is_rejected_before_integrity_hashing() {
        let oversized = vec![0_u8; MAX_WORKLOAD_RECORD_BYTES + 1];
        assert_eq!(
            WorkloadRecord::decode(&oversized).unwrap_err(),
            ValidationError::RecordTooLarge {
                actual: MAX_WORKLOAD_RECORD_BYTES + 1,
                maximum: MAX_WORKLOAD_RECORD_BYTES,
            }
        );
    }

    fn resign(encoded: &mut [u8]) {
        let body_len = encoded.len() - INTEGRITY_BYTES;
        let integrity = record_digest(&encoded[..body_len]).as_bytes();
        encoded[body_len..].copy_from_slice(&integrity);
    }
}
