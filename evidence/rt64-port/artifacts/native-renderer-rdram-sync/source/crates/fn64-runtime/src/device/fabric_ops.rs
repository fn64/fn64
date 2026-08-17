use super::*;

impl<R: RomStorage, T: PiTimingModel> DeviceFabric<R, T> {
    pub fn set_sp_pc(&mut self, pc: u32) {
        self.sp_pc = pc & 0x0ffc;
    }

    pub const fn sp_pc(&self) -> u32 {
        self.sp_pc
    }

    /// Snapshot every SP/DPC register owned by synchronous RSP execution.
    ///
    /// DMA BUSY/FULL in the SP status are guest-visible values derived from
    /// the fabric's active and queued DMA slots. No DPC transaction token or
    /// renderer payload crosses this architectural-state boundary.
    pub const fn rsp_execution_state(&self) -> RspExecutionState {
        RspExecutionState {
            pc: self.sp_pc,
            sp_status: self.sp_status(),
            sp_semaphore: self.sp_semaphore,
            sp_dma_mem_addr: self.sp_mem_addr,
            sp_dma_dram_addr: self.sp_dram_addr,
            sp_dma_read_length: self.sp_rd_len,
            sp_dma_write_length: self.sp_wr_len,
            dpc_start: self.dpc.start,
            dpc_end: self.dpc.end,
            dpc_current: self.dpc.current,
            dpc_status: self.dpc.status,
            dpc_clock: self.dpc.clock.get(),
            dpc_busy: self.dpc.busy.get(),
            dpc_pipe_busy: self.dpc.pipe_busy.get(),
            dpc_tmem_busy: self.dpc.tmem_busy.get(),
        }
    }

    /// Validate a complete synchronous RSP register image without mutation.
    ///
    /// A higher-layer transactional adapter may need to perform a fallible
    /// renderer operation before publishing this state. It must retain
    /// exclusive ownership of the fabric from this preflight through
    /// [`Self::commit_complete_rsp_execution_state`]; otherwise a new DPC
    /// owner could invalidate the successful preflight.
    pub fn preflight_complete_rsp_execution_state(
        &self,
        state: &RspExecutionState,
    ) -> Result<(), DeviceFault> {
        if state.pc & !0x0ffc != 0 {
            return Err(DeviceFault::InvalidRspExecutionPc { pc: state.pc });
        }
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        Ok(())
    }

    /// Atomically commit registers produced by speculative RSP execution.
    ///
    /// A pending DPC renderer transaction owns its rollback register image, so
    /// replacing DPC registers while one is live is rejected. Address latches
    /// apply the same hardware masks as raw MMIO. SP DMA BUSY/FULL remain
    /// derived from the fabric queues rather than copied from an interpreter.
    pub fn commit_complete_rsp_execution_state(
        &mut self,
        state: RspExecutionState,
    ) -> Result<(), DeviceFault> {
        self.preflight_complete_rsp_execution_state(&state)?;

        self.sp_pc = state.pc;
        self.sp_status = state.sp_status & !(SP_STATUS_DMA_BUSY | SP_STATUS_DMA_FULL);
        self.sp_semaphore = state.sp_semaphore;
        self.sp_mem_addr = state.sp_dma_mem_addr;
        self.sp_dram_addr = RdramAddr::from_offset(state.sp_dma_dram_addr.offset() & 0x00ff_ffff);
        self.sp_rd_len = state.sp_dma_read_length;
        self.sp_wr_len = state.sp_dma_write_length;
        self.dpc = DpcRegisters {
            start: state.dpc_start & DPC_ADDR_MASK,
            end: state.dpc_end & DPC_ADDR_MASK,
            current: state.dpc_current & DPC_ADDR_MASK,
            status: state.dpc_status,
            clock: DpcCounter24::from_register(state.dpc_clock),
            busy: DpcCounter24::from_register(state.dpc_busy),
            pipe_busy: DpcCounter24::from_register(state.dpc_pipe_busy),
            tmem_busy: DpcCounter24::from_register(state.dpc_tmem_busy),
        };
        Ok(())
    }

    /// Commit architectural state produced by a synchronous RSP execution.
    /// DMA BUSY/FULL are derived from the fabric's queues and cannot be
    /// overwritten by an interpreter snapshot.
    pub fn commit_rsp_execution_state(&mut self, pc: u32, status: u32) {
        self.set_sp_pc(pc);
        self.sp_status = status & !(SP_STATUS_DMA_BUSY | SP_STATUS_DMA_FULL);
    }

    /// Complete the CPU-side `osSpTaskLoad` admission sequence at its shim
    /// return boundary. The public RSP guide's "Starting RSP Tasks" algorithm
    /// requires the 64-byte `OSTask` at DMEM `0xfc0`, rspboot at IMEM `0`, and
    /// PC `0`. Raw SP DMA remains independently timed; this helper represents
    /// the two DMA-and-poll loops as already complete when the synchronous OS
    /// function returns.
    pub fn admit_sp_task<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &M,
        task_addr: RdramAddr,
        header: crate::rsp::OsTaskHeader,
    ) -> Result<(), DeviceFault> {
        let boot_size = header
            .ucode_boot_size
            .checked_add(7)
            .map(|size| size & !7)
            .filter(|size| *size != 0 && *size as usize <= RSP_MEMORY_BANK_SIZE)
            .ok_or(DeviceFault::InvalidSpTaskBootSize {
                size: header.ucode_boot_size,
            })? as usize;
        // OSTask pointers may be physical or direct-mapped KSEG0/KSEG1.
        // Both reduce to the public 29-bit physical bus address this way.
        let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
        let boot = rdram.dma_read_bytes_flat(boot_addr as usize, boot_size);
        self.admit_sp_task_with_boot_image(rdram, task_addr, header, &boot)
    }

    /// Variant of [`Self::admit_sp_task`] for a host whose CPU cache and
    /// physical DRAM share one backing allocation. `boot` is the CPU-visible
    /// rspboot text selected by the OS loader, while `rdram` remains the
    /// physical image used for the task header and all device-visible data.
    pub fn admit_sp_task_with_boot_image<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &M,
        task_addr: RdramAddr,
        header: crate::rsp::OsTaskHeader,
        boot: &[u8],
    ) -> Result<(), DeviceFault> {
        if self.sp_status() & SP_STATUS_HALT == 0 || self.pending_sp.is_some() {
            return Err(DeviceFault::SpTaskNotHalted);
        }
        if self.active_sp_dma.is_some() || self.queued_sp_dma.is_some() {
            return Err(DeviceFault::SpDmaFull);
        }
        let boot_size = header
            .ucode_boot_size
            .checked_add(7)
            .map(|size| size & !7)
            .filter(|size| *size != 0 && *size as usize <= RSP_MEMORY_BANK_SIZE)
            .ok_or(DeviceFault::InvalidSpTaskBootSize {
                size: header.ucode_boot_size,
            })? as usize;
        assert_eq!(
            boot.len(),
            boot_size,
            "osSpTaskLoad cached rspboot image has {} bytes; aligned task size requires {boot_size}",
            boot.len()
        );
        let task_bytes = rdram.dma_read_bytes_flat(task_addr.offset() as usize, 64);
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x0fc0), &task_bytes)
            .map_err(DeviceFault::SpDmaMemory)?;

        let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x1000), boot)
            .map_err(DeviceFault::SpDmaMemory)?;
        self.sp_mem_addr = RspMemAddr::from_register(0x1000);
        self.sp_dram_addr = RdramAddr::from_offset(boot_addr);
        self.sp_pc = 0;
        self.record(DeviceTraceKind::SpTaskAdmitted { task_addr, header });
        Ok(())
    }

    /// Schedule the externally visible completion of work already executed by
    /// the HLE task backend. SP completes one deterministic guest cycle after
    /// the kick; graphics DP completion follows one cycle later, preserving
    /// the architectural SP-before-DP ordering without claiming RDP timing.
    pub fn start_rcp_task(&mut self, plan: RcpTaskCompletionPlan) -> Result<(), DeviceFault> {
        self.start_rcp_task_with_latency(plan, Cycles::new(1))
    }

    /// Schedule completion after a measured amount of synchronous RSP work.
    /// The caller has already executed that work while the guest is suspended;
    /// this delay controls only when its architectural interrupt is observable.
    pub fn start_rcp_task_with_latency(
        &mut self,
        plan: RcpTaskCompletionPlan,
        sp_latency: Cycles,
    ) -> Result<(), DeviceFault> {
        if self.pending_sp.is_some() {
            return Err(DeviceFault::SpBusy);
        }
        if plan.reaches_dp_full_sync() && self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        self.begin_rcp_task()?;
        self.finish_rcp_task(plan, sp_latency)
    }

    /// Mark an asynchronously chunked RSP task as running without fabricating
    /// a completion deadline. The retained token becomes schedulable exactly
    /// once through [`Self::finish_rcp_task`].
    pub fn begin_rcp_task(&mut self) -> Result<(), DeviceFault> {
        if self.pending_sp.is_some() {
            return Err(DeviceFault::SpBusy);
        }
        let sp_token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pending_sp = Some(sp_token);
        self.sp_status &= !(SP_STATUS_HALT | SP_STATUS_BROKE);
        Ok(())
    }

    /// Schedule the sole completion of work previously admitted by
    /// [`Self::begin_rcp_task`].
    pub fn finish_rcp_task(
        &mut self,
        plan: RcpTaskCompletionPlan,
        sp_latency: Cycles,
    ) -> Result<(), DeviceFault> {
        let needs_dp = plan.reaches_dp_full_sync();
        let sp_token = self.pending_sp.ok_or(DeviceFault::SpNotRunning)?;
        if self
            .events
            .values()
            .any(|event| matches!(event, DeviceEvent::Sp { token } if *token == sp_token))
        {
            return Err(DeviceFault::SpBusy);
        }
        if needs_dp && self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        let sp_deadline = self
            .now
            .checked_add(sp_latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.events
            .insert((sp_deadline, sp_token), DeviceEvent::Sp { token: sp_token });
        if needs_dp {
            let dp_deadline = self
                .now
                .checked_add(
                    sp_latency
                        .checked_add(Cycles::new(1))
                        .ok_or(DeviceFault::DeadlineOverflow)?,
                )
                .ok_or(DeviceFault::DeadlineOverflow)?;
            let dp_token = self.next_event_sequence;
            self.next_event_sequence = self
                .next_event_sequence
                .checked_add(1)
                .ok_or(DeviceFault::DeadlineOverflow)?;
            self.pending_dp = Some(dp_token);
            self.events
                .insert((dp_deadline, dp_token), DeviceEvent::Dp { token: dp_token });
        }
        self.record(DeviceTraceKind::RcpTaskStarted { needs_dp });
        Ok(())
    }

    /// Prove that one raw FullSync can reserve the sole DP completion slot.
    /// This is nonmutating so a renderer can be rejected before it observes
    /// or changes guest memory.
    pub fn preflight_dp_full_sync(&self, latency: Cycles) -> Result<(), DeviceFault> {
        assert!(latency.get() > 0, "DP FullSync latency must be nonzero");
        // Interleaving closed here: CPU thread A may submit a second raw DPC
        // FullSync before thread B services the first DP event. The synchronous
        // renderer path calls this before backend entry, and the single-owner
        // dispatcher cannot advance devices until it either commits or
        // unwinds, so the checked slot/deadline/token remain available.
        if self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        self.now
            .checked_add(latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        Ok(())
    }

    /// Schedule the DP interrupt generated by a raw CPU/RSP DPC range that
    /// reached FullSync without starting a new SP task.
    pub fn start_dp_full_sync(&mut self, latency: Cycles) -> Result<(), DeviceFault> {
        self.preflight_dp_full_sync(latency)?;
        let deadline = self
            .now
            .checked_add(latency)
            .expect("DP FullSync deadline was preflighted");
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .expect("DP FullSync event token was preflighted");
        self.pending_dp = Some(token);
        self.events
            .insert((deadline, token), DeviceEvent::Dp { token });
        Ok(())
    }

    pub fn read_mmio(&mut self, addr: MmioAddr) -> Result<u32, DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            addr if (SP_DMEM_START..SP_IMEM_END).contains(&addr.get()) => self
                .rsp_memory
                .read_word(RspMemAddr::from_register(addr.get() - SP_DMEM_START))
                .map_err(DeviceFault::SpDmaMemory),
            SP_MEM_ADDR_REG => Ok(self.sp_mem_addr.get() as u32),
            SP_DRAM_ADDR_REG => Ok(self.sp_dram_addr.offset()),
            SP_RD_LEN_REG => Ok(self.sp_rd_len),
            SP_WR_LEN_REG => Ok(self.sp_wr_len),
            SP_STATUS_REG => Ok(self.sp_status()),
            SP_DMA_FULL_REG => Ok(u32::from(self.queued_sp_dma.is_some())),
            SP_DMA_BUSY_REG => Ok(u32::from(self.active_sp_dma.is_some())),
            SP_SEMAPHORE_REG => {
                let previous = u32::from(self.sp_semaphore);
                self.sp_semaphore = true;
                Ok(previous)
            }
            SP_PC_REG => Ok(self.sp_pc),
            DPC_START_REG => Ok(self.dpc.start),
            DPC_END_REG => Ok(self.dpc.end),
            DPC_CURRENT_REG => Ok(self.dpc.current),
            DPC_STATUS_REG => Ok(self.dpc.status),
            DPC_CLOCK_REG => Ok(self.dpc.clock.get()),
            DPC_BUFBUSY_REG => Ok(self.dpc.busy.get()),
            DPC_PIPEBUSY_REG => Ok(self.dpc.pipe_busy.get()),
            DPC_TMEM_REG => Ok(self.dpc.tmem_busy.get()),
            MI_INTR_REG => Ok(self.mi_pending),
            MI_INTR_MASK_REG => Ok(self.mi_mask),
            VI_CURRENT_REG => Ok(self.vi_current()),
            addr if (VI_STATUS_REG.get()..=VI_Y_SCALE_REG.get()).contains(&addr.get()) => {
                let index = ((addr.get() - VI_STATUS_REG.get()) / 4) as usize;
                Ok(self.vi_registers[index])
            }
            AI_DRAM_ADDR_REG => Ok(self.ai_dram_addr.offset()),
            AI_LEN_REG => Ok(self.ai_length()),
            AI_CONTROL_REG => Ok(self.ai_control),
            AI_STATUS_REG => Ok(self.ai_status()),
            AI_DACRATE_REG => Ok(self.ai_dacrate),
            AI_BITRATE_REG => Ok(self.ai_bitrate),
            PI_DRAM_ADDR_REG => Ok(self.pi_dram_addr.offset()),
            PI_CART_ADDR_REG => Ok(self.pi_cart_addr),
            PI_STATUS_REG => Ok(self.pi_status),
            PI_DOM1_LAT_REG => Ok(self.pi_domain1.latency as u32),
            PI_DOM1_PWD_REG => Ok(self.pi_domain1.pulse_width as u32),
            PI_DOM1_PGS_REG => Ok(self.pi_domain1.page_size as u32),
            PI_DOM1_RLS_REG => Ok(self.pi_domain1.release as u32),
            PI_DOM2_LAT_REG => Ok(self.pi_domain2.latency as u32),
            PI_DOM2_PWD_REG => Ok(self.pi_domain2.pulse_width as u32),
            PI_DOM2_PGS_REG => Ok(self.pi_domain2.page_size as u32),
            PI_DOM2_RLS_REG => Ok(self.pi_domain2.release as u32),
            SI_DRAM_ADDR_REG => Ok(self.si_dram_addr.offset()),
            SI_STATUS_REG => Ok(self.si_status()),
            _ => Err(DeviceFault::UnmodeledMmioRead { addr }),
        }
    }

    pub fn write_mmio(
        &mut self,
        addr: MmioAddr,
        value: u32,
    ) -> Result<DeviceMmioWriteEffect, DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            AI_DRAM_ADDR_REG => {
                self.ai_dram_addr = RdramAddr::from_offset(value & AI_DRAM_ADDR_MASK);
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_LEN_REG => {
                let request = AiDmaRequest {
                    dram_addr: self.ai_dram_addr,
                    len: value & AI_LEN_MASK,
                    sample_rate_hz: self.ai_sample_rate_hz()?,
                };
                self.start_ai_dma(request)?;
                return Ok(DeviceMmioWriteEffect::AiDmaStarted(request));
            }
            AI_CONTROL_REG => {
                let requested = value & 1;
                if self.ai_control == 1
                    && requested == 0
                    && (self.current_ai.is_some() || self.queued_ai.is_some())
                {
                    return Err(DeviceFault::AiControlWhileBusy {
                        current: self.ai_control,
                        requested,
                    });
                }
                let prepared = if requested == 1 {
                    self.current_ai
                        .filter(|pending| pending.deadline == pending.started_at)
                        .map(|dormant| self.prepare_ai_dma(dormant.request, self.now))
                        .transpose()?
                } else {
                    None
                };
                self.ai_control = requested;
                if let Some(prepared) = prepared {
                    self.commit_ai_dma(prepared);
                }
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_STATUS_REG => {
                self.clear_interrupt(InterruptSource::Ai);
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_DACRATE_REG => {
                let dacrate = value & AI_DACRATE_MASK;
                if self.current_ai.is_some() || self.queued_ai.is_some() {
                    if dacrate == self.ai_dacrate {
                        return Ok(DeviceMmioWriteEffect::None);
                    }
                    return Err(DeviceFault::AiDacrateWhileBusy {
                        current: self.ai_dacrate,
                        requested: dacrate,
                    });
                }
                let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
                self.ai_dacrate = dacrate;
                return Ok(DeviceMmioWriteEffect::AiFrequencyChanged {
                    sample_rate_hz: tv_type.vi_clock_hz() / (dacrate + 1),
                });
            }
            AI_BITRATE_REG => {
                let bitrate = value & AI_BITRATE_MASK;
                if self.current_ai.is_some() || self.queued_ai.is_some() {
                    if bitrate == self.ai_bitrate {
                        return Ok(DeviceMmioWriteEffect::None);
                    }
                    return Err(DeviceFault::AiBitrateWhileBusy {
                        current: self.ai_bitrate,
                        requested: bitrate,
                    });
                }
                self.ai_bitrate = bitrate;
                return Ok(DeviceMmioWriteEffect::None);
            }
            DPC_START_REG => {
                if self.pending_dpc.is_some() {
                    return Err(DeviceFault::DpBusy);
                }
                if self.dpc.status & DPC_STATUS_START_VALID == 0 {
                    self.dpc.start = value & DPC_ADDR_MASK;
                    self.dpc.status |= DPC_STATUS_START_VALID;
                }
                return Ok(DeviceMmioWriteEffect::None);
            }
            DPC_END_REG => {
                if self.pending_dpc.is_some() {
                    return Err(DeviceFault::DpBusy);
                }
                let rollback = self.dpc;
                let end = value & DPC_ADDR_MASK;
                let start = if self.dpc.status & DPC_STATUS_START_VALID != 0 {
                    self.dpc.start
                } else {
                    self.dpc.current
                };
                if start == end {
                    self.dpc.end = end;
                    self.dpc.current = start;
                    self.dpc.status &= !DPC_STATUS_START_VALID;
                    return Ok(DeviceMmioWriteEffect::None);
                }
                let source = if self.dpc.status & DPC_STATUS_XBUS_DMEM_DMA != 0 {
                    DpcSubmissionSource::Dmem
                } else {
                    DpcSubmissionSource::Rdram
                };
                Self::validate_dpc_range(source, start, end)?;
                self.dpc.end = end;
                self.dpc.current = start;
                self.dpc.status &= !DPC_STATUS_START_VALID;
                if self.dpc.status & DPC_STATUS_FREEZE != 0 {
                    return Ok(DeviceMmioWriteEffect::None);
                }
                let submission = self.begin_dpc_submission(source, start, end, rollback)?;
                return Ok(DeviceMmioWriteEffect::DpcSubmissionRequested(submission));
            }
            DPC_CURRENT_REG => {
                return Err(DeviceFault::UnmodeledMmioWrite { addr, value });
            }
            DPC_STATUS_REG => {
                let was_frozen = self.dpc.status & DPC_STATUS_FREEZE != 0;
                apply_dpc_status_mode_commands(&mut self.dpc.status, value);
                // Interleaving closed: END admission captured a status rollback,
                // then the CPU issues a mode command before the renderer cancels.
                // Mirror the command into the rollback so cancellation reverses
                // only the admission and does not discard this later command.
                if let Some(pending) = self.pending_dpc.as_mut() {
                    apply_dpc_status_mode_commands(&mut pending.rollback.status, value);
                }
                if value & DPC_STATUS_CLEAR_TMEM_COUNTER_COMMAND != 0 {
                    self.dpc.tmem_busy = DpcCounter24::ZERO;
                }
                if value & DPC_STATUS_CLEAR_PIPE_COUNTER_COMMAND != 0 {
                    self.dpc.pipe_busy = DpcCounter24::ZERO;
                }
                if value & DPC_STATUS_CLEAR_CMD_COUNTER_COMMAND != 0 {
                    self.dpc.busy = DpcCounter24::ZERO;
                }
                if value & DPC_STATUS_CLEAR_CLOCK_COUNTER_COMMAND != 0 {
                    self.dpc.clock = DpcCounter24::ZERO;
                }
                if was_frozen
                    && self.dpc.status & DPC_STATUS_FREEZE == 0
                    && self.pending_dpc.is_none()
                    && self.dpc.current < self.dpc.end
                {
                    let source = if self.dpc.status & DPC_STATUS_XBUS_DMEM_DMA != 0 {
                        DpcSubmissionSource::Dmem
                    } else {
                        DpcSubmissionSource::Rdram
                    };
                    Self::validate_dpc_range(source, self.dpc.current, self.dpc.end)?;
                    let rollback = self.dpc;
                    let submission = self.begin_dpc_submission(
                        source,
                        self.dpc.current,
                        self.dpc.end,
                        rollback,
                    )?;
                    return Ok(DeviceMmioWriteEffect::DpcSubmissionRequested(submission));
                }
                return Ok(DeviceMmioWriteEffect::None);
            }
            SP_STATUS_REG => {
                // Hardware starts the RSP when a STATUS write clears HALT on a
                // halted unit; it keeps running if it was already going. Report
                // only the halted -> running edge so a repeated clear-halt does
                // not re-enter a running task.
                let was_halted = self.sp_status & SP_STATUS_HALT != 0;
                self.write_sp_status(value);
                let now_halted = self.sp_status & SP_STATUS_HALT != 0;
                return Ok(if was_halted && !now_halted {
                    DeviceMmioWriteEffect::RspStartRequested { pc: self.sp_pc() }
                } else {
                    DeviceMmioWriteEffect::None
                });
            }
            _ => {}
        }
        self.write_mmio_without_effect(addr, value)?;
        Ok(DeviceMmioWriteEffect::None)
    }

    pub(crate) fn write_mmio_without_effect(
        &mut self,
        addr: MmioAddr,
        value: u32,
    ) -> Result<(), DeviceFault> {
        match addr {
            addr if (SP_DMEM_START..SP_IMEM_END).contains(&addr.get()) => self
                .rsp_memory
                .write_word(RspMemAddr::from_register(addr.get() - SP_DMEM_START), value)
                .map_err(DeviceFault::SpDmaMemory),
            SP_MEM_ADDR_REG => {
                self.sp_mem_addr = RspMemAddr::from_register(value);
                Ok(())
            }
            SP_DRAM_ADDR_REG => {
                self.sp_dram_addr = RdramAddr::from_offset(value & 0x00ff_ffff);
                Ok(())
            }
            SP_RD_LEN_REG => {
                self.sp_rd_len = value;
                self.start_sp_dma(SpDmaRequest {
                    direction: SpDmaDirection::RdramToRsp,
                    mem_addr: self.sp_mem_addr.dma_aligned(),
                    dram_addr: RdramAddr::from_offset(self.sp_dram_addr.offset() & !7),
                    encoded_len: value,
                })
            }
            SP_WR_LEN_REG => {
                self.sp_wr_len = value;
                self.start_sp_dma(SpDmaRequest {
                    direction: SpDmaDirection::RspToRdram,
                    mem_addr: self.sp_mem_addr.dma_aligned(),
                    dram_addr: RdramAddr::from_offset(self.sp_dram_addr.offset() & !7),
                    encoded_len: value,
                })
            }
            SP_STATUS_REG => {
                self.write_sp_status(value);
                Ok(())
            }
            SP_SEMAPHORE_REG if value == 0 => {
                self.sp_semaphore = false;
                Ok(())
            }
            SP_SEMAPHORE_REG => Err(DeviceFault::InvalidSpSemaphoreWrite { value }),
            SP_PC_REG => {
                self.set_sp_pc(value);
                Ok(())
            }
            MI_INTR_MASK_REG if value & !0x0FFF == 0 => {
                // MI_INTR_MASK is a command register, not a replacement
                // value. Public N64 hardware documentation assigns one
                // clear/set pair to each MI source, in MI_INTR bit order.
                // Apply clear before set so a malformed command containing
                // both leaves the source enabled, matching the paired
                // clear-then-set register behavior.
                for (index, source) in [
                    InterruptSource::Sp,
                    InterruptSource::Si,
                    InterruptSource::Ai,
                    InterruptSource::Vi,
                    InterruptSource::Pi,
                    InterruptSource::Dp,
                ]
                .into_iter()
                .enumerate()
                {
                    let clear = 1 << (index * 2);
                    let set = 1 << (index * 2 + 1);
                    if value & clear != 0 {
                        self.mi_mask &= !source.bit();
                    }
                    if value & set != 0 {
                        self.mi_mask |= source.bit();
                    }
                }
                Ok(())
            }
            VI_CURRENT_REG => {
                // VI_CURRENT is read-only as a counter. Any write is the
                // documented acknowledgement for the level-sensitive VI
                // source; it must not replace the sampled line value.
                self.clear_interrupt(InterruptSource::Vi);
                Ok(())
            }
            addr if (VI_STATUS_REG.get()..=VI_Y_SCALE_REG.get()).contains(&addr.get()) => {
                let index = ((addr.get() - VI_STATUS_REG.get()) / 4) as usize;
                self.vi_registers[index] = match addr {
                    VI_STATUS_REG => value & 0x1ffff,
                    VI_ORIGIN_REG => value & 0x00ff_ffff,
                    VI_INTR_REG | VI_V_SYNC_REG => value & 0x3ff,
                    _ => value,
                };
                if matches!(addr, VI_V_SYNC_REG | VI_H_SYNC_REG) {
                    if self.tv_type.is_some() {
                        self.refresh_vi_interval_from_standard()?;
                    } else if addr == VI_V_SYNC_REG {
                        self.reschedule_vi_interrupt()?;
                    }
                } else if addr == VI_INTR_REG {
                    self.reschedule_vi_interrupt()?;
                }
                Ok(())
            }
            PI_DRAM_ADDR_REG => {
                self.pi_dram_addr = RdramAddr::from_offset(value);
                Ok(())
            }
            PI_CART_ADDR_REG => {
                self.pi_cart_addr = value;
                Ok(())
            }
            PI_RD_LEN_REG => {
                let len = value
                    .checked_add(1)
                    .ok_or(DeviceFault::PiLengthOverflow { encoded: value })?;
                let device = decode_raw_pi_device_address(self.pi_cart_addr)?;
                self.start_latched_pi_dma(PiDmaRequest {
                    direction: DmaDirection::FromRdram,
                    dram_addr: self.pi_dram_addr,
                    device,
                    len,
                })
            }
            PI_WR_LEN_REG => {
                let len = value
                    .checked_add(1)
                    .ok_or(DeviceFault::PiLengthOverflow { encoded: value })?;
                let device = decode_raw_pi_device_address(self.pi_cart_addr)?;
                self.start_latched_pi_dma(PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: self.pi_dram_addr,
                    device,
                    len,
                })
            }
            PI_STATUS_REG if value & !0b11 == 0 => {
                // Public PI_STATUS command bits: bit 0 resets/aborts PI and
                // bit 1 clears the PI interrupt. An aborted event remains in
                // the heap but its token no longer owns `pending_pi`, so it
                // cannot later copy bytes or raise MI.
                if value & 0b1 != 0 {
                    self.pending_pi = None;
                    self.pi_status = 0;
                }
                if value & 0b10 != 0 {
                    self.clear_interrupt(InterruptSource::Pi);
                }
                Ok(())
            }
            PI_DOM1_LAT_REG => {
                self.pi_domain1.latency = value as u8;
                Ok(())
            }
            PI_DOM1_PWD_REG => {
                self.pi_domain1.pulse_width = value as u8;
                Ok(())
            }
            PI_DOM1_PGS_REG => {
                self.pi_domain1.page_size = (value & 0xF) as u8;
                Ok(())
            }
            PI_DOM1_RLS_REG => {
                self.pi_domain1.release = (value & 0x3) as u8;
                Ok(())
            }
            PI_DOM2_LAT_REG => {
                self.pi_domain2.latency = value as u8;
                Ok(())
            }
            PI_DOM2_PWD_REG => {
                self.pi_domain2.pulse_width = value as u8;
                Ok(())
            }
            PI_DOM2_PGS_REG => {
                self.pi_domain2.page_size = (value & 0xF) as u8;
                Ok(())
            }
            PI_DOM2_RLS_REG => {
                self.pi_domain2.release = (value & 0x3) as u8;
                Ok(())
            }
            SI_DRAM_ADDR_REG => {
                self.si_dram_addr = RdramAddr::from_offset(value & 0x00FF_FFFF);
                Ok(())
            }
            SI_PIF_ADDR_RD64B_REG => self.start_si_dma(SiDmaRequest {
                kind: SiDmaKind::PifToDram,
                dram_addr: self.si_dram_addr,
            }),
            SI_PIF_ADDR_WR64B_REG => self.start_si_dma(SiDmaRequest {
                kind: SiDmaKind::DramToPif,
                dram_addr: self.si_dram_addr,
            }),
            SI_STATUS_REG => {
                self.clear_interrupt(InterruptSource::Si);
                Ok(())
            }
            _ => Err(DeviceFault::UnmodeledMmioWrite { addr, value }),
        }
    }

    /// Advance deterministic device time and fully commit every due event.
    /// Notifications are returned only after their device and MI state is
    /// guest-visible, so the executor can post them before resuming a thread.
    /// Advance the fabric clock when nothing is due, touching no memory.
    ///
    /// `advance_to`/`advance_to_with_pif` both require a `DmaMemory` because a
    /// due event may commit bytes through it. When NOTHING is due there is no
    /// such commit, and the whole operation is moving `now` forward.
    ///
    /// This exists because the obvious shortcut -- calling `advance_to_with_pif`
    /// with an empty view -- is unsound and silently corrupts state: the view
    /// is still handed to the fabric as `DmaMemory`, so anything committed
    /// lands in a zero-length buffer. That zeroed WM2000's executable baseline
    /// and cost a full debugging session.
    ///
    /// Returns `false` and does nothing when a deadline IS due, so a caller
    /// cannot use it to skip real work; the assertion is enforced, not assumed.
    pub fn advance_clock_if_idle(&mut self, requested: Cycles) -> bool {
        if requested < self.now {
            return false;
        }
        if self
            .next_deadline()
            .is_some_and(|deadline| deadline <= requested)
        {
            return false;
        }
        self.now = requested;
        true
    }

    pub fn advance_to<M: DmaMemory + ?Sized>(
        &mut self,
        requested: Cycles,
        rdram: &mut M,
    ) -> Result<Vec<DeviceNotification>, DeviceFault> {
        self.advance_to_with_pif(requested, rdram, |_, _, _| {
            panic!("SI DRAM-to-PIF completion requires a PIF command handler")
        })
    }

    pub fn advance_to_with_pif<M: DmaMemory + ?Sized>(
        &mut self,
        requested: Cycles,
        rdram: &mut M,
        mut execute_pif: impl FnMut(Cycles, &mut [u8; 64], &mut PiDma<R>),
    ) -> Result<Vec<DeviceNotification>, DeviceFault> {
        if requested < self.now {
            return Err(DeviceFault::TimeWentBack {
                now: self.now,
                requested,
            });
        }
        let mut notifications = Vec::new();
        while let Some((&key, &event)) = self.events.first_key_value() {
            if key.0 > requested {
                break;
            }
            let prepared_ai_promotion = match event {
                DeviceEvent::Ai { token }
                    if self
                        .current_ai
                        .is_some_and(|current| current.token == token) =>
                {
                    self.queued_ai
                        .map(|next| self.prepare_ai_dma(next, key.0))
                        .transpose()?
                }
                _ => None,
            };
            self.events.remove(&key);
            self.now = key.0;
            self.pi_dma.advance_eeprom_to(self.now);
            match event {
                DeviceEvent::Pi { token } => {
                    let Some(pending) = self.pending_pi else {
                        continue;
                    };
                    if pending.token != token {
                        continue;
                    }
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
                    self.pi_dma.record_sram_dma_commit(self.now, completion);
                    self.record(DeviceTraceKind::PiBytesCommitted(request));
                    self.pending_pi = None;
                    self.pi_status &= !PI_STATUS_DMA_BUSY;
                    self.record(DeviceTraceKind::PiBusyCleared);
                    self.raise_interrupt(InterruptSource::Pi);
                    let notification = DeviceNotification::PiDmaComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::Ai { token } => {
                    let Some(current) = self.current_ai else {
                        continue;
                    };
                    if current.token != token {
                        continue;
                    }
                    let full_before_completion = self.queued_ai.is_some();
                    self.current_ai = None;
                    self.record(DeviceTraceKind::AiDmaComplete(current.request));
                    if self.queued_ai.take().is_some() {
                        self.commit_ai_dma(prepared_ai_promotion.expect(
                            "queued AI promotion was preflighted before event-state mutation",
                        ));
                    }
                    // Public rcp.h defines FIFO FULL transitioning 1 -> 0 as
                    // an AI interrupt edge. Other silicon assertion causes
                    // and the sub-cycle phase remain unclaimed.
                    if full_before_completion {
                        self.raise_interrupt(InterruptSource::Ai);
                        let notification = DeviceNotification::AiDmaComplete(current.request);
                        notifications.push(notification);
                        self.record(DeviceTraceKind::NotificationReady(notification));
                    }
                }
                DeviceEvent::Si { token } => {
                    let Some(pending) = self.pending_si else {
                        continue;
                    };
                    if pending.token != token {
                        continue;
                    }
                    let request = pending.request;
                    match request.kind {
                        SiDmaKind::DramToPif => {
                            let bytes =
                                rdram.dma_read_bytes_flat(request.dram_addr.offset() as usize, 64);
                            self.pif_ram.copy_from_slice(&bytes);
                            execute_pif(self.now, &mut self.pif_ram, &mut self.pi_dma);
                        }
                        SiDmaKind::PifToDram => {
                            {
                                static PROBE: std::sync::OnceLock<bool> =
                                    std::sync::OnceLock::new();
                                if *PROBE
                                    .get_or_init(|| std::env::var_os("FN64_BOOT_PROBE").is_some())
                                {
                                    eprintln!(
                                        "[boot-probe] PifToDram response: {:02x?}",
                                        self.pif_ram
                                    );
                                }
                            }
                            rdram.dma_write_bytes(
                                crate::rom::DmaWriterChannel::Si,
                                request.dram_addr.offset() as usize,
                                &self.pif_ram,
                            )
                        }
                        SiDmaKind::ControllerQuery | SiDmaKind::ControllerRead => {
                            execute_pif(self.now, &mut self.pif_ram, &mut self.pi_dma);
                        }
                    }
                    self.record(DeviceTraceKind::SiBytesCommitted(request));
                    self.pending_si = None;
                    self.record(DeviceTraceKind::SiBusyCleared);
                    self.raise_interrupt(InterruptSource::Si);
                    let notification = DeviceNotification::SiDmaComplete(request);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::SpDma { token } => {
                    let Some(active) = self.active_sp_dma else {
                        continue;
                    };
                    if active.token != token {
                        continue;
                    }
                    let request = active.request;
                    let line_len = request.line_len();
                    let row_stride = line_len + request.skip();
                    match request.direction {
                        SpDmaDirection::RdramToRsp => {
                            let mut bytes = Vec::with_capacity(request.total_bytes());
                            for row in 0..request.line_count() {
                                let offset = request.dram_addr.offset() as usize + row * row_stride;
                                bytes.extend(rdram.dma_read_bytes_flat(offset, line_len));
                            }
                            self.rsp_memory
                                .write_bytes(request.mem_addr, &bytes)
                                .map_err(DeviceFault::SpDmaMemory)?;
                        }
                        SpDmaDirection::RspToRdram => {
                            let bytes = self
                                .rsp_memory
                                .read_bytes(request.mem_addr, request.total_bytes())
                                .map_err(DeviceFault::SpDmaMemory)?;
                            for (row, line) in bytes.chunks_exact(line_len).enumerate() {
                                let offset = request.dram_addr.offset() as usize + row * row_stride;
                                rdram.dma_write_bytes(
                                    crate::rom::DmaWriterChannel::Sp,
                                    offset,
                                    line,
                                );
                            }
                        }
                    }
                    self.record(DeviceTraceKind::SpDmaBytesCommitted(request));
                    self.active_sp_dma = None;
                    if let Some(next) = self.queued_sp_dma.take() {
                        // The public guide requires a pending request to begin
                        // before DMA_BUSY clears. Starting it in this same
                        // ordered event transition makes that intermediate
                        // false-busy state unobservable to the guest.
                        self.begin_sp_dma(next)?;
                    } else {
                        self.record(DeviceTraceKind::SpDmaBusyCleared);
                    }
                }
                DeviceEvent::Vi { token } => {
                    if self.pending_vi != Some(token) {
                        continue;
                    }
                    self.pending_vi = None;
                    self.record(DeviceTraceKind::ViInterrupt);
                    self.raise_interrupt(InterruptSource::Vi);
                    let notification = DeviceNotification::ViRetrace { at: self.now };
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                    self.reschedule_vi_interrupt()?;
                }
                DeviceEvent::Sp { token } => {
                    if self.pending_sp != Some(token) {
                        continue;
                    }
                    self.pending_sp = None;
                    self.sp_status |= SP_STATUS_HALT | SP_STATUS_BROKE;
                    let completion = RcpTaskCompletion::Sp;
                    self.record(DeviceTraceKind::RcpTaskComplete(completion));
                    self.raise_interrupt(InterruptSource::Sp);
                    let notification = DeviceNotification::RcpTaskComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::Dp { token } => {
                    if self.pending_dp != Some(token) {
                        continue;
                    }
                    self.pending_dp = None;
                    let completion = RcpTaskCompletion::Dp;
                    self.record(DeviceTraceKind::RcpTaskComplete(completion));
                    self.raise_interrupt(InterruptSource::Dp);
                    let notification = DeviceNotification::RcpTaskComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
            }
        }
        self.now = requested;
        self.pi_dma.advance_eeprom_to(self.now);
        Ok(notifications)
    }

    pub(crate) fn validate_mmio(&self, addr: MmioAddr) -> Result<(), DeviceFault> {
        if addr.is_word_aligned() {
            Ok(())
        } else {
            Err(DeviceFault::UnalignedMmio { addr })
        }
    }

    pub(crate) fn record(&mut self, kind: DeviceTraceKind) {
        self.trace_summary.record(kind);
        let sequence = self.next_trace_sequence;
        self.next_trace_sequence = self
            .next_trace_sequence
            .checked_add(1)
            .expect("device trace sequence overflow");
        if self.trace_enabled {
            self.trace.push(DeviceTraceEvent {
                at: self.now,
                sequence,
                kind,
            });
        }
    }

    /// Nonmutating preparation for [`ReadyDpcFabricCommit::commit`]. This is
    /// the single place that proves readiness by construction: every fallible
    /// check `commit_dpc_submission`/`cancel_dpc_submission` would perform,
    /// plus the v11 all-readiness list -- MINUS one item deliberately
    /// dropped after tracing a real false-positive it caused, see below --
    /// run here against a read-only view before any register is touched:
    ///
    /// 1. pending submission exists, and its token matches;
    /// 2. source-aware `validate_dpc_range` (8-byte-aligned, nonempty START <
    ///    END, and within the source's address space -- the 24-bit RDP bus
    ///    for RDRAM, the 4 KiB RSP DMEM bank for DMEM);
    /// 3. live CURRENT correspondence: `self.dpc.current` still equals the
    ///    submission's START;
    /// 4. live END correspondence: `self.dpc.end` still equals the
    ///    submission's END;
    /// 5. required status: `DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY |
    ///    DPC_STATUS_CMD_BUSY` are all set in `self.dpc.status`;
    /// 6. complete rollback consistency: the rollback image's own `start <=
    ///    end` and `current` within `[start, end]`.
    ///
    /// Checks 3-6 above are each individually reachability-audited, not
    /// assumed unreachable as a group:
    ///
    /// - CURRENT (3) is unreachable: `DPC_CURRENT_REG` MMIO writes always
    ///   return `UnmodeledMmioWrite`, so nothing can move it while a
    ///   submission is pending except `commit`/`Drop` themselves, which take
    ///   `pending_dpc` to `None` in the same step.
    /// - END (4) is unreachable: `DPC_END_REG`/`DPC_START_REG` writes reject
    ///   with `DeviceFault::DpBusy` whenever `self.pending_dpc.is_some()`.
    /// - Required status (5) is unreachable: `begin_dpc_submission` sets
    ///   `END_VALID | DMA_BUSY | CMD_BUSY` at admission; the only other
    ///   writers of `self.dpc.status` are `commit`/`Drop` (which clear or
    ///   overwrite them as part of the same take-to-`None` step) and
    ///   `write_mmio(DPC_STATUS_REG, ..)`'s mode-command bits (0-5), which
    ///   never touch bits 6/8/9.
    /// - Rollback consistency (6) is unreachable: `apply_dpc_status_mode_commands`
    ///   (the only thing `write_mmio(DPC_STATUS_REG, ..)` can do to a pending
    ///   submission's mirrored `pending.rollback`, see that write arm's own
    ///   interleaving comment) only ever touches `rollback.status`, never
    ///   `rollback.start`/`end`/`current`.
    ///
    /// **A fifth check -- source/XBUS status correspondence,
    /// `self.dpc.status`'s `DPC_STATUS_XBUS_DMEM_DMA` bit matching the
    /// submission's source -- was in an earlier version of this method and
    /// has been removed.** It was NOT unreachable: `write_mmio(DPC_STATUS_REG,
    /// ..)` legitimately sets/clears that exact bit while a submission is
    /// pending (a real guest STATUS mode command interleaved with an
    /// in-flight renderer transaction), and `cancel_dpc_submission` is
    /// separately, deliberately designed to preserve -- not discard -- that
    /// later command through cancellation rather than treat it as corruption
    /// (proven by `dpc_status_mode_commands_during_renderer_admission_survive_cancellation`
    /// in `device/tests/device_b.rs`). `commit_dpc_submission`'s own body
    /// never reads the XBUS bit at all, so the removed check was never a
    /// genuine precondition of a correct commit -- it was a false positive
    /// that would have rejected exactly the interleaving cancellation is
    /// built to tolerate.
    ///
    /// This generalizes beyond just the XBUS bit, but only as far as the code
    /// actually supports -- two DISTINCT mechanisms inside the
    /// `DPC_STATUS_REG` write arm, not one:
    ///
    /// (a) **Three mode STATUS bits.** `apply_dpc_status_mode_commands`
    ///     applies exactly the XBUS/FREEZE/FLUSH clear/set bit pairs to
    ///     `self.dpc.status`, and -- if a submission is pending -- mirrors
    ///     the same command into that submission's `pending.rollback.status`
    ///     (see the write arm's own "Interleaving closed" comment). This
    ///     mirroring is what makes `cancel_dpc_submission`'s later
    ///     `self.dpc.status = pending.rollback.status` restore the
    ///     admission-era mode bits while still carrying forward whatever the
    ///     guest changed afterward -- an overwrite from an already-updated
    ///     source, not a partial bit-clear.
    ///
    /// (b) **Four separate counter registers.** `tmem_busy`/`pipe_busy`/`busy`/
    ///     `clock` are their own `DpcCounter24` fields on `DpcRegisters` --
    ///     NOT part of `status`. The four `DPC_STATUS_CLEAR_*_COUNTER_COMMAND`
    ///     bits are read directly off the same STATUS write value, but
    ///     handled by separate `if` arms that zero those fields straight
    ///     (`self.dpc.tmem_busy = DpcCounter24::ZERO`, etc.) -- entirely
    ///     outside `apply_dpc_status_mode_commands`, and with no rollback
    ///     mirroring at all. A counter cleared while a submission is pending
    ///     stays cleared through BOTH commit and cancel; neither restores it
    ///     (see `cancel_dpc_submission`'s own comment: "the four performance
    ///     counters and any counter-clear issued during admission are NOT
    ///     rolled back").
    ///
    /// Neither the three mode bits nor the four counters are read by
    /// `commit_dpc_submission`, so none of the seven is a readiness
    /// precondition here: a STATUS write that changes XBUS/FREEZE/FLUSH or
    /// clears a counter while this submission is pending is an intended,
    /// supported interleaving, not a fault to detect. `ReadyDpcFabricCommit::commit`
    /// and `Drop` each preserve it, but by DIFFERENT means, matching
    /// `commit_dpc_submission`/`cancel_dpc_submission` exactly:
    ///
    /// - `commit`'s `status &= !(END_VALID | DMA_BUSY | CMD_BUSY)` clears
    ///   only those three admission-owned bits and leaves every mode bit and
    ///   every counter exactly as the guest last set them (a targeted clear,
    ///   not a restore).
    /// - `Drop`'s `status = self.rollback.status` is a full overwrite from
    ///   the already-mirrored rollback image, which is how a live mode-bit
    ///   change survives cancellation even though the write is a whole-word
    ///   assignment, not a bit-clear -- the counters are untouched by this
    ///   line either way, since they live outside `status` entirely.
    ///
    /// See `prepare_and_commit_survive_an_interleaved_xbus_mode_command`
    /// (this module) and its ABI-level companion
    /// `with_ready_commit_succeeds_through_a_real_interleaved_xbus_mode_command`
    /// (`fn64-abi`'s `render_ir_integration.rs`) for the commit-path proof of
    /// mechanism (a); this doc comment does not claim a colocated test for
    /// mechanism (b), since (b) was never checked by the removed readiness
    /// check in the first place. This pair of tests freezes the bug class
    /// (a) exercises (a readiness check stricter than what commit actually
    /// needs) against recurrence.
    ///
    /// Checks 1-2 above ARE reachable in production (a genuinely wrong token,
    /// or -- unreachable through the real admission path but exercised via
    /// hand-corrupted fixtures in this module's tests -- an inconsistent
    /// range); checks 3-6 exist so this method is a COMPLETE proof rather
    /// than trusting an invariant it happens to be able to see holds, even
    /// though nothing on this crate's single-thread executor can currently
    /// make them fire outside a test.
    ///
    /// Every rejection restores the exact owned `pending` value to
    /// `self.pending_dpc` before returning `Err`, so a caller sees no
    /// observable difference from a design that only ever reads through
    /// `self.pending_dpc` -- the outer cancel guard (in ABI,
    /// `LiveDpcTransaction::drop`) can still cleanly cancel that exact same
    /// slot afterward. Only on success does this leave `self.pending_dpc` as
    /// `None`, exactly where a successful commit or cancel would eventually
    /// leave it. `ReadyDpcFabricCommit<'_>` borrows only `self.dpc` (a direct
    /// `&mut DpcRegisters`, not wrapped in `Option`) and `self.pending_dpc`
    /// (a direct `&mut Option<PendingDpc>`, already `None`) -- two disjoint
    /// fields, not the whole fabric, so the type remains concrete, not
    /// generic over `R`/`T`, and nameable at a crate boundary that cannot
    /// name `DeviceFabric<R, T>` itself (`fn64-render`'s sealed commit
    /// capsule is one such boundary; see the type's own doc comment). Every
    /// other field of `self` (RSP/SI/PI/AI/VI/etc.) stays unborrowed, so a
    /// caller may continue using them through `self` while a
    /// `ReadyDpcFabricCommit` is alive.
    pub fn prepare_dpc_commit(
        &mut self,
        token: u64,
    ) -> Result<ReadyDpcFabricCommit<'_>, DeviceFault> {
        // Take ownership up front -- `PendingDpc` is `Copy`, so this costs
        // nothing -- and validate the OWNED value against a read-only
        // snapshot of `self.dpc`. Any rejection puts `pending` straight back
        // before returning `Err`.
        let Some(pending) = self.pending_dpc.take() else {
            return Err(DeviceFault::NoPendingDpcSubmission);
        };
        let submission = pending.submission;
        if submission.token != token {
            self.pending_dpc = Some(pending);
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: token,
            });
        }
        if Self::validate_dpc_range(submission.source, submission.start, submission.end).is_err() {
            self.pending_dpc = Some(pending);
            return Err(DeviceFault::InvalidDpcRange {
                source: submission.source,
                start: submission.start,
                end: submission.end,
            });
        }
        let live = self.dpc;
        let required_status = DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY;
        // CURRENT and END are checked against the admitted submission because
        // neither is legitimately writable while a DPC submission is pending
        // (`DPC_CURRENT_REG` writes are always `UnmodeledMmioWrite`;
        // `DPC_START_REG`/`DPC_END_REG` writes reject with `DpBusy` whenever
        // `pending_dpc.is_some()`) -- any live disagreement can only mean
        // stale/foreign state, not an intended guest interleaving.
        //
        // Two things `write_mmio(DPC_STATUS_REG, ..)` can legitimately change
        // while a submission is pending are deliberately NOT checked here,
        // even though `request_dpc_submission` sets/clears the XBUS bit to
        // match `source` at admission:
        //
        // - The DPC_STATUS_XBUS_DMEM_DMA/FREEZE/FLUSH mode bit pairs, part of
        //   `self.dpc.status` itself, applied by `apply_dpc_status_mode_commands`
        //   and mirrored into `pending.rollback.status` by the same write arm.
        // - The four counter registers (`self.dpc.tmem_busy`/`pipe_busy`/`busy`/
        //   `clock`) -- separate `DpcCounter24` fields, NOT part of
        //   `status` -- which the DPC_STATUS_CLEAR_*_COUNTER_COMMAND bits
        //   zero directly and which are never mirrored to any rollback.
        //
        // A raw STATUS command can legitimately change either while a
        // submission is pending, and `cancel_dpc_submission`'s rollback is
        // specifically designed to preserve -- not reject or discard -- that
        // later command rather than treat it as corruption (see
        // `dpc_status_mode_commands_during_renderer_admission_survive_cancellation`
        // in `device/tests/device_b.rs`, and the interleaving comment on the
        // `DPC_STATUS_REG` write arm above). `commit_dpc_submission` itself
        // never reads any of those bits/counters either, so none of them was
        // ever a genuine precondition of a correct commit -- only
        // CURRENT/END/required-status (the three bits `commit` itself
        // clears) are.
        let live_matches_admission = live.current == submission.start
            && live.end == submission.end
            && (live.status & required_status) == required_status;
        if !live_matches_admission {
            self.pending_dpc = Some(pending);
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: token,
            });
        }
        let rollback = pending.rollback;
        if rollback.start > rollback.end
            || rollback.current < rollback.start
            || rollback.current > rollback.end
        {
            self.pending_dpc = Some(pending);
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: token,
            });
        }
        // Every check passed against the owned `pending`; `self.pending_dpc`
        // is already `None` from the `take()` above, exactly where a
        // successful commit or cancel would eventually leave it.
        Ok(ReadyDpcFabricCommit {
            dpc: &mut self.dpc,
            pending_dpc: &mut self.pending_dpc,
            pending,
            end: submission.end,
            rollback,
            armed: true,
        })
    }
}

/// Proof that one DPC submission's commit preconditions already hold,
/// produced only by [`DeviceFabric::prepare_dpc_commit`].
///
/// Concrete and non-generic: it borrows only the two disjoint fields
/// `commit`/`cancel` touch directly -- `&'a mut DpcRegisters` and `&'a mut
/// Option<PendingDpc>` (no `Option<&mut _>` wrapper around either: both are
/// unconditional borrows, since `prepare_dpc_commit` only ever constructs
/// this value once it already holds them) -- both plain `pub(crate)` data
/// types with no `R`/`T` type parameter of their own. This is what lets the
/// type be named across a crate boundary that cannot name `DeviceFabric<R,
/// T>` -- `fn64-render`'s sealed commit capsule
/// (`ReadyRawDpcCommitCapsule`/`ReadyRawDpcBackendCommitParts` in the
/// accepted v11 migration card) is declared against a concrete
/// lifetime-bearing type, not a renderer-agnostic crate's own generic
/// parameter. The two borrows are disjoint fields of the same
/// `DeviceFabric`, so holding this value does not prevent a caller from
/// concurrently using every other field through the original `&mut
/// DeviceFabric` (RSP/SI/PI/AI/VI/etc, and in ABI's case, `RENDER_BACKEND`,
/// which is a wholly separate `RefCell` and was never reachable through
/// `DeviceFabric` in the first place).
///
/// `pending`/`end`/`rollback` are the exact facts
/// [`DeviceFabric::prepare_dpc_commit`] already validated and took ownership
/// of; `armed` is the single bit of state that decides whether `Drop`
/// performs a register write. There is no `Option<&mut _>`, `unwrap`,
/// `expect`, `assert`, or `if let` anywhere in `commit`/`Drop`. `commit` is
/// unconditional -- one straight-line sequence of field assignments, then
/// `armed = false`, no branch at all. `Drop` has exactly one branch, `if
/// !self.armed { return; }`, gating its own straight-line rollback
/// assignments; `armed` is written twice in this value's lifetime (`true` at
/// construction, `false` at the end of `commit`) and read exactly once (that
/// one `Drop` check) -- never re-read after being cleared, so there is no
/// possible silent no-op branch representing an already-committed or
/// already-cancelled value as anything other than exactly that.
///
/// This value has no public constructor and no field access. Its only
/// production transitions are the infallible, consuming [`Self::commit`]
/// (advances CURRENT) and its `Drop` impl (rolls back exactly once for any
/// value that reaches scope exit still armed, covering an ordinary early
/// return and a panic unwind alike). A `#[cfg(test)]` hostile constructor
/// exists solely to prove a foreign token's mismatched facts are rejected
/// before `prepare_dpc_commit` ever runs, not to reach a panic branch inside
/// `commit`/`Drop` -- there is none to reach; see `fabric_ops` tests.
#[must_use = "an unconsumed ReadyDpcFabricCommit cancels its DPC submission on drop"]
pub struct ReadyDpcFabricCommit<'a> {
    dpc: &'a mut DpcRegisters,
    pending_dpc: &'a mut Option<PendingDpc>,
    pending: PendingDpc,
    end: u32,
    rollback: DpcRegisters,
    armed: bool,
}

impl core::fmt::Debug for ReadyDpcFabricCommit<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ReadyDpcFabricCommit")
            .field("end", &self.end)
            .field("armed", &self.armed)
            .finish_non_exhaustive()
    }
}

impl ReadyDpcFabricCommit<'_> {
    pub const fn token(&self) -> u64 {
        self.pending.submission.token
    }

    /// Sole infallible final transition: one straight-line sequence of field
    /// assignments, then `armed = false`. No branch that can fail, and no
    /// branch on `armed` itself -- `commit` and `Drop` are the only two
    /// places `armed` is read, `commit` always runs its writes (it is the
    /// caller's job to call this at most once, which `self` being consumed
    /// already guarantees), and `Drop` checks `armed` exactly once to decide
    /// whether it, not `commit`, is the one performing a write.
    pub fn commit(mut self) {
        self.dpc.current = self.end;
        self.dpc.status &= !(DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY);
        *self.pending_dpc = None;
        self.armed = false;
    }
}

impl Drop for ReadyDpcFabricCommit<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Same admission-owned-registers-only rollback as
        // `cancel_dpc_submission`; see that method's doc comment for why the
        // four performance counters are deliberately excluded. One
        // straight-line sequence of field assignments, same shape as
        // `commit`'s -- the only difference is which values they write.
        self.dpc.start = self.rollback.start;
        self.dpc.end = self.rollback.end;
        self.dpc.current = self.rollback.current;
        self.dpc.status = self.rollback.status;
        *self.pending_dpc = None;
    }
}

#[cfg(test)]
impl<'a> ReadyDpcFabricCommit<'a> {
    /// Hostile construction bypassing `prepare_dpc_commit`'s validation
    /// entirely, used only to prove a caller cannot fabricate a
    /// `ReadyDpcFabricCommit` whose `end`/`rollback`/`pending` disagree with
    /// the real pending submission and have it silently accepted -- there is
    /// no runtime check left inside `commit`/`Drop` to catch that anymore (by
    /// design: readiness is now proved once, at `prepare_dpc_commit`), so
    /// this test instead proves the MISUSE surface is inert: a hostile
    /// caller can drive `dpc`/`pending_dpc` to whatever bytes it supplies,
    /// but it cannot do so through any production entry point, because this
    /// whole `impl` is `#[cfg(test)]` and every field of
    /// `ReadyDpcFabricCommit` stays private outside it.
    pub(crate) fn new_hostile_for_test(
        dpc: &'a mut DpcRegisters,
        pending_dpc: &'a mut Option<PendingDpc>,
        pending: PendingDpc,
        end: u32,
        rollback: DpcRegisters,
    ) -> Self {
        Self {
            dpc,
            pending_dpc,
            pending,
            end,
            rollback,
            armed: true,
        }
    }
}

#[cfg(test)]
mod ready_dpc_fabric_commit_tests {
    use super::*;
    use crate::rom::InMemoryRom;

    fn fabric() -> DeviceFabric<InMemoryRom, FixedPiTiming> {
        DeviceFabric::new(
            PiDma::new(InMemoryRom::new(Vec::new())),
            FixedPiTiming(Cycles::new(0)),
        )
    }

    fn submitted(fabric: &mut DeviceFabric<InMemoryRom, FixedPiTiming>) -> DpcSubmission {
        fabric
            .request_dpc_submission(DpcSubmissionSource::Rdram, 0x100, 0x180)
            .unwrap()
            .expect("fresh fabric is never frozen")
    }

    #[test]
    fn commit_advances_current_exactly_where_commit_dpc_submission_would() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        let ready = fabric.prepare_dpc_commit(submission.token).unwrap();
        ready.commit();
        assert_eq!(fabric.rsp_execution_state().dpc_current, 0x180);
        assert!(fabric.pending_dpc_submission().is_none());
    }

    #[test]
    fn dropping_unconsumed_ready_commit_cancels_without_mutation() {
        let mut fabric = fabric();
        let before_admission = fabric.rsp_execution_state();
        let submission = submitted(&mut fabric);
        assert_ne!(
            fabric.rsp_execution_state(),
            before_admission,
            "admission itself must mutate DPC registers, or this test proves nothing"
        );
        let ready = fabric.prepare_dpc_commit(submission.token).unwrap();
        drop(ready);
        // Drop-cancel rolls back exactly the admission-owned registers, same
        // as a direct `cancel_dpc_submission` would.
        assert_eq!(fabric.rsp_execution_state(), before_admission);
        assert!(fabric.pending_dpc_submission().is_none());
        // A rejected token is never retried: the pending submission is gone,
        // so a second prepare against the same token finds nothing pending.
        assert_eq!(
            fabric.prepare_dpc_commit(submission.token).unwrap_err(),
            DeviceFault::NoPendingDpcSubmission
        );
    }

    #[test]
    fn stale_token_is_rejected_before_any_mutation() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        let before = fabric.rsp_execution_state();
        assert_eq!(
            fabric
                .prepare_dpc_commit(submission.token.wrapping_add(1))
                .unwrap_err(),
            DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: submission.token.wrapping_add(1),
            }
        );
        assert_eq!(fabric.rsp_execution_state(), before);
    }

    #[test]
    fn no_pending_submission_is_rejected_before_any_mutation() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.prepare_dpc_commit(1).unwrap_err(),
            DeviceFault::NoPendingDpcSubmission
        );
    }

    #[test]
    fn stale_token_is_rejected_before_any_mutation_via_prepare_not_commit() {
        // Companion to `stale_token_is_rejected_before_any_mutation` above,
        // stated as a negative: after the redesign that moves every check
        // into `prepare_dpc_commit`, `commit`/`Drop` have NO panic branch
        // left to reach for a wrong token. `ReadyDpcFabricCommit` DOES store
        // the admitted submission's own token (inside `pending`, exposed
        // read-only by `Self::token`) -- what it never does is accept or
        // compare against an EXTERNALLY supplied token: neither `commit` nor
        // `Drop` takes a token parameter or validates one against anything,
        // so there is no wrong-token failure branch inside either. The only
        // place "wrong token" can be observed is `prepare_dpc_commit`'s
        // `Result`, which the earlier test already covers exhaustively. This
        // test exists so the invariant is named explicitly rather than only
        // implied by `ReadyDpcFabricCommit`'s field list.
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        assert!(fabric
            .prepare_dpc_commit(submission.token.wrapping_add(1))
            .is_err());
        // The one and only READY value producible from this fabric right now
        // is against the real token, and using it does not panic.
        fabric
            .prepare_dpc_commit(submission.token)
            .unwrap()
            .commit();
    }

    #[test]
    fn prepare_rejects_dmem_end_outside_the_4kib_bank() {
        // DMEM's `validate_dpc_range` upper bound is `RSP_MEMORY_BANK_SIZE`,
        // not the 24-bit RDP bus RDRAM uses -- this is the source-aware half
        // of `validate_dpc_range` that
        // `prepare_rejects_end_outside_the_24_bit_rdp_address_space` (below)
        // does not exercise.
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        let before = fabric.rsp_execution_state();
        fabric.pending_dpc = Some(PendingDpc {
            submission: DpcSubmission {
                source: DpcSubmissionSource::Dmem,
                start: 0,
                end: RSP_MEMORY_BANK_SIZE as u32 + 8,
                ..submission
            },
            rollback: fabric.pending_dpc.unwrap().rollback,
        });
        let corrupted_pending = fabric.pending_dpc;
        assert!(matches!(
            fabric.prepare_dpc_commit(submission.token),
            Err(DeviceFault::InvalidDpcRange { .. })
        ));
        assert_eq!(fabric.rsp_execution_state(), before);
        assert_eq!(fabric.pending_dpc, corrupted_pending);
    }

    #[test]
    fn prepare_rejects_unaligned_or_empty_range() {
        for (start, end) in [
            (0x101, 0x180), // START not 8-byte aligned
            (0x100, 0x179), // END not 8-byte aligned
            (0x180, 0x180), // empty (START == END)
            (0x188, 0x180), // reversed (START > END)
        ] {
            let mut fabric = fabric();
            let submission = submitted(&mut fabric);
            let before = fabric.rsp_execution_state();
            fabric.pending_dpc = Some(PendingDpc {
                submission: DpcSubmission {
                    start,
                    end,
                    ..submission
                },
                rollback: fabric.pending_dpc.unwrap().rollback,
            });
            let corrupted_pending = fabric.pending_dpc;
            assert!(
                matches!(
                    fabric.prepare_dpc_commit(submission.token),
                    Err(DeviceFault::InvalidDpcRange { .. })
                ),
                "start={start:#x} end={end:#x} must be rejected"
            );
            assert_eq!(fabric.rsp_execution_state(), before);
            assert_eq!(fabric.pending_dpc, corrupted_pending);
        }
    }

    #[test]
    fn prepare_rejects_live_current_not_matching_submission_start() {
        // `self.dpc.current` is set to `submission.start` at admission and
        // advances only inside `commit`/`Drop` (which take `pending_dpc` to
        // `None` in the same step) -- so on a real fabric this can never
        // disagree with the still-pending submission's START. Corrupting
        // `self.dpc.current` directly (not reachable from any public API)
        // proves the correspondence check exists and fires.
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        fabric.dpc.current = submission.start + 8;
        let before = fabric.rsp_execution_state();
        let pending_before_prepare = fabric.pending_dpc;
        assert_eq!(
            fabric.prepare_dpc_commit(submission.token).unwrap_err(),
            DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: submission.token,
            }
        );
        assert_eq!(fabric.rsp_execution_state(), before);
        assert_eq!(fabric.pending_dpc, pending_before_prepare);
    }

    #[test]
    fn prepare_rejects_live_end_not_matching_submission_end() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        fabric.dpc.end = submission.end + 8;
        let before = fabric.rsp_execution_state();
        let pending_before_prepare = fabric.pending_dpc;
        assert_eq!(
            fabric.prepare_dpc_commit(submission.token).unwrap_err(),
            DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: submission.token,
            }
        );
        assert_eq!(fabric.rsp_execution_state(), before);
        assert_eq!(fabric.pending_dpc, pending_before_prepare);
    }

    #[test]
    fn prepare_and_commit_survive_an_interleaved_xbus_mode_command() {
        // Companion to `dpc_status_mode_commands_during_renderer_admission_
        // survive_cancellation` (device/tests/device_b.rs), which proves the
        // same interleaving survives CANCELLATION. This proves it survives
        // the COMMIT path instead: `prepare_dpc_commit` must not treat a
        // real, publicly-reachable `write_mmio(DPC_STATUS_REG, ..)` mode
        // command -- issued by the guest CPU after RDRAM admission, before
        // the renderer's `prepare_dpc_commit` call -- as stale/corrupted
        // state. `commit_dpc_submission`'s own body never reads the XBUS bit,
        // so there was never a genuine invariant here for `prepare_dpc_commit`
        // to enforce; the earlier design that DID reject this exact scenario
        // was a real bug, not a hardening measure.
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        assert_eq!(submission.source, DpcSubmissionSource::Rdram);
        assert_eq!(fabric.dpc.status & DPC_STATUS_XBUS_DMEM_DMA, 0);

        // Command `0x02` sets DPC_STATUS_XBUS_DMEM_DMA (see
        // `apply_dpc_status_mode_commands`'s clear/set bit-pair encoding: bit
        // 0 clears, bit 1 sets). This is the exact public MMIO write a guest
        // issues, not a hand-corrupted fixture.
        let _ = fabric.write_mmio(DPC_STATUS_REG, 0x02).unwrap();
        assert_eq!(
            fabric.dpc.status & DPC_STATUS_XBUS_DMEM_DMA,
            DPC_STATUS_XBUS_DMEM_DMA,
            "the interleaved mode command must have taken effect"
        );

        let ready = fabric
            .prepare_dpc_commit(submission.token)
            .expect("an interleaved XBUS mode command must not make prepare_dpc_commit reject");
        ready.commit();

        // The interleaved command's effect on STATUS is preserved through
        // commit, same as the cited cancellation test preserves it through
        // cancellation -- this is one command, checked from both terminal
        // outcomes now.
        assert_eq!(
            fabric.dpc.status & DPC_STATUS_XBUS_DMEM_DMA,
            DPC_STATUS_XBUS_DMEM_DMA,
            "commit must not silently revert the interleaved mode command"
        );
        assert_eq!(fabric.dpc.current, submission.end);
        assert!(fabric.pending_dpc_submission().is_none());
    }

    #[test]
    fn prepare_rejects_missing_required_status_bits() {
        // `begin_dpc_submission` sets END_VALID | DMA_BUSY | CMD_BUSY at
        // admission; nothing else clears them before `prepare_dpc_commit`.
        // Clear each individually, hand-corrupting `self.dpc.status` directly
        // (not reachable from any public API), and confirm each alone is
        // sufficient to reject.
        for bit in [
            DPC_STATUS_END_VALID,
            DPC_STATUS_DMA_BUSY,
            DPC_STATUS_CMD_BUSY,
        ] {
            let mut fabric = fabric();
            let submission = submitted(&mut fabric);
            fabric.dpc.status &= !bit;
            let before = fabric.rsp_execution_state();
            let pending_before_prepare = fabric.pending_dpc;
            assert_eq!(
                fabric.prepare_dpc_commit(submission.token).unwrap_err(),
                DeviceFault::StaleDpcSubmission {
                    pending_token: submission.token,
                    received_token: submission.token,
                },
                "clearing status bit {bit:#x} alone must be rejected"
            );
            assert_eq!(fabric.rsp_execution_state(), before);
            assert_eq!(fabric.pending_dpc, pending_before_prepare);
        }
    }

    #[test]
    fn prepare_rejects_end_outside_the_24_bit_rdp_address_space() {
        // `request_dpc_submission`/`begin_dpc_submission` already enforce this
        // at admission (`validate_dpc_range`), so this path is unreachable on
        // any real fabric; the check exists so `prepare_dpc_commit` is a
        // complete proof rather than trusting a caller-controlled invariant.
        // Exercised here directly against a hand-built `PendingDpc`, since a
        // real fabric can never admit an out-of-range END to begin with.
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        fabric.pending_dpc = Some(PendingDpc {
            submission: DpcSubmission {
                end: 0x0100_0008,
                ..submission
            },
            rollback: fabric.pending_dpc.unwrap().rollback,
        });
        assert!(matches!(
            fabric.prepare_dpc_commit(submission.token),
            Err(DeviceFault::InvalidDpcRange { .. })
        ));
    }

    #[test]
    fn prepare_rejects_end_not_multiple_of_eight() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        fabric.pending_dpc = Some(PendingDpc {
            submission: DpcSubmission {
                end: submission.end + 1,
                ..submission
            },
            rollback: fabric.pending_dpc.unwrap().rollback,
        });
        assert!(matches!(
            fabric.prepare_dpc_commit(submission.token),
            Err(DeviceFault::InvalidDpcRange { .. })
        ));
    }

    #[test]
    fn prepare_rejects_an_inconsistent_rollback_image_and_leaves_it_cancellable() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        let mut inconsistent = fabric.pending_dpc.unwrap();
        // `current` outside `[start, end]` can never occur on a real fabric
        // (the rollback image is a snapshot of the exact pre-admission DPC
        // registers), so this is a hand-corrupted fixture proving the check
        // exists and fires, not a reachable production state.
        inconsistent.rollback.current = inconsistent.rollback.end + 8;
        fabric.pending_dpc = Some(inconsistent);
        // Captured AFTER the hand-corruption above and BEFORE the rejected
        // `prepare_dpc_commit` call: this is the exact live-register image
        // and exact full `PendingDpc` (including the corrupted `rollback`,
        // not just the `DpcSubmission` half) the rejection must leave
        // completely untouched.
        let live_before_prepare = fabric.dpc;
        let full_pending_before_prepare = fabric.pending_dpc;
        assert_eq!(
            fabric.prepare_dpc_commit(submission.token).unwrap_err(),
            DeviceFault::StaleDpcSubmission {
                pending_token: submission.token,
                received_token: submission.token,
            }
        );
        // Immediately across the rejected call, BEFORE any cancel runs:
        // exact `DpcRegisters` unchanged, and the exact FULL `PendingDpc`
        // (submission AND rollback, not just the submission half a looser
        // `pending_dpc_submission()` comparison would check) restored
        // byte-for-byte, including the still-corrupted `rollback` this test
        // put there -- `prepare_dpc_commit` must put back exactly what it
        // took, not a repaired or partial copy.
        assert_eq!(fabric.dpc, live_before_prepare);
        assert_eq!(fabric.pending_dpc, full_pending_before_prepare);
        // This is the class of `prepare_dpc_commit` rejection where `token`
        // still legitimately owns the pending slot (unlike
        // `NoPendingDpcSubmission`/token-mismatched `StaleDpcSubmission`,
        // where nothing valid is left to cancel either). Only now, after the
        // exact-unchanged assertions above, does cancellation run -- proven
        // directly here via `cancel_dpc_submission`, the same call
        // `LiveDpcTransaction::drop` makes.
        assert!(fabric.cancel_dpc_submission(submission.token).is_ok());
        assert!(fabric.pending_dpc_submission().is_none());
    }

    #[test]
    fn commit_is_unconditional_and_drop_is_gated_only_by_armed_no_panic_possible() {
        // Once a `ReadyDpcFabricCommit` exists, neither `commit` nor `Drop`
        // contains an `expect`/`assert`/`panic!` reachable from ANY field
        // state. `commit` performs unconditional fixed writes (no branch at
        // all); `Drop` has exactly one branch -- `if !self.armed { return; }`
        // -- gating its own fixed rollback writes. Neither holds its state
        // behind `Option<&mut _>`, and neither calls `Option::take` on
        // anything: both borrow `self.dpc`/`self.pending_dpc` directly. The
        // `#[cfg(test)]` hostile constructor lets the no-panic claim be
        // demonstrated directly: even a `ReadyDpcFabricCommit` built from
        // completely disagreeing `end`/`rollback` values still runs
        // `commit`/`Drop` to completion without panicking (it just writes
        // exactly the bytes it was told to, which is the whole point -- the
        // type no longer second-guesses its own fields at the write site).
        // What keeps this safe in production is that `new_hostile_for_test`
        // is unreachable outside `#[cfg(test)]`, not a runtime check inside
        // `commit`/`Drop`.
        let mut commit_fabric = fabric();
        let hostile_submission = submitted(&mut commit_fabric);
        let hostile_pending = commit_fabric.pending_dpc.take().unwrap();
        let hostile_end = 0x0000_0008;
        let hostile_rollback = DpcRegisters {
            start: 0,
            end: hostile_end,
            current: 0,
            status: 0,
            clock: DpcCounter24::from_register(0),
            busy: DpcCounter24::from_register(0),
            pipe_busy: DpcCounter24::from_register(0),
            tmem_busy: DpcCounter24::from_register(0),
        };
        let hostile_commit = ReadyDpcFabricCommit::new_hostile_for_test(
            &mut commit_fabric.dpc,
            &mut commit_fabric.pending_dpc,
            hostile_pending,
            hostile_end,
            hostile_rollback,
        );
        assert_eq!(hostile_commit.token(), hostile_submission.token);
        // No panic, no unwind: this line completing at all is the assertion.
        hostile_commit.commit();
        assert_eq!(commit_fabric.dpc.current, hostile_end);

        let mut drop_fabric = fabric();
        let _submission2 = submitted(&mut drop_fabric);
        let hostile_pending2 = drop_fabric.pending_dpc.take().unwrap();
        let hostile_drop = ReadyDpcFabricCommit::new_hostile_for_test(
            &mut drop_fabric.dpc,
            &mut drop_fabric.pending_dpc,
            hostile_pending2,
            hostile_end,
            hostile_rollback,
        );
        drop(hostile_drop); // No panic, no unwind.
        assert_eq!(drop_fabric.dpc.current, hostile_rollback.current);
    }

    /// Field-isolation proof.
    ///
    /// `DeviceFabric::prepare_dpc_commit(&mut self, ...)` ties its returned
    /// value's lifetime to the *whole* `&mut self` at the call site -- Rust's
    /// borrow checker cannot see through a method body to know only two
    /// fields end up borrowed, so `fabric.prepare_dpc_commit(...)` alone does
    /// NOT let a caller keep using other fields of `fabric` concurrently
    /// (confirmed: an earlier draft of this test called `fabric.si_status()`
    /// while holding the method's return value and it failed to borrow-check
    /// with E0502, exactly as disjoint-field method returns do in current
    /// stable Rust). What genuinely changed from the prior generic design is
    /// narrower and still real: `ReadyDpcFabricCommit<'a>`'s OWN fields are
    /// disjoint borrows of `dpc`/`pending_dpc` (not a single `&mut
    /// DeviceFabric<R, T>`), so a caller who destructures `DeviceFabric`
    /// itself -- exactly as `prepare_dpc_commit`'s body does internally --
    /// keeps every other field usable while holding those two. This test
    /// reproduces that destructuring at the call site to prove it, since
    /// `prepare_dpc_commit`'s own signature cannot expose it through a normal
    /// method call.
    #[test]
    fn disjoint_field_destructuring_leaves_every_other_field_usable() {
        let mut fabric = fabric();
        let submission = submitted(&mut fabric);
        // Two direct field-expression borrows (not a whole-struct pattern,
        // and not through a `&mut self` method) -- the form NLL reliably
        // treats as disjoint.
        let dpc: &mut DpcRegisters = &mut fabric.dpc;
        let pending_dpc: &mut Option<PendingDpc> = &mut fabric.pending_dpc;
        assert_eq!(
            pending_dpc
                .expect("submitted() left a pending DPC")
                .submission
                .token,
            submission.token
        );
        dpc.status |= 0; // touch `dpc` to keep the borrow demonstrably live
                         // `si_dram_addr` is an untouched field on the same struct; reading it
                         // directly while `dpc`/`pending_dpc` are still mutably borrowed above
                         // proves the compiler treats them as genuinely separate storage, not
                         // a whole-`fabric` borrow -- exactly the property `prepare_dpc_commit`
                         // relies on internally, even though its own `&mut self` signature
                         // cannot expose that property to a caller across the method boundary
                         // (see this test's doc comment above).
        let _si_dram_addr_reachable_while_dpc_fields_are_mutably_borrowed = fabric.si_dram_addr;
        dpc.status |= 0;
        let _ = pending_dpc;
    }
}
