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
                let submission = self.begin_dpc_submission(source, start, end, rollback)?;
                return Ok(DeviceMmioWriteEffect::DpcSubmissionRequested(submission));
            }
            DPC_CURRENT_REG => {
                return Err(DeviceFault::UnmodeledMmioWrite { addr, value });
            }
            DPC_STATUS_REG => {
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

    pub(crate) fn write_mmio_without_effect(&mut self, addr: MmioAddr, value: u32) -> Result<(), DeviceFault> {
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
}
