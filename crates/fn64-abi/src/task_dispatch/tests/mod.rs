    use super::*;
    use crate::test_support::*;
    use fn64_render::{FrameStatus, RenderConfig, RenderError, UcodeId};
    use fn64_runtime::RecvMesgOutcome;
    use std::rc::Rc;

    /// Emits the `RenderBackend::observe_non_rdp_write16` method reporting no
    /// Rust hidden sidecar. Shared by the helpers here and all three test
    /// buckets; `macro_rules!` is in scope for the rest of this module and for
    /// the child modules declared textually after it.
    macro_rules! no_rust_hidden_sidecar {
        () => {
            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
            }
        };
    }

    fn install_running_task_lineage(
        task_addr: RdramAddr,
        admission_generation: RspTaskAdmissionGeneration,
    ) {
        with_host(|host| {
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    admission_generation,
                    original_header: OsTaskHeader::default(),
                    data_identity: None,
                    phase: RspTaskLineagePhase::Running,
                },
            );
        });
    }


    fn write_test_task_header(rdram: &mut [u8], task_offset: usize, header: OsTaskHeader) {
        for (index, word) in [
            header.task_type,
            header.flags,
            header.ucode_boot,
            header.ucode_boot_size,
            header.ucode,
            header.ucode_size,
            header.ucode_data,
            header.ucode_data_size,
            header.dram_stack,
            header.dram_stack_size,
            header.output_buff,
            header.output_buff_size,
            header.data_ptr,
            header.data_size,
            header.yield_data_ptr,
            header.yield_data_size,
        ]
        .into_iter()
        .enumerate()
        {
            let offset = task_offset + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
    }


    fn prepare_audio_capture_task(
        rdram: &mut Vec<u8>,
        header: OsTaskHeader,
    ) -> (RdramAddr, RspTaskAdmissionGeneration) {
        const TASK_OFFSET: usize = 0x40;
        crate::load_rom(Vec::new());
        rdram.resize(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE, 0);
        write_test_task_header(rdram, TASK_OFFSET, header);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + TASK_OFFSET as u64;
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let task_addr = RdramAddr::from_offset(TASK_OFFSET as u32);
        let loaded = take_loaded_rsp_task(task_addr);
        let admission_generation = loaded.admission_generation;
        retain_started_rsp_task_lineage(loaded, None);
        (task_addr, admission_generation)
    }


    fn boot_overlay_audio_header() -> OsTaskHeader {
        OsTaskHeader {
            task_type: fn64_runtime::M_AUDTASK,
            ucode_boot: 0x8000_0100,
            ucode_boot_size: 8,
            ucode: 0xa000_0120,
            ucode_size: 8,
            ..OsTaskHeader::default()
        }
    }


    fn prepare_renderer_rdram(rdram: &mut Vec<u8>) {
        rdram.resize(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE, 0);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
    }


    const VERIFIED_AUDIO_TASK_OFFSET: u32 = 0x80;

    const VERIFIED_AUDIO_GENERATION: NonZeroU64 = NonZeroU64::MIN;


    fn prepare_verified_audio_rdram(rdram: &mut Vec<u8>) -> (RdramAddr, NonZeroU64) {
        prepare_renderer_rdram(rdram);
        let task_addr = RdramAddr::from_offset(VERIFIED_AUDIO_TASK_OFFSET);
        with_host(|host| {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(
                    task_addr.offset(),
                    RspTaskAdmissionGeneration::new(VERIFIED_AUDIO_GENERATION),
                ),
            };
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    admission_generation: RspTaskAdmissionGeneration(VERIFIED_AUDIO_GENERATION),
                    original_header: OsTaskHeader::default(),
                    data_identity: None,
                    phase: RspTaskLineagePhase::Running,
                },
            );
            host.next_rsp_task_admission_generation =
                RspTaskAdmissionGeneration(NonZeroU64::new(2).unwrap());
        });
        (task_addr, VERIFIED_AUDIO_GENERATION)
    }


    fn verified_audio_test_machine() -> fn64_audio::rsp::runtime::RspMachineState {
        let mut storage = vec![0; 0x1000];
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut storage);
        machine.ctx.r[7] = 0x1234_5678;
        machine.snapshot_state()
    }


    fn full_sync_deferred_submission() -> fn64_audio::hle_outcome::DeferredDpcSubmission {
        fn64_audio::hle_outcome::DeferredDpcSubmission::from_rdram_words(
            0x100,
            0x108,
            vec![0xe900_0000, 0],
        )
        .unwrap()
    }


    fn empty_verified_audio_patches() -> fn64_audio::hle_outcome::CanonicalRdramPatches {
        fn64_audio::hle_outcome::CanonicalRdramPatches::new(Vec::new()).unwrap()
    }


    struct StatusRenderBackend(FrameStatus);


    impl RenderBackend for StatusRenderBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(self.0)
        }

        fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
            if self.0 == FrameStatus::Complete {
                fn64_render::DpFullSyncStatus::Reached
            } else {
                fn64_render::DpFullSyncStatus::NotReached
            }
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    struct CountingPanicRenderBackend(std::rc::Rc<Cell<u32>>);


    impl RenderBackend for CountingPanicRenderBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            self.0.set(self.0.get() + 1);
            panic!("intentional direct-IMEM backend panic")
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    fn direct_imem_test_header(image: u32) -> OsTaskHeader {
        OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ucode_boot: 0x8000_0000 | image,
            ucode_boot_size: 8,
            ucode: 0xa000_0000 | image,
            ucode_size: 8,
            ..OsTaskHeader::default()
        }
    }


    struct UnsupportedUcodeBackend;


    impl RenderBackend for UnsupportedUcodeBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Err(RenderError::UnsupportedUcode {
                ucode_addr: task.ucode,
            })
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    struct ExactIdentityBackend {
        admitted: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        admitted_data: fn64_render::MicrocodeDataImageIdentity,
        family: UcodeId,
    }


    impl RenderBackend for ExactIdentityBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(FrameStatus::Complete)
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn identify_microcode(
            &self,
            imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        ) -> Option<UcodeId> {
            (imem == &self.admitted).then_some(self.family)
        }

        fn identify_microcode_pair(
            &self,
            imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            data: fn64_render::MicrocodeDataImageIdentity,
        ) -> Option<UcodeId> {
            (imem == &self.admitted && data == self.admitted_data).then_some(self.family)
        }

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
        payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic")
    }


    /// Install a minimal public-protocol rspboot which DMA-loads eight bytes
    /// at IMEM 0x1080 and jumps there, then admit the task through the real
    /// `osSpTaskLoad` shim. Words use the native backing representation which
    /// `RdramPtr` exposes as guest big-endian logical bytes.
    fn admit_synthetic_hle_task(rdram: &mut Vec<u8>, header_off: usize, ctx: &mut RecompContext) {
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let boot_off = (rdram.len() + 7) & !7;
        let ucode_off = boot_off + 32;
        assert!(ucode_off <= i16::MAX as usize);
        rdram.resize(ucode_off + 8, 0);
        let boot = [
            0x2402_0000 | ucode_off as u32,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        for (index, word) in boot.into_iter().enumerate() {
            let offset = boot_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        for (field, value) in [
            (0x08, boot_off as u32),
            (0x0c, 32),
            (0x10, ucode_off as u32),
            (0x14, 8),
        ] {
            rdram[header_off + field..header_off + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), ctx) };
    }


    #[derive(Clone, Copy, Debug)]
    enum RawMutationOutcome {
        Complete,
        Error,
        Panic,
        Yielded,
    }


    struct MutatingRawBackend {
        calls: Rc<Cell<u32>>,
        outcome: RawMutationOutcome,
        mutation_offset: usize,
    }


    impl RenderBackend for MutatingRawBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            unreachable!("raw DPC regression backend received an HLE task")
        }

        fn process_rdp_commands(
            &mut self,
            rdram: &mut [u8],
            _start: u32,
            _end: u32,
            _output_addr: u32,
            _wait_for_completion: bool,
        ) -> Result<FrameStatus, RenderError> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            rdram[self.mutation_offset] = call as u8;
            match self.outcome {
                RawMutationOutcome::Complete => Ok(FrameStatus::Complete),
                RawMutationOutcome::Error => Err(RenderError::Backend {
                    backend: "synthetic-raw",
                    reason: "mutate-then-error".to_owned(),
                }),
                RawMutationOutcome::Panic => panic!("mutating raw backend panic"),
                RawMutationOutcome::Yielded => Ok(FrameStatus::Yielded),
            }
        }

        fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
            fn64_render::DpFullSyncStatus::Reached
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    #[derive(Clone, Copy)]
    enum ScheduledRawDpcReply {
        BackendError,
        WrongTransaction,
        WrongQuantum,
        WrongCursor,
        Continue(fn64_render::DpFullSyncStatus),
        Complete(fn64_render::DpFullSyncStatus),
    }


    struct ScheduledRawDpcBackend {
        replies: std::collections::VecDeque<ScheduledRawDpcReply>,
        calls: usize,
        steps: Vec<fn64_render::RawDpcStep>,
    }


    impl ScheduledRawDpcBackend {
        fn new(replies: impl IntoIterator<Item = ScheduledRawDpcReply>) -> Self {
            Self {
                replies: replies.into_iter().collect(),
                calls: 0,
                steps: Vec::new(),
            }
        }
    }


    impl RenderBackend for ScheduledRawDpcBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        fn observe_non_rdp_write16(
            &mut self,
            _write: fn64_render::NonRdpWrite16,
        ) -> fn64_render::NonRdpWrite16Disposition {
            fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
        }

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            unreachable!("scheduled raw-DPC test cannot dispatch an HLE task")
        }

        fn raw_dpc_progression(&self) -> fn64_render::RawDpcProgression {
            fn64_render::RawDpcProgression::Acknowledged
        }

        fn process_rdp_command_chunk(
            &mut self,
            rdram: &mut [u8],
            quantum: fn64_render::RawDpcQuantum,
            step: fn64_render::RawDpcStep,
        ) -> Result<fn64_render::RawDpcChunkAck, RenderError> {
            self.calls += 1;
            self.steps.push(step);
            rdram[self.calls - 1] = 0xa0 + self.calls as u8;
            let reply = self
                .replies
                .pop_front()
                .expect("scheduled raw-DPC test exhausted backend replies");
            let mut ack = fn64_render::RawDpcChunkAck {
                transaction: quantum.request.transaction,
                quantum: quantum.request.quantum,
                committed_through: quantum.request.end,
                status: fn64_render::RawDpcChunkStatus::Continue(
                    fn64_render::RenderRawDpcContinuation::new(91),
                ),
                full_sync: fn64_render::DpFullSyncStatus::NotReached,
            };
            match reply {
                ScheduledRawDpcReply::BackendError => Err(RenderError::Backend {
                    backend: "scheduled-raw-dpc-test",
                    reason: "injected failure after shadow mutation".into(),
                }),
                ScheduledRawDpcReply::WrongTransaction => {
                    ack.transaction = fn64_runtime::DpcTransactionId::from_submission(
                        fn64_runtime::DpcSubmission {
                            token: quantum.request.transaction.get() + 1,
                            source: quantum.request.start.source(),
                            start: quantum.request.start.address(),
                            end: quantum.request.end.address(),
                        },
                    );
                    Ok(ack)
                }
                ScheduledRawDpcReply::WrongQuantum => {
                    ack.quantum =
                        fn64_runtime::DpcQuantumId::new(quantum.request.quantum.get() + 1);
                    Ok(ack)
                }
                ScheduledRawDpcReply::WrongCursor => {
                    ack.committed_through = quantum.request.start;
                    Ok(ack)
                }
                ScheduledRawDpcReply::Continue(full_sync) => {
                    ack.full_sync = full_sync;
                    Ok(ack)
                }
                ScheduledRawDpcReply::Complete(full_sync) => {
                    ack.status = fn64_render::RawDpcChunkStatus::Complete;
                    ack.full_sync = full_sync;
                    Ok(ack)
                }
            }
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }


    fn scheduled_raw_dpc_transaction() -> ScheduledRawDpcTransaction {
        let source = fn64_runtime::DpcSubmissionSource::Rdram;
        let cursor = |address| fn64_runtime::DpcCursor::new(source, address).unwrap();
        ScheduledRawDpcTransaction::new(
            fn64_runtime::DpcScheduledExecution::new(
                fn64_runtime::DpcSubmission {
                    token: 5,
                    source,
                    start: 0x100,
                    end: 0x110,
                },
                fn64_runtime::Cycles::new(0),
                vec![
                    fn64_runtime::DpcQuantumPlan {
                        at: fn64_runtime::Cycles::new(2),
                        id: fn64_runtime::DpcQuantumId::new(1),
                        start: cursor(0x100),
                        end: cursor(0x108),
                    },
                    fn64_runtime::DpcQuantumPlan {
                        at: fn64_runtime::Cycles::new(3),
                        id: fn64_runtime::DpcQuantumId::new(2),
                        start: cursor(0x108),
                        end: cursor(0x110),
                    },
                ],
            )
            .unwrap(),
        )
    }

mod dispatch_a;
mod dispatch_b;
mod dispatch_c;
mod render_ir_integration;
