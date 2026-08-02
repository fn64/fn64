    use super::*;
    use crate::rdram::Rdram;
    use crate::rom::{DmaWriterChannel, InMemoryRom, ProcessDmaMemory};

    #[derive(Clone, Copy)]
    struct TestTiming(Cycles);


    impl PiTimingModel for TestTiming {
        fn completion_latency(&self, _request: PiDmaRequest, _timing: PiDomainTiming) -> Cycles {
            self.0
        }

        fn evidence_bytes(&self) -> Vec<u8> {
            let mut bytes = b"fn64.pi-timing.test.v1\0".to_vec();
            bytes.extend_from_slice(&self.0.get().to_be_bytes());
            bytes
        }
    }


    fn fabric() -> DeviceFabric<InMemoryRom, TestTiming> {
        let mut rom = vec![0u8; 0x100];
        rom[0x10..0x14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        DeviceFabric::new(
            PiDma::new(InMemoryRom::new(rom)),
            TestTiming(Cycles::new(12)),
        )
    }


    fn complete_rsp_state() -> RspExecutionState {
        RspExecutionState {
            pc: 0x0abc,
            sp_status: SP_STATUS_HALT
                | SP_STATUS_BROKE
                | SP_STATUS_DMA_BUSY
                | SP_STATUS_DMA_FULL
                | SP_STATUS_SIGNAL_0,
            sp_semaphore: true,
            sp_dma_mem_addr: RspMemAddr::from_register(0x1a5b),
            sp_dma_dram_addr: RdramAddr::from_offset(0x00ab_cdef),
            sp_dma_read_length: 0x1234_5678,
            sp_dma_write_length: 0x9abc_def0,
            dpc_start: 0x0012_3400,
            dpc_end: 0x0012_3480,
            dpc_current: 0x0012_3440,
            dpc_status: DPC_STATUS_FREEZE | DPC_STATUS_CMD_BUSY,
            dpc_clock: 0x1020_3040,
            dpc_busy: 0x5060_7080,
            dpc_pipe_busy: 0x90a0_b0c0,
            dpc_tmem_busy: 0xd0e0_f000,
        }
    }


    fn seed_dpc_counters(fabric: &mut DeviceFabric<InMemoryRom, TestTiming>) {
        fabric
            .commit_complete_rsp_execution_state(RspExecutionState {
                dpc_start: 0,
                dpc_end: 0,
                dpc_current: 0,
                dpc_status: 0,
                dpc_clock: 1,
                dpc_busy: 2,
                dpc_pipe_busy: 3,
                dpc_tmem_busy: 4,
                ..complete_rsp_state()
            })
            .unwrap();
    }

mod device_a;
mod device_b;
