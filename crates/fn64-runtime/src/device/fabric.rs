use super::*;

/// Exact AI DAC sample period selected by the television clock and
/// `AI_DACRATE`. The physical sample rate is
/// `video_clock_hz / dacrate_plus_one`; retaining the rational avoids
/// accumulating the remainder discarded by an integer-Hz compatibility API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSamplePeriod {
    video_clock_hz: u32,
    dacrate_plus_one: u32,
}

impl AiSamplePeriod {
    pub const fn new(video_clock_hz: u32, dacrate_plus_one: u32) -> Self {
        assert!(video_clock_hz != 0, "AI video clock must be nonzero");
        assert!(dacrate_plus_one != 0, "AI DAC period must be nonzero");
        assert!(
            video_clock_hz >= dacrate_plus_one,
            "AI sample rate must be at least one hertz"
        );
        Self {
            video_clock_hz,
            dacrate_plus_one,
        }
    }

    /// Compatibility representation for APIs that can carry only whole Hz.
    pub const fn floor_hz(self) -> u32 {
        self.video_clock_hz / self.dacrate_plus_one
    }

    pub const fn video_clock_hz(self) -> u32 {
        self.video_clock_hz
    }

    pub const fn dacrate_plus_one(self) -> u32 {
        self.dacrate_plus_one
    }
}

pub struct DeviceFabric<R: RomStorage, T: PiTimingModel> {
    pub(crate) now: crate::EmulatedInstant,
    pub(crate) pi_dma: PiDma<R>,
    pub(crate) pi_timing: T,
    pub(crate) pi_dram_addr: RdramAddr,
    pub(crate) pi_cart_addr: u32,
    pub(crate) pi_status: u32,
    pub(crate) mi_pending: u32,
    pub(crate) mi_mask: u32,
    pub(crate) mi_interrupt_occurrences: [Option<InterruptOccurrence>; 6],
    pub(crate) pi_domain1: PiDomainTiming,
    pub(crate) pi_domain2: PiDomainTiming,
    pub(crate) pending_pi: Option<PendingPi>,
    pub(crate) ai_dram_addr: RdramAddr,
    pub(crate) ai_control: u32,
    pub(crate) ai_dacrate: u32,
    pub(crate) ai_bitrate: u32,
    pub(crate) current_ai: Option<PendingAi>,
    pub(crate) queued_ai: Option<QueuedAi>,
    pub(crate) next_ai_dma_id: u64,
    pub(crate) dpc: DpcRegisters,
    pub(crate) pending_dpc: Option<PendingDpc>,
    /// A known-width command parked mid-arrival, awaiting a later END.
    /// Distinct from `pending_dpc`: nothing is in flight, but the DP is
    /// architecturally busy and CURRENT names the stalled command.
    pub(crate) stalled_dpc: Option<StalledDpc>,
    pub(crate) si_dram_addr: RdramAddr,
    pub(crate) si_dma_error: bool,
    pub(crate) pending_si: Option<PendingSi>,
    pub(crate) si_latency: Cycles,
    pub(crate) pif_control_latency: Cycles,
    pub(crate) pif_ram: [u8; 64],
    pub(crate) rsp_memory: RspMemory,
    pub(crate) sp_mem_addr: RspMemAddr,
    pub(crate) sp_dram_addr: RdramAddr,
    pub(crate) sp_rd_len: u32,
    pub(crate) sp_wr_len: u32,
    pub(crate) sp_status: u32,
    pub(crate) sp_pc: u32,
    pub(crate) sp_semaphore: bool,
    pub(crate) active_sp_dma: Option<PendingSpDma>,
    pub(crate) queued_sp_dma: Option<SpDmaRequest>,
    pub(crate) sp_dma_setup_cycles: Cycles,
    pub(crate) vi_registers: [u32; 14],
    pub(crate) tv_type: Option<TvType>,
    pub(crate) vi_field_interval: Option<Cycles>,
    pub(crate) vi_epoch: crate::EmulatedInstant,
    pub(crate) pending_vi: Option<u64>,
    pub(crate) pending_sp: Option<u64>,
    pub(crate) pending_dp: Option<u64>,
    pub(crate) events: BTreeMap<(crate::EmulatedInstant, u64), DeviceEvent>,
    pub(crate) next_event_sequence: u64,
    pub(crate) trace: Vec<DeviceTraceEvent>,
    pub(crate) trace_enabled: bool,
    pub(crate) trace_summary: DeviceTraceSummary,
    pub(crate) next_trace_sequence: u64,
}

impl<R: RomStorage, T: PiTimingModel> DeviceFabric<R, T> {
    pub fn new(pi_dma: PiDma<R>, pi_timing: T) -> Self {
        Self {
            now: crate::EmulatedInstant::ZERO,
            pi_dma,
            pi_timing,
            pi_dram_addr: RdramAddr::from_offset(0),
            pi_cart_addr: 0,
            pi_status: 0,
            mi_pending: 0,
            mi_mask: 0,
            mi_interrupt_occurrences: [None; 6],
            pi_domain1: PiDomainTiming::default(),
            pi_domain2: PiDomainTiming::default(),
            pending_pi: None,
            ai_dram_addr: RdramAddr::from_offset(0),
            ai_control: 0,
            ai_dacrate: 0,
            ai_bitrate: 0,
            current_ai: None,
            queued_ai: None,
            next_ai_dma_id: 1,
            dpc: DpcRegisters {
                start: 0,
                end: 0,
                current: 0,
                status: 0,
                clock: DpcCounter24::ZERO,
                busy: DpcCounter24::ZERO,
                pipe_busy: DpcCounter24::ZERO,
                tmem_busy: DpcCounter24::ZERO,
            },
            pending_dpc: None,
            stalled_dpc: None,
            si_dram_addr: RdramAddr::from_offset(0),
            si_dma_error: false,
            pending_si: None,
            si_latency: Cycles::new(1),
            // Ten aligned black-box reference runs retained 4,616 R4300
            // master cycles for the direct terminate-boot transaction. This
            // is a deterministic compatibility policy, not a silicon-timing
            // claim; it remains independent from the SI DMA policy.
            pif_control_latency: Cycles::new(4_616),
            pif_ram: [0; 64],
            rsp_memory: RspMemory::new(),
            sp_mem_addr: RspMemAddr::default(),
            sp_dram_addr: RdramAddr::from_offset(0),
            sp_rd_len: 0,
            sp_wr_len: 0,
            sp_status: SP_STATUS_HALT,
            sp_pc: 0,
            sp_semaphore: false,
            active_sp_dma: None,
            queued_sp_dma: None,
            sp_dma_setup_cycles: Cycles::new(8),
            vi_registers: {
                let mut registers = [0; 14];
                // libdragon's permissively licensed public VI register
                // definitions name 0x3ff as VI_V_INTR_DEFAULT. A separate
                // public-debugger reset observation agrees. Keeping this in
                // the register image matters: zero would request line zero,
                // not represent an unprogrammed vertical interrupt.
                registers[3] = 0x3ff;
                registers
            },
            tv_type: None,
            vi_field_interval: None,
            vi_epoch: crate::EmulatedInstant::ZERO,
            pending_vi: None,
            pending_sp: None,
            pending_dp: None,
            events: BTreeMap::new(),
            next_event_sequence: 0,
            trace: Vec::new(),
            trace_enabled: true,
            trace_summary: DeviceTraceSummary::default(),
            next_trace_sequence: 0,
        }
    }

    pub const fn now(&self) -> crate::EmulatedInstant {
        self.now
    }

    /// Mutable access to the one PI storage engine for synchronous save-chip
    /// protocols and host configuration. Timed transfers still enter through
    /// [`Self::start_pi_dma`] or [`Self::write_mmio`].
    pub fn pi_dma_mut(&mut self) -> &mut PiDma<R> {
        &mut self.pi_dma
    }

    /// Immutable access to the PI storage engine's typed observation history.
    /// Mutating save protocols continue to use [`Self::pi_dma_mut`].
    pub fn pi_dma(&self) -> &PiDma<R> {
        &self.pi_dma
    }

    pub const fn pending_pi_request(&self) -> Option<PiDmaRequest> {
        match self.pending_pi {
            Some(pending) => Some(pending.request),
            None => None,
        }
    }

    pub const fn pending_si_request(&self) -> Option<SiDmaRequest> {
        match self.pending_si {
            Some(PendingSi::Dma { request, .. }) => Some(request),
            Some(PendingSi::PifControl { .. }) | None => None,
        }
    }

    pub fn snapshot(&self) -> DeviceSnapshot {
        DeviceSnapshot {
            now: self.now,
            pi_dram_addr: self.pi_dram_addr,
            pi_cart_addr: self.pi_cart_addr,
            pi_status: self.pi_status,
            ai_status: self.ai_status(),
            ai_length: self.ai_length(),
            ai_dram_addr: self.ai_dram_addr,
            ai_control: self.ai_control,
            ai_dacrate: self.ai_dacrate,
            ai_bitrate: self.ai_bitrate,
            si_dram_addr: self.si_dram_addr,
            si_status: self.si_status(),
            vi_current: self.vi_current(),
            vi_intr: self.vi_registers[3],
            vi_v_sync: self.vi_registers[6],
            tv_type: self.tv_type,
            vi_field_interval: self.vi_field_interval,
            sp_busy: self.pending_sp.is_some(),
            sp_status: self.sp_status(),
            sp_mem_addr: self.sp_mem_addr,
            sp_dram_addr: self.sp_dram_addr,
            sp_imem_generation: self.rsp_memory.imem_generation(),
            // A parked tail is architecturally busy: hardware reads DP busy
            // mid-command, and reporting idle would let software issue a
            // conflicting START/END believing the pipe is free.
            dp_busy: self.pending_dp.is_some()
                || self.pending_dpc.is_some()
                || self.stalled_dpc.is_some(),
            dpc_start: self.dpc.start,
            dpc_end: self.dpc.end,
            dpc_current: self.dpc.current,
            dpc_status: self.dpc.status,
            dpc_clock: self.dpc.clock.get(),
            dpc_busy: self.dpc.busy.get(),
            dpc_pipe_busy: self.dpc.pipe_busy.get(),
            dpc_tmem_busy: self.dpc.tmem_busy.get(),
            pending_dpc: self.pending_dpc.map(|pending| pending.submission),
            mi_pending: self.mi_pending,
            mi_mask: self.mi_mask,
            pi_domain1: self.pi_domain1,
            pi_domain2: self.pi_domain2,
        }
    }

    pub fn evidence_snapshot(&mut self) -> DeviceEvidenceSnapshot {
        let pi_timing_policy = self.pi_timing.evidence_bytes();
        assert!(
            !pi_timing_policy.is_empty(),
            "PiTimingModel::evidence_bytes must identify every future-affecting timing policy"
        );
        let scheduled_events = self
            .events
            .iter()
            .map(|(&(at, sequence), event)| {
                let (token, kind) = match *event {
                    DeviceEvent::Pi { token } => (token, ScheduledDeviceEventKind::Pi),
                    DeviceEvent::Ai { token } => (token, ScheduledDeviceEventKind::Ai),
                    DeviceEvent::Si { token } => (token, ScheduledDeviceEventKind::Si),
                    DeviceEvent::PifControl { token } => {
                        (token, ScheduledDeviceEventKind::PifControl)
                    }
                    DeviceEvent::SpDma { token } => (token, ScheduledDeviceEventKind::SpDma),
                    DeviceEvent::Vi { token } => (token, ScheduledDeviceEventKind::Vi),
                    DeviceEvent::Sp { token } => (token, ScheduledDeviceEventKind::Sp),
                    DeviceEvent::Dp { token } => (token, ScheduledDeviceEventKind::Dp),
                };
                ScheduledDeviceEventSnapshot {
                    at,
                    sequence,
                    token,
                    kind,
                }
            })
            .collect();
        let pending_eeprom_write = self.pi_dma.pending_eeprom_write_snapshot();
        let save_bytes = self.pi_dma.save_snapshot_bytes();
        DeviceEvidenceSnapshot {
            guest: self.snapshot(),
            pi_timing_policy,
            pending_pi: self.pending_pi.map(|pending| PendingPiSnapshot {
                token: pending.token,
                request: pending.request,
            }),
            current_ai: self.current_ai.map(|pending| PendingAiSnapshot {
                id: pending.id,
                token: pending.token,
                request: pending.request,
                started_at: pending.started_at,
                deadline: pending.deadline,
            }),
            queued_ai: self.queued_ai.map(|queued| QueuedAiSnapshot {
                id: queued.id,
                request: queued.request,
            }),
            pending_dpc: self.pending_dpc.map(|pending| PendingDpcSnapshot {
                submission: pending.submission,
                rollback_start: pending.rollback.start,
                rollback_end: pending.rollback.end,
                rollback_current: pending.rollback.current,
                rollback_status: pending.rollback.status,
            }),
            pending_si: self.pending_si.map(|pending| match pending {
                PendingSi::Dma { token, request } => PendingSiSnapshot::Dma { token, request },
                PendingSi::PifControl { token, command } => {
                    PendingSiSnapshot::PifControl { token, command }
                }
            }),
            si_dma_error: self.si_dma_error,
            si_latency: self.si_latency,
            pif_control_latency: self.pif_control_latency,
            mi_interrupt_occurrences: self.mi_interrupt_occurrences,
            pif_ram: self.pif_ram,
            rsp_dmem: *self.rsp_memory.bank(RspMemoryBank::Dmem),
            rsp_imem: *self.rsp_memory.bank(RspMemoryBank::Imem),
            sp_rd_len: self.sp_rd_len,
            sp_wr_len: self.sp_wr_len,
            sp_pc: self.sp_pc,
            sp_semaphore: self.sp_semaphore,
            active_sp_dma: self.active_sp_dma.map(|pending| PendingSpDmaSnapshot {
                token: pending.token,
                request: pending.request,
            }),
            queued_sp_dma: self.queued_sp_dma,
            sp_dma_setup_cycles: self.sp_dma_setup_cycles,
            vi_registers: self.vi_registers,
            vi_epoch: self.vi_epoch,
            pending_vi_token: self.pending_vi,
            pending_sp_token: self.pending_sp,
            pending_dp_token: self.pending_dp,
            scheduled_events,
            next_event_sequence: self.next_event_sequence,
            next_ai_dma_id: self.next_ai_dma_id,
            save_bytes,
            pending_eeprom_write,
        }
    }

    pub const fn rsp_memory(&self) -> &RspMemory {
        &self.rsp_memory
    }

    /// Mutable access for the one RSP execution engine owned by the host.
    /// Device DMA and the interpreter are never advanced concurrently.
    pub fn rsp_memory_mut(&mut self) -> &mut RspMemory {
        &mut self.rsp_memory
    }

    pub const fn sp_status(&self) -> u32 {
        let mut status = self.sp_status;
        if self.active_sp_dma.is_some() {
            status |= SP_STATUS_DMA_BUSY;
        }
        if self.queued_sp_dma.is_some() {
            status |= SP_STATUS_DMA_FULL;
        }
        status
    }

    pub const fn sp_dma_busy(&self) -> bool {
        self.active_sp_dma.is_some() || self.queued_sp_dma.is_some()
    }

    pub fn set_interrupt_mask(&mut self, source: InterruptSource, enabled: bool) {
        if enabled {
            self.mi_mask |= source.bit();
        } else {
            self.mi_mask &= !source.bit();
        }
    }

    pub fn set_pi_domain_timing(&mut self, domain: PiDomain, timing: PiDomainTiming) {
        match domain {
            PiDomain::Domain1 => self.pi_domain1 = timing,
            PiDomain::Domain2 => self.pi_domain2 = timing,
        }
    }

    pub const fn pi_domain_timing(&self, domain: PiDomain) -> PiDomainTiming {
        match domain {
            PiDomain::Domain1 => self.pi_domain1,
            PiDomain::Domain2 => self.pi_domain2,
        }
    }

    pub fn interrupt_pending(&self, source: InterruptSource) -> bool {
        self.mi_pending & source.bit() != 0
    }

    pub fn raise_interrupt(&mut self, source: InterruptSource) {
        if self.mi_pending & source.bit() == 0 {
            self.mi_pending |= source.bit();
            self.mi_interrupt_occurrences[source.index()] = None;
            self.record(DeviceTraceKind::MiInterruptRaised(source));
        }
    }

    pub fn raise_interrupt_occurrence(&mut self, occurrence: InterruptOccurrence) {
        assert_eq!(
            occurrence.at, self.now,
            "interrupt occurrence instant differs from the device commit instant"
        );
        let source = occurrence.source;
        assert!(
            self.mi_pending & source.bit() == 0,
            "a second exact {source:?} occurrence arrived while its MI level remained pending"
        );
        self.mi_pending |= source.bit();
        self.mi_interrupt_occurrences[source.index()] = Some(occurrence);
        self.record(DeviceTraceKind::MiInterruptRaised(source));
    }

    pub fn clear_interrupt(&mut self, source: InterruptSource) {
        if self.mi_pending & source.bit() != 0 {
            self.mi_pending &= !source.bit();
            self.mi_interrupt_occurrences[source.index()] = None;
            self.record(DeviceTraceKind::MiInterruptCleared(source));
        }
    }

    pub fn cpu_interrupt_pending(&self) -> bool {
        self.mi_pending & self.mi_mask != 0
    }

    /// Seal one currently asserted and enabled MI source for HostKernel.
    /// Masked sources remain visible to raw polling and produce no token.
    pub fn prepare_host_interrupt_service(
        &self,
        source: InterruptSource,
    ) -> Option<PreparedHostInterruptService> {
        (self.mi_pending & self.mi_mask & source.bit() != 0).then_some(
            PreparedHostInterruptService {
                source,
                occurrence: None,
            },
        )
    }

    pub fn prepare_host_interrupt_occurrence_service(
        &self,
        occurrence: InterruptOccurrence,
    ) -> Option<PreparedHostInterruptService> {
        let source = occurrence.source;
        (self.mi_pending & self.mi_mask & source.bit() != 0
            && self.mi_interrupt_occurrences[source.index()] == Some(occurrence))
        .then_some(PreparedHostInterruptService {
            source,
            occurrence: Some(occurrence),
        })
    }

    /// Consume one prepared HostKernel service and acknowledge its MI level.
    ///
    /// Rechecking both predicates rejects a replayed token and prevents a
    /// mask change between prepare and commit from clearing a raw-polled line.
    pub fn commit_host_interrupt_service(
        &mut self,
        prepared: PreparedHostInterruptService,
    ) -> ServicedHostInterrupt {
        let source = prepared.source;
        assert!(
            self.mi_pending & source.bit() != 0,
            "HostKernel interrupt service replayed or lost pending source {source:?}"
        );
        assert!(
            self.mi_mask & source.bit() != 0,
            "HostKernel interrupt service source {source:?} became masked before commit"
        );
        if let Some(occurrence) = prepared.occurrence {
            assert_eq!(
                self.mi_interrupt_occurrences[source.index()],
                Some(occurrence),
                "HostKernel interrupt service occurrence became stale before commit"
            );
        }
        self.clear_interrupt(source);
        ServicedHostInterrupt { source }
    }

    /// Direct CPU word load from the 64-byte PIF RAM window
    /// (`0x1FC007C0..0x1FC00800`). Real hardware exposes PIF RAM to uncached
    /// CPU loads as well as SI DMA; AKI-era hand-rolled joybus code and
    /// boot-handshake polls read it directly (e.g. the terminate-boot status
    /// word at 0x1FC007FC).
    pub fn pif_ram_cpu_read_w(&self, offset: usize) -> u32 {
        let offset = offset & !3;
        u32::from_be_bytes(self.pif_ram[offset..offset + 4].try_into().unwrap())
    }

    /// Direct CPU word store into PIF RAM.
    ///
    /// A final-word terminate-boot control byte acquires the same typed SI
    /// owner as DMA and schedules its completion on the device event heap.
    /// Validation and deadline arithmetic precede byte mutation so a rejected
    /// overlap or overflow cannot leave a half-admitted command image.
    pub fn pif_ram_cpu_write_w(&mut self, offset: usize, value: u32) -> Result<(), DeviceFault> {
        let offset = offset & !3;
        if self.pending_si.is_some() {
            self.si_dma_error = true;
            return Err(DeviceFault::SiBusy);
        }
        let command = if offset == 60 {
            match value as u8 {
                0 => None,
                0x08 => Some(PifControlCommand::TerminateBoot),
                control => {
                    crate::record_unsupported_event(
                        crate::UnsupportedSubsystem::Runtime,
                        "runtime.device.direct-pif-control",
                        format!("direct CPU PIF control byte {control:#04x} is not admitted"),
                        Some(Cycles::new(self.now.get())),
                        crate::UnsupportedDisposition::ReturnedError,
                    );
                    return Err(DeviceFault::UnsupportedPifControl { control });
                }
            }
        } else {
            None
        };
        let prepared = command
            .map(|command| {
                let deadline = self
                    .now
                    .checked_add(self.pif_control_latency)
                    .ok_or(DeviceFault::DeadlineOverflow)?;
                let token = self.next_event_sequence;
                let next_sequence = token.checked_add(1).ok_or(DeviceFault::DeadlineOverflow)?;
                Ok::<_, DeviceFault>((command, deadline, token, next_sequence))
            })
            .transpose()?;
        self.pif_ram[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        if let Some((command, deadline, token, next_sequence)) = prepared {
            self.next_event_sequence = next_sequence;
            self.pending_si = Some(PendingSi::PifControl { token, command });
            self.events
                .insert((deadline, token), DeviceEvent::PifControl { token });
            self.record(DeviceTraceKind::PifControlStarted(command));
        }
        Ok(())
    }

    /// Stage one complete Controller Manager command image in the physical
    /// PIF RAM owned by this fabric. The caller must first acquire the SI
    /// engine with a typed controller request; otherwise a failed overlap
    /// could overwrite the command belonging to the live transfer.
    pub fn stage_controller_pif_command(&mut self, command: [u8; 64]) {
        assert!(
            matches!(
                self.pending_si_request(),
                Some(SiDmaRequest {
                    kind: SiDmaKind::ControllerQuery | SiDmaKind::ControllerRead,
                    ..
                })
            ),
            "controller PIF command staged without an accepted Controller Manager SI request"
        );
        self.pif_ram = command;
    }

    /// Exact physical PIF RAM image. Controller Manager getters decode only
    /// this completed device-owned transaction, never a second live sample.
    pub const fn pif_ram(&self) -> &[u8; 64] {
        &self.pif_ram
    }

    pub const fn ai_status(&self) -> u32 {
        let mut status = 0;
        if self.ai_control & 1 != 0 {
            status |= AI_STATUS_ENABLED;
        }
        if self.current_ai.is_some() {
            status |= AI_STATUS_BUSY;
        }
        if self.queued_ai.is_some() {
            status |= AI_STATUS_FULL;
        }
        status
    }

    pub const fn ai_dram_addr(&self) -> RdramAddr {
        self.ai_dram_addr
    }

    pub const fn ai_control(&self) -> u32 {
        self.ai_control
    }

    pub const fn ai_dacrate(&self) -> u32 {
        self.ai_dacrate
    }

    pub const fn ai_bitrate(&self) -> u32 {
        self.ai_bitrate
    }

    /// True sample rate selected by the latched DAC period and the IPL-owned
    /// television clock. Production may not guess NTSC when boot has not
    /// established that clock authority.
    pub fn ai_sample_period(&self) -> Result<AiSamplePeriod, DeviceFault> {
        let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
        Ok(AiSamplePeriod::new(
            tv_type.vi_clock_hz(),
            self.ai_dacrate + 1,
        ))
    }

    /// Whole-Hz compatibility view of [`Self::ai_sample_period`].
    pub fn ai_sample_rate_hz(&self) -> Result<u32, DeviceFault> {
        Ok(self.ai_sample_period()?.floor_hz())
    }

    /// Guest-visible bytes remaining in the active DMA. The device fabric is
    /// advanced at every translated checkpoint, so this interpolation is a
    /// deterministic function of guest time and never host callback jitter.
    pub fn ai_length(&self) -> u32 {
        let Some(current) = self.current_ai else {
            return 0;
        };
        if self.ai_control & 1 == 0 {
            return current.request.len;
        }
        let duration = current.deadline.get() - current.started_at.get();
        let remaining_cycles = current.deadline.get().saturating_sub(self.now.get());
        let tv_type = self
            .tv_type
            .expect("an active AI DMA has an IPL-selected television clock");
        let interval_frames = (u128::from(duration) * u128::from(tv_type.vi_clock_hz()))
            / (u128::from(CPU_CLOCK_HZ) * u128::from(self.ai_dacrate + 1));
        let interval_len = interval_frames * 4;
        let remaining =
            (interval_len * u128::from(remaining_cycles)).div_ceil(u128::from(duration));
        let remaining = remaining.div_ceil(8) * 8;
        u32::try_from(remaining).expect("AI remaining length exceeds u32")
    }

    pub fn stalled_dpc(&self) -> Option<&StalledDpc> {
        self.stalled_dpc.as_ref()
    }

    pub const fn pending_dpc_submission(&self) -> Option<DpcSubmission> {
        match self.pending_dpc {
            Some(pending) => Some(pending.submission),
            None => None,
        }
    }

    pub(crate) fn validate_dpc_range(
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    ) -> Result<(), DeviceFault> {
        let upper_bound = match source {
            DpcSubmissionSource::Rdram => 0x0100_0000,
            DpcSubmissionSource::Dmem => RSP_MEMORY_BANK_SIZE as u32,
        };
        if !start.is_multiple_of(8) || !end.is_multiple_of(8) || start >= end || end > upper_bound {
            return Err(DeviceFault::InvalidDpcRange { source, start, end });
        }
        Ok(())
    }

    pub(crate) fn begin_dpc_submission(
        &mut self,
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
        rollback: DpcRegisters,
    ) -> Result<DpcSubmission, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        Self::validate_dpc_range(source, start, end)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let submission = DpcSubmission {
            token,
            source,
            start,
            end,
        };
        self.install_dpc_submission(submission, rollback);
        Ok(submission)
    }

    fn install_dpc_submission(&mut self, submission: DpcSubmission, rollback: DpcRegisters) {
        self.dpc.status &= !DPC_STATUS_START_VALID;
        self.dpc.status |= DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY;
        self.pending_dpc = Some(PendingDpc {
            submission,
            rollback,
        });
    }

    /// Reserve exact, monotonically ordered tokens for an RSP task's DPC
    /// runs without exposing any pending transaction or mutating DPC
    /// registers. All ranges are validated and ordinal capacity is proven
    /// before `next_event_sequence` advances.
    pub fn reserve_dpc_submission_batch(
        &mut self,
        requests: &[(DpcSubmissionSource, u32, u32)],
    ) -> Result<ReservedDpcSubmissionBatch, DeviceFault> {
        let requests: Vec<_> = requests
            .iter()
            .map(|&(source, start, end)| (source, start, end, 1u64))
            .collect();
        self.reserve_dpc_submission_batch_with_temporal_spans(&requests)
    }

    /// The temporal-span form used when a capture binds additional ordered
    /// observations after a member's DPC token. `span` includes the DPC token
    /// itself, so the following member is guaranteed to sort after every
    /// boundary reserved within the preceding member's span.
    pub fn reserve_dpc_submission_batch_with_temporal_spans(
        &mut self,
        requests: &[(DpcSubmissionSource, u32, u32, u64)],
    ) -> Result<ReservedDpcSubmissionBatch, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        for &(source, start, end, span) in requests {
            Self::validate_dpc_range(source, start, end)?;
            assert!(span != 0, "a reserved DPC temporal span cannot be zero");
        }
        let first = self.next_event_sequence;
        let total_span = requests
            .iter()
            .try_fold(0u64, |total, request| total.checked_add(request.3))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let next = first
            .checked_add(total_span)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let mut cursor = first;
        let submissions = requests
            .iter()
            .map(|&(source, start, end, span)| {
                let submission = DpcSubmission {
                    token: cursor,
                    source,
                    start,
                    end,
                };
                cursor = cursor
                    .checked_add(span)
                    .expect("the temporal-span sum was preflighted below");
                submission
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        self.next_event_sequence = next;
        Ok(ReservedDpcSubmissionBatch {
            submissions,
            next: 0,
        })
    }

    /// Activate the next member of one ordered reservation through the same
    /// architectural register transition as [`Self::request_dpc_submission`].
    /// The reservation advances only after every fallible admission check.
    pub fn activate_reserved_dpc_submission(
        &mut self,
        batch: &mut ReservedDpcSubmissionBatch,
    ) -> Result<Option<DpcSubmission>, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        let submission = *batch
            .submissions
            .get(batch.next)
            .expect("reserved DPC submission batch is exhausted");
        Self::validate_dpc_range(submission.source, submission.start, submission.end)?;
        let rollback = self.dpc;
        self.dpc.start = submission.start;
        self.dpc.end = submission.end;
        self.dpc.current = submission.start;
        match submission.source {
            DpcSubmissionSource::Rdram => self.dpc.status &= !DPC_STATUS_XBUS_DMEM_DMA,
            DpcSubmissionSource::Dmem => self.dpc.status |= DPC_STATUS_XBUS_DMEM_DMA,
        }
        self.dpc.status &= !DPC_STATUS_START_VALID;
        batch.next += 1;
        if self.dpc.status & DPC_STATUS_FREEZE != 0 {
            return Ok(None);
        }
        self.install_dpc_submission(submission, rollback);
        Ok(Some(submission))
    }

    /// Begin one renderer transaction through the same state used by raw
    /// START/END MMIO. The range is not architecturally consumed until the
    /// renderer returns and the caller commits this exact token.
    pub fn request_dpc_submission(
        &mut self,
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    ) -> Result<Option<DpcSubmission>, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        Self::validate_dpc_range(source, start, end)?;
        let rollback = self.dpc;
        self.dpc.start = start;
        self.dpc.end = end;
        self.dpc.current = start;
        match source {
            DpcSubmissionSource::Rdram => self.dpc.status &= !DPC_STATUS_XBUS_DMEM_DMA,
            DpcSubmissionSource::Dmem => self.dpc.status |= DPC_STATUS_XBUS_DMEM_DMA,
        }
        self.dpc.status &= !DPC_STATUS_START_VALID;
        if self.dpc.status & DPC_STATUS_FREEZE != 0 {
            return Ok(None);
        }
        self.begin_dpc_submission(source, start, end, rollback)
            .map(Some)
    }

    /// Commit renderer acceptance. CURRENT advances only here, after the
    /// selected backend has consumed the submitted bytes.
    /// Consume an admitted token by PARKING its incomplete tail.
    ///
    /// Every fallible check precedes mutation, so a rejected park leaves the
    /// transaction exactly as it was. On success the token is consumed once,
    /// CURRENT identifies the stalled command, and the DP stays busy without
    /// a live renderer transaction.
    pub fn park_dpc_submission(
        &mut self,
        token: u64,
        command_start: u32,
        exposed_end: u32,
        bytes_required: u32,
        retained_words: Vec<u32>,
    ) -> Result<(), DeviceFault> {
        let pending = self
            .pending_dpc
            .ok_or(DeviceFault::NoPendingDpcSubmission)?;
        if pending.submission.token != token {
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: pending.submission.token,
                received_token: token,
            });
        }
        assert_eq!(
            pending.submission.end, exposed_end,
            "parked DPC END disagrees with the admitted transaction"
        );
        assert!(
            command_start >= pending.submission.start && command_start < exposed_end,
            "parked command start lies outside the admitted DPC range"
        );
        let retained_bytes = u32::try_from(retained_words.len() * size_of::<u32>())
            .expect("parked DPC tail exceeds u32");
        assert_eq!(
            command_start.checked_add(retained_bytes),
            Some(exposed_end),
            "parked words do not exactly cover command_start..exposed_end"
        );
        assert!(
            retained_bytes < bytes_required,
            "parked DPC command is not incomplete"
        );
        self.dpc.current = command_start;
        self.dpc.end = exposed_end;
        self.stalled_dpc = Some(StalledDpc {
            source: pending.submission.source,
            command_start,
            exposed_end,
            bytes_required,
            retained_words,
        });
        self.pending_dpc = None;
        Ok(())
    }

    pub fn commit_dpc_submission(&mut self, token: u64) -> Result<(), DeviceFault> {
        let pending = self
            .pending_dpc
            .ok_or(DeviceFault::NoPendingDpcSubmission)?;
        if pending.submission.token != token {
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: pending.submission.token,
                received_token: token,
            });
        }
        self.dpc.current = pending.submission.end;
        self.dpc.status &= !(DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY);
        // A completed dispatch consumed any tail it was resuming.
        self.stalled_dpc = None;
        self.pending_dpc = None;
        Ok(())
    }

    /// Roll back every register mutation made while accepting a renderer
    /// transaction. This closes the interleaving where a backend rejection
    /// could otherwise consume START_VALID or advance a range that never ran.
    pub fn cancel_dpc_submission(&mut self, token: u64) -> Result<(), DeviceFault> {
        let pending = self
            .pending_dpc
            .ok_or(DeviceFault::NoPendingDpcSubmission)?;
        if pending.submission.token != token {
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: pending.submission.token,
                received_token: token,
            });
        }
        // Reverse only the admission-owned registers. The four performance
        // counters and any counter-clear issued during admission are NOT rolled
        // back: a wholesale `self.dpc = pending.rollback` would resurrect a
        // cleared counter, and mode-command interleaving is preserved by the
        // rollback.status mirror maintained in the STATUS write handler.
        self.dpc.start = pending.rollback.start;
        self.dpc.end = pending.rollback.end;
        self.dpc.current = pending.rollback.current;
        self.dpc.status = pending.rollback.status;
        self.pending_dpc = None;
        Ok(())
    }

    pub const fn si_status(&self) -> u32 {
        let mut status = 0;
        match self.pending_si {
            Some(PendingSi::Dma { .. }) => status |= 1,
            Some(PendingSi::PifControl { .. }) => status |= 1 | 2,
            None => {}
        }
        if self.si_dma_error {
            status |= 1 << 3;
        }
        if self.mi_pending & InterruptSource::Si.bit() != 0 {
            status |= 1 << 12;
        }
        status
    }

    /// Current VI field selected by `VI_CURRENT` bit zero. Public `rcp.h` and
    /// the `osViGetCurrentField` manual define it as zero in non-interlaced
    /// mode and alternating zero/one for interlaced fields.
    pub fn vi_field(&self) -> u32 {
        const VI_STATUS_SERRATE: u32 = 1 << 6;
        if self.vi_registers[0] & VI_STATUS_SERRATE == 0 {
            return 0;
        }
        self.vi_field_interval.map_or(0, |interval| {
            let elapsed = self
                .now
                .checked_duration_since(self.vi_epoch)
                .expect("VI epoch cannot follow the device clock");
            ((elapsed.get() / interval.get()) & 1) as u32
        })
    }

    pub const fn tv_type(&self) -> Option<TvType> {
        self.tv_type
    }

    pub const fn vi_field_interval(&self) -> Option<Cycles> {
        self.vi_field_interval
    }

    /// Current sampled VI half-line. The public VI manual defines V_CURRENT
    /// as an even sequence `0,2,...` in non-interlaced mode and alternating
    /// even/odd sequences in interlaced mode. The caller-supplied field
    /// interval supplies the deterministic time base while VI_V_SYNC supplies
    /// the field size. Before either is configured the hardware-facing value
    /// remains zero.
    pub fn vi_current(&self) -> u32 {
        let Some(interval) = self.vi_field_interval else {
            return 0;
        };
        let total = self.vi_registers[6] & 0x3ff;
        if total == 0 {
            return 0;
        }
        let elapsed = self
            .now
            .checked_duration_since(self.vi_epoch)
            .expect("VI epoch cannot follow the device clock")
            .get();
        let phase = elapsed % interval.get();
        let field = self.vi_field();
        let lines_in_field = (total + 1 - field) / 2;
        if lines_in_field == 0 {
            return field;
        }
        let line = u32::try_from(
            (u128::from(phase) * u128::from(lines_in_field)) / u128::from(interval.get()),
        )
        .expect("VI line exceeds u32");
        line * 2 + field
    }

    /// The framebuffer line width in pixels, latched from `OSViMode.common.width`
    /// into VI_WIDTH (`vi_registers[2]`, a 12-bit field). `None` before the
    /// first `osViSetMode` (no mode latched), so a presenter can fall back to a
    /// default rather than a bogus zero-stride. This is the origin's line
    /// stride the CPU/RSP write into — the correct stride for reading the
    /// framebuffer, as distinct from the displayed x-scale.
    pub fn vi_width(&self) -> Option<u32> {
        let width = self.vi_registers[2] & 0x0fff;
        (width != 0).then_some(width)
    }

    /// The live VI_STATUS/VI_CONTROL register (`vi_registers[0]`), read as a
    /// direct field access. This is the cheap presenter path: it is the same
    /// value `read_live_device_mmio(0xA440_0000)` returns, but without the
    /// per-call MMIO address-decode the raw read routes through -- which a
    /// host presenter must not pay once per presented frame.
    pub fn vi_control(&self) -> u32 {
        self.vi_registers[0]
    }

    /// The guest-programmed active digital output height in lines, decoded
    /// from VI_V_START (`vi_registers[10]`) exactly as
    /// `fn64_render::ViActiveWindow` decodes it: the half-line interval
    /// `(end - start) / 2`. `None` until both the H and V intervals have been
    /// programmed, since register initialization is not atomic.
    ///
    /// A presenter must size its surface from this rather than from a fixed
    /// 240: the guest's own output rectangle is the only authority for how
    /// many scanned-out lines exist, and rows past it are memory the game
    /// never rendered into. Measured on WM2000, V_START is `0x002501ff` --
    /// half-lines 37..511, i.e. **237** output lines, not 240.
    pub fn vi_output_height(&self) -> Option<u32> {
        let horizontal = self.vi_registers[9];
        let vertical = self.vi_registers[10];
        let used = 0x03ff | (0x03ff << 16);
        if horizontal & used == 0 || vertical & used == 0 {
            return None;
        }
        let start = (vertical >> 16) & 0x03ff;
        let end = vertical & 0x03ff;
        (end > start).then(|| (end - start) / 2)
    }

    /// The physical RDRAM address the video interface is currently scanning
    /// out, read straight from VI_ORIGIN (`vi_registers[1]`, masked to 24 bits
    /// on write). `None` while the register is still zero, so a presenter can
    /// tell "no scanout programmed yet" from "scanning out address 0".
    ///
    /// This is the ONLY origin fact that holds for every game, because it is
    /// the register the hardware actually scans from. `osViSwapBuffer` is one
    /// way to reach it, not the only one: libultra's VI manager latches the
    /// swapped pointer into this register at the next retrace, but a game may
    /// equally program VI_ORIGIN itself and never call the libultra entry
    /// point at all. WM2000 is exactly that second shape -- it alternates two
    /// framebuffers (`0x0038fbc0`/`0x003c7fc0`) by writing VI_ORIGIN through
    /// raw MMIO, so `Executor::vi().current_framebuffer` (which only
    /// `osViSwapBuffer_recomp` ever sets) stays `None` forever while the game
    /// is in fact double-buffering normally. A presenter keyed on the libultra
    /// call therefore shows nothing for such a game even though every frame
    /// was rendered; keyed on this register it shows the same pixels the
    /// hardware would.
    pub fn vi_origin(&self) -> Option<u32> {
        let origin = self.vi_registers[1];
        (origin != 0).then_some(origin)
    }

    /// Install an explicit field-duration override for compatibility tests or
    /// embedders without IPL state. This clears the typed television standard;
    /// production boot should call [`Self::configure_tv_type`] instead.
    pub fn arm_vi(&mut self, interval: Cycles) -> Result<(), DeviceFault> {
        if interval.get() == 0 {
            return Err(DeviceFault::ZeroViInterval);
        }
        self.tv_type = None;
        self.vi_field_interval = Some(interval);
        self.vi_epoch = self.now;
        self.reschedule_vi_interrupt()
    }

    /// Select the IPL television standard used to interpret VI and AI timing.
    /// This does not manufacture a running VI before H_SYNC/V_SYNC exist: TV
    /// type is boot metadata, while those registers are the hardware mode.
    pub fn configure_tv_type(&mut self, tv_type: TvType) -> Result<Option<Cycles>, DeviceFault> {
        self.tv_type = Some(tv_type);
        self.vi_epoch = self.now;
        self.refresh_vi_interval_from_standard()?;
        Ok(self.vi_field_interval)
    }

    pub(crate) fn refresh_vi_interval_from_standard(&mut self) -> Result<(), DeviceFault> {
        let Some(tv_type) = self.tv_type else {
            return self.reschedule_vi_interrupt();
        };
        let Some(interval) =
            tv_type.programmed_field_cycles(self.vi_registers[7], self.vi_registers[6])
        else {
            self.vi_field_interval = None;
            if let Some(stale_token) = self.pending_vi.take() {
                self.events.retain(
                    |_, event| !matches!(event, DeviceEvent::Vi { token } if *token == stale_token),
                );
            }
            return Ok(());
        };
        if self.vi_field_interval.is_none() {
            self.vi_epoch = self.now;
        }
        self.vi_field_interval = Some(Cycles::new(interval));
        // Timing-register writes alter the running VI cadence; they do not
        // restart the beam at scanline zero. Keeping the IPL/configuration
        // epoch prevents a VI manager that rewrites its mode every retrace
        // from scheduling VI_INTR again a few scanlines after acknowledgement.
        self.reschedule_vi_interrupt()
    }

    pub(crate) fn vi_interrupt_offset(&self, interval: Cycles) -> Cycles {
        let total = self.vi_registers[6] & 0x3ff;
        if total == 0 {
            return interval;
        }
        let target = (self.vi_registers[3] & 0x3ff).min(total.saturating_sub(1));
        if target == 0 {
            return interval;
        }
        let offset = (u128::from(interval.get()) * u128::from(target)).div_ceil(u128::from(total));
        Cycles::new(
            u64::try_from(offset)
                .expect("VI interrupt offset exceeds u64")
                .max(1),
        )
    }

    pub(crate) fn reschedule_vi_interrupt(&mut self) -> Result<(), DeviceFault> {
        let Some(interval) = self.vi_field_interval else {
            return Ok(());
        };
        let offset = self.vi_interrupt_offset(interval).get();
        let elapsed = self
            .now
            .checked_duration_since(self.vi_epoch)
            .expect("VI epoch cannot follow the device clock")
            .get();
        let field = elapsed / interval.get();
        let mut deadline = self
            .vi_epoch
            .get()
            .checked_add(
                field
                    .checked_mul(interval.get())
                    .ok_or(DeviceFault::DeadlineOverflow)?,
            )
            .and_then(|base| base.checked_add(offset))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        if deadline <= self.now.get() {
            deadline = deadline
                .checked_add(interval.get())
                .ok_or(DeviceFault::DeadlineOverflow)?;
        }
        if let Some(stale_token) = self.pending_vi.take() {
            self.events.retain(
                |_, event| !matches!(event, DeviceEvent::Vi { token } if *token == stale_token),
            );
        }
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pending_vi = Some(token);
        self.events.insert(
            (crate::EmulatedInstant::new(deadline), token),
            DeviceEvent::Vi { token },
        );
        Ok(())
    }

    pub fn set_si_latency(&mut self, latency: Cycles) {
        assert!(latency.get() > 0, "SI latency must be nonzero");
        self.si_latency = latency;
    }

    pub fn set_pif_control_latency(&mut self, latency: Cycles) {
        assert!(latency.get() > 0, "PIF control latency must be nonzero");
        self.pif_control_latency = latency;
    }

    pub fn next_deadline(&self) -> Option<crate::EmulatedInstant> {
        self.events.first_key_value().map(|(key, _)| key.0)
    }

    /// Exact pending VI interrupt deadline. Hosts use this rather than adding
    /// a cached interval to an older host tick: instruction checkpoints may
    /// advance the shared clock between quiescent field pumps, and VI timing
    /// register writes may reschedule the next interrupt.
    pub fn next_vi_deadline(&self) -> Option<crate::EmulatedInstant> {
        let pending = self.pending_vi?;
        self.events
            .iter()
            .find_map(|(&(at, _), event)| match event {
                DeviceEvent::Vi { token } if *token == pending => Some(at),
                _ => None,
            })
    }

    pub fn trace(&self) -> &[DeviceTraceEvent] {
        &self.trace
    }

    pub const fn trace_summary(&self) -> DeviceTraceSummary {
        self.trace_summary
    }

    /// Control diagnostic event retention without changing device behavior or
    /// the constant-space transition summary. Disabling also releases events
    /// already retained by the current exploratory run.
    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.trace_enabled = enabled;
        if !enabled {
            self.trace.clear();
        }
    }

    /// Shim entry path. Raw MMIO converges here after latching its registers.
    pub fn start_pi_dma(&mut self, request: PiDmaRequest) -> Result<(), DeviceFault> {
        let (physical, deadline) = self.preflight_pi_dma(request)?;
        self.pi_cart_addr = physical;
        self.admit_pi_dma(request, deadline);
        Ok(())
    }

    /// Move the admitted PI transfer's bytes now, leaving the completion
    /// (busy clear, interrupt, notification) on the already-scheduled
    /// deadline. Hardware streams the bytes during the transfer window -- for
    /// the sub-millisecond transfers game audio drivers race (issue a ROM
    /// sample fill, then dispatch the RSP task without waiting on the
    /// completion queue), the bytes are in RDRAM long before any reader.
    /// Deferring the copy to the deadline event let dispatched RSP tasks read
    /// stale RDRAM that real hardware never shows them.
    pub fn commit_admitted_pi_dma_bytes<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &mut M,
    ) -> Result<(), DeviceFault> {
        let pending = self
            .pending_pi
            .expect("commit_admitted_pi_dma_bytes: no admitted PI transfer");
        assert!(
            pending.committed.is_none(),
            "commit_admitted_pi_dma_bytes: bytes already committed"
        );
        let request = pending.request;
        let completion = self
            .pi_dma
            .try_start_dma(
                rdram,
                request.direction,
                request.dram_addr,
                request.device,
                request.len,
            )
            .map_err(DeviceFault::PiTransfer)?;
        self.pi_dma
            .record_sram_dma_commit(Cycles::new(self.now.get()), completion);
        self.record(DeviceTraceKind::PiBytesCommitted(request));
        self.pending_pi
            .as_mut()
            .expect("admitted PI transfer vanished during byte commit")
            .committed = Some(completion);
        Ok(())
    }

    /// Move a not-yet-admitted (queued) PI transfer's bytes now. The manager
    /// serializes admissions, but hardware drains a burst of queued transfers
    /// in microseconds -- the bytes are in memory long before any guest read.
    /// When the transfer is later admitted, the issuer marks it precommitted
    /// via [`Self::mark_admitted_pi_dma_bytes_precommitted`] so the deadline
    /// event does not copy again.
    pub fn commit_queued_pi_dma_bytes<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &mut M,
        request: PiDmaRequest,
    ) -> Result<(), DeviceFault> {
        let completion = self
            .pi_dma
            .try_start_dma(
                rdram,
                request.direction,
                request.dram_addr,
                request.device,
                request.len,
            )
            .map_err(DeviceFault::PiTransfer)?;
        self.pi_dma
            .record_sram_dma_commit(Cycles::new(self.now.get()), completion);
        Ok(())
    }

    /// Record that the admitted PI transfer's bytes were already moved by the
    /// issuer (a managed shim's eager commit while this transfer was queued).
    /// The deadline event then reuses this evidence instead of copying again.
    pub fn mark_admitted_pi_dma_bytes_precommitted(&mut self) {
        let pending = self
            .pending_pi
            .as_mut()
            .expect("mark_admitted_pi_dma_bytes_precommitted: no admitted PI transfer");
        assert!(
            pending.committed.is_none(),
            "mark_admitted_pi_dma_bytes_precommitted: bytes already committed"
        );
        let request = pending.request;
        pending.committed = Some(DmaCompletion {
            direction: request.direction,
            dram_addr: request.dram_addr,
            device: request.device,
            len: request.len,
        });
        self.record(DeviceTraceKind::PiBytesCommitted(request));
    }

    pub(crate) fn start_latched_pi_dma(
        &mut self,
        request: PiDmaRequest,
    ) -> Result<(), DeviceFault> {
        let (_, deadline) = self.preflight_pi_dma(request)?;
        self.admit_pi_dma(request, deadline);
        Ok(())
    }

    pub(crate) fn preflight_pi_dma(
        &self,
        request: PiDmaRequest,
    ) -> Result<(u32, crate::EmulatedInstant), DeviceFault> {
        if self.pending_pi.is_some() {
            return Err(DeviceFault::PiBusy);
        }
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthPiDma);
        }
        let physical = physical_pi_device_range(request.device, request.len)?;
        let timing = self.pi_domain_timing(request.domain());
        let deadline = self
            .now
            .checked_add(self.pi_timing.completion_latency(request, timing))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        Ok((physical, deadline))
    }

    pub(crate) fn admit_pi_dma(&mut self, request: PiDmaRequest, deadline: crate::EmulatedInstant) {
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .expect("preflight PI event sequence overflow");
        self.pi_dram_addr = request.dram_addr;
        self.pi_status = PI_STATUS_DMA_BUSY;
        self.pending_pi = Some(PendingPi {
            token,
            request,
            committed: None,
        });
        self.events
            .insert((deadline, token), DeviceEvent::Pi { token });
        self.record(DeviceTraceKind::PiDmaStarted(request));
    }

    /// Enqueue one AI buffer in the hardware's current/next two-slot FIFO.
    /// Timing uses the exact public `VI_CLOCK / (DACRATE + 1)` rational and
    /// four bytes per stereo 16-bit frame; the one final ceiling prevents a
    /// nonempty buffer from completing early without feeding the truncated
    /// integer ABI playback rate back into the device clock.
    pub fn start_ai_dma(&mut self, request: AiDmaRequest) -> Result<(), DeviceFault> {
        self.accept_ai_dma(request).map(|_| ())
    }

    pub(crate) fn accept_ai_dma(
        &mut self,
        request: AiDmaRequest,
    ) -> Result<AiDmaAdmission, DeviceFault> {
        let address = request.dram_addr.offset();
        if address & !AI_DRAM_ADDR_MASK != 0 {
            return Err(DeviceFault::InvalidAiDramAddress { address });
        }
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthAiDma);
        }
        if request.len & !AI_LEN_MASK != 0 {
            return Err(DeviceFault::InvalidAiDmaLength { len: request.len });
        }
        if address
            .checked_add(request.len)
            .is_none_or(|end| end > AI_DRAM_DOMAIN_END)
        {
            return Err(DeviceFault::AiDmaRangeOverflow {
                address,
                len: request.len,
            });
        }
        if request.sample_rate_hz == 0 {
            return Err(DeviceFault::ZeroAiSampleRate);
        }
        let register_rate = self.ai_sample_rate_hz()?;
        if request.sample_rate_hz != register_rate {
            return Err(DeviceFault::AiSampleRateMismatch {
                request: request.sample_rate_hz,
                register: register_rate,
            });
        }
        if self.current_ai.is_some() && self.queued_ai.is_some() {
            return Err(DeviceFault::AiFull);
        }
        let id = AiDmaId::new(self.next_ai_dma_id);
        let next_ai_dma_id = self
            .next_ai_dma_id
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let start = if let Some(current) = self.current_ai {
            if current.deadline != current.started_at {
                self.prepare_ai_dma(id, request, current.deadline)?;
            }
            self.ai_dram_addr = request.dram_addr;
            self.queued_ai = Some(QueuedAi { id, request });
            None
        } else {
            if self.ai_control & 1 != 0 {
                let prepared = self.prepare_ai_dma(id, request, self.now)?;
                self.ai_dram_addr = request.dram_addr;
                self.commit_ai_dma(prepared);
                Some(AiDmaStart {
                    id,
                    request,
                    started_at: self.now,
                    dacrate: self.ai_dacrate,
                })
            } else {
                // AI_LEN fills the FIFO even while CONTROL disables the DAC.
                // The zero-duration marker owns the current FIFO slot without
                // scheduling a completion; the 0->1 CONTROL transition below
                // replaces it with a timed transfer at that exact guest cycle.
                self.current_ai = Some(PendingAi {
                    id,
                    token: self.next_event_sequence,
                    request,
                    started_at: self.now,
                    deadline: self.now,
                });
                self.ai_dram_addr = request.dram_addr;
                None
            }
        };
        self.next_ai_dma_id = next_ai_dma_id;
        Ok(AiDmaAdmission { id, request, start })
    }

    pub(crate) fn prepare_ai_dma(
        &self,
        id: AiDmaId,
        request: AiDmaRequest,
        started_at: crate::EmulatedInstant,
    ) -> Result<PendingAi, DeviceFault> {
        let deadline = self.ai_dma_deadline(request.len, started_at, self.ai_dacrate)?;
        let token = self.next_event_sequence;
        self.next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        Ok(PendingAi {
            id,
            token,
            request,
            started_at,
            deadline,
        })
    }

    pub(crate) fn ai_dma_deadline(
        &self,
        len: u32,
        started_at: crate::EmulatedInstant,
        dacrate: u32,
    ) -> Result<crate::EmulatedInstant, DeviceFault> {
        const BYTES_PER_STEREO_FRAME: u128 = 4;
        let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
        let frames = u128::from(len) / BYTES_PER_STEREO_FRAME;
        let duration = (frames * u128::from(CPU_CLOCK_HZ) * u128::from(dacrate + 1))
            .div_ceil(u128::from(tv_type.vi_clock_hz()));
        let duration = u64::try_from(duration.max(1)).map_err(|_| DeviceFault::DeadlineOverflow)?;
        started_at
            .checked_add(Cycles::new(duration))
            .ok_or(DeviceFault::DeadlineOverflow)
    }

    pub(crate) fn commit_ai_dma(&mut self, pending: PendingAi) {
        self.next_event_sequence = pending
            .token
            .checked_add(1)
            .expect("AI admission preflight proved the event sequence increment");
        self.current_ai = Some(pending);
        self.events.insert(
            (pending.deadline, pending.token),
            DeviceEvent::Ai {
                token: pending.token,
            },
        );
        self.record(DeviceTraceKind::AiDmaStarted(pending.request));
    }

    pub fn start_si_dma(&mut self, request: SiDmaRequest) -> Result<(), DeviceFault> {
        if self.pending_si.is_some() {
            self.si_dma_error = true;
            return Err(DeviceFault::SiBusy);
        }
        let deadline = self
            .now
            .checked_add(self.si_latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.si_dram_addr = request.dram_addr;
        self.pending_si = Some(PendingSi::Dma { token, request });
        self.events
            .insert((deadline, token), DeviceEvent::Si { token });
        self.record(DeviceTraceKind::SiDmaStarted(request));
        Ok(())
    }

    pub(crate) fn validate_sp_dma(request: SpDmaRequest) -> Result<(), DeviceFault> {
        let total = request.total_bytes();
        let remaining = RSP_MEMORY_BANK_SIZE - request.mem_addr.offset();
        if total > remaining {
            return Err(DeviceFault::SpDmaMemory(RspMemoryError::CrossesBank {
                addr: request.mem_addr,
                len: total,
            }));
        }
        let row_stride = request
            .line_len()
            .checked_add(request.skip())
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        let last_row = request
            .line_count()
            .saturating_sub(1)
            .checked_mul(row_stride)
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        let end = (request.dram_addr.offset() as usize)
            .checked_add(last_row)
            .and_then(|start| start.checked_add(request.line_len()))
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        if end > 0x0100_0000 {
            return Err(DeviceFault::SpDmaDramRangeOverflow { request });
        }
        Ok(())
    }

    pub(crate) fn begin_sp_dma(&mut self, request: SpDmaRequest) -> Result<(), DeviceFault> {
        let transfer_cycles =
            u64::try_from(request.total_bytes() / 8).map_err(|_| DeviceFault::DeadlineOverflow)?;
        let latency = self
            .sp_dma_setup_cycles
            .checked_add(Cycles::new(transfer_cycles))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let deadline = self
            .now
            .checked_add(latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.active_sp_dma = Some(PendingSpDma { token, request });
        self.events
            .insert((deadline, token), DeviceEvent::SpDma { token });
        self.record(DeviceTraceKind::SpDmaStarted(request));
        Ok(())
    }

    pub(crate) fn start_sp_dma(&mut self, request: SpDmaRequest) -> Result<(), DeviceFault> {
        Self::validate_sp_dma(request)?;
        if self.active_sp_dma.is_none() {
            self.begin_sp_dma(request)
        } else if self.queued_sp_dma.is_none() {
            self.queued_sp_dma = Some(request);
            self.record(DeviceTraceKind::SpDmaQueued(request));
            Ok(())
        } else {
            Err(DeviceFault::SpDmaFull)
        }
    }

    /// Apply the SP status command register's documented clear/set pairs.
    pub fn write_sp_status(&mut self, command: u32) {
        if command & (1 << 0) != 0 {
            self.sp_status &= !SP_STATUS_HALT;
        }
        if command & (1 << 1) != 0 {
            self.sp_status |= SP_STATUS_HALT;
        }
        if command & (1 << 2) != 0 {
            self.sp_status &= !SP_STATUS_BROKE;
        }
        if command & (1 << 3) != 0 {
            self.clear_interrupt(InterruptSource::Sp);
        }
        if command & (1 << 4) != 0 {
            self.raise_interrupt(InterruptSource::Sp);
        }
        apply_device_clear_set_pair(&mut self.sp_status, command, 5, 6, SP_STATUS_SINGLE_STEP);
        apply_device_clear_set_pair(
            &mut self.sp_status,
            command,
            7,
            8,
            SP_STATUS_INTERRUPT_ON_BREAK,
        );
        for signal in 0..8 {
            apply_device_clear_set_pair(
                &mut self.sp_status,
                command,
                9 + signal * 2,
                10 + signal * 2,
                1 << (7 + signal),
            );
        }
    }
}

#[cfg(test)]
mod ai_sample_period_tests {
    use super::AiSamplePeriod;

    #[test]
    fn exact_period_retains_the_fraction_discarded_by_whole_hz() {
        let period = AiSamplePeriod::new(48_681_812, 1_520);
        assert_eq!(period.video_clock_hz(), 48_681_812);
        assert_eq!(period.dacrate_plus_one(), 1_520);
        assert_eq!(period.floor_hz(), 32_027);
        assert_eq!(
            period.video_clock_hz() % period.dacrate_plus_one(),
            772,
            "the vector must exercise a non-integral physical sample rate"
        );
    }
}
