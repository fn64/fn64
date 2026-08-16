//! The device-fabric half of the interpreter→device seam, proven against the
//! crate's REAL modeled device state (`fn64_runtime::DeviceFabric`).
//!
//! `fn64-cpu-runtime`'s `MmioPort` trait (see its `interp` module) is the door the
//! interpreter reaches a modeled hardware register through; `fn64-cpu-runtime`
//! itself cannot depend on this crate (the dependency edge runs the other way —
//! `docs/DESIGN.md` §1), so the *implementation* of that door over the real
//! `DeviceFabric` lives here. This test proves the whole slice end to end:
//!
//!  1. an interpreted `lw` of PI_STATUS reads the fabric's modeled status word,
//!     including a real device state transition (busy after a DMA start, cleared
//!     after the fabric advances time) — no second device authority, the same
//!     `DeviceFabric` a shim/AOT PI path would drive;
//!  2. an interpreted `sw` to PI_STATUS updates modeled device state (the PI
//!     interrupt-acknowledge/abort semantics `DeviceFabric::write_mmio` owns);
//!  3. the generated-code sparse direct backing remains distinct from MMIO:
//!     a canonical non-RDRAM KSEG load reaches supplied storage, while a true
//!     out-of-backing address and an unmodeled/misaligned register remain typed
//!     faults — never a silent 0.
//!
//! The device interaction happens inside one synchronous interpreter turn, on
//! the calling stack: no wall-clock, no second thread. `DeviceFabric` advances
//! guest device time only when the test explicitly calls `advance_to` between
//! turns, exactly the deterministic (deadline, sequence) model U2 describes.

use fn64_cpu_runtime::execution::{
    BankId, BlockExit, CodeBank, CpuFault, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget,
};
use fn64_cpu_runtime::interp::{run_bank_with_mmio, MmioOutcome, MmioPort};
use fn64_cpu_runtime::runtime::{Rdram, RecompContext, RDRAM_LEN};
use fn64_cpu_runtime::CodeCatalog;

use fn64_runtime::device::{Cycles, DeviceFabric, PiDmaRequest, PiTimingModel};
use fn64_runtime::rdram::Rdram as DeviceRdram;
use fn64_runtime::{
    is_mmio_offset, DmaDirection, InMemoryRom, MmioAddr, PiDma, RdramAddr, PI_STATUS_DMA_BUSY,
};

const BANK: BankId = BankId::new(0xD0);
const VA: u32 = 0x8000_1000;

/// A test PI timing model: every DMA completes 8 device cycles after it starts.
#[derive(Clone, Copy)]
struct FixedTiming(Cycles);

impl PiTimingModel for FixedTiming {
    fn completion_latency(
        &self,
        _request: PiDmaRequest,
        _timing: fn64_runtime::device::PiDomainTiming,
    ) -> Cycles {
        self.0
    }

    fn evidence_bytes(&self) -> Vec<u8> {
        let mut bytes = b"fn64.pi-timing.interp-mmio-test.v1\0".to_vec();
        bytes.extend_from_slice(&self.0.get().to_be_bytes());
        bytes
    }
}

/// The runtime-side `MmioPort`: the interpreter's word MMIO accesses translated
/// onto the real `DeviceFabric`. The fabric is the SINGLE device authority — the
/// same state a shim or AOT PI path drives; this port is only a typed adapter.
struct FabricPort<'a, R: fn64_runtime::RomStorage, T: PiTimingModel> {
    fabric: &'a mut DeviceFabric<R, T>,
}

impl<R: fn64_runtime::RomStorage, T: PiTimingModel> FabricPort<'_, R, T> {
    /// Whether `vaddr` is in the modeled hardware-register window. Uses the
    /// crate's own `is_mmio_offset` over `RdramAddr::from_gpr` — the exact
    /// window authority the shim MMIO path already uses, so there is no second
    /// definition of "what is a register."
    fn in_window(vaddr: u64) -> bool {
        is_mmio_offset(RdramAddr::from_gpr(vaddr).offset())
    }

    /// The raw KSEG1 register address the fabric decodes. The interpreter's
    /// effective address is the sign-extended 64-bit KSEG1 address; its low 32
    /// bits are exactly the `0xA4xx_xxxx` word the fabric keys on.
    fn mmio_addr(vaddr: u64) -> MmioAddr {
        MmioAddr::new(vaddr as u32)
    }
}

impl<R: fn64_runtime::RomStorage, T: PiTimingModel> MmioPort for FabricPort<'_, R, T> {
    fn read_w(&mut self, vaddr: u64) -> MmioOutcome<u32> {
        if !Self::in_window(vaddr) {
            return MmioOutcome::NotMmio;
        }
        match self.fabric.read_mmio(Self::mmio_addr(vaddr)) {
            Ok(value) => MmioOutcome::Handled(value),
            // In-window but the device rejected it (unmodeled/misaligned): a
            // typed fault, never a silent success.
            Err(_) => MmioOutcome::Fault { addr: vaddr },
        }
    }

    fn write_w(&mut self, vaddr: u64, value: u32) -> MmioOutcome<()> {
        if !Self::in_window(vaddr) {
            return MmioOutcome::NotMmio;
        }
        match self.fabric.write_mmio(Self::mmio_addr(vaddr), value) {
            Ok(_) => MmioOutcome::Handled(()),
            Err(_) => MmioOutcome::Fault { addr: vaddr },
        }
    }
}

fn catalog_of(words: &[u32]) -> CodeCatalog {
    let bank = CodeBank::new(BANK, GuestPc::new(VA), words.to_vec()).unwrap();
    let mut catalog = CodeCatalog::new();
    catalog.register(bank).unwrap();
    catalog
}

fn fabric() -> DeviceFabric<InMemoryRom, FixedTiming> {
    let rom = vec![0u8; 0x100];
    DeviceFabric::new(
        PiDma::new(InMemoryRom::new(rom)),
        FixedTiming(Cycles::new(8)),
    )
}

/// (1) An interpreted `lw` of PI_STATUS observes the fabric's modeled status —
/// including a real device state transition driven by the fabric's own timed
/// event model. This is the one concrete device interaction proven through the
/// interpreter.
#[test]
fn interpreted_lw_of_pi_status_observes_the_modeled_device_transition() {
    // lui $t0,0xA460 ; lw $v0,0x10($t0) ; jr $ra ; nop  — read PI_STATUS.
    let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0010, 0x03E0_0008, 0x0000_0000]);

    let mut fabric = fabric();
    // Start a real PI DMA on the fabric — its modeled PI_STATUS is now DMA_BUSY.
    fabric
        .start_pi_dma(PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x40),
            device: fn64_runtime::PiDeviceAddress::RomOffset(0x10),
            len: 4,
        })
        .unwrap();

    let read_status = |fabric: &mut DeviceFabric<InMemoryRom, FixedTiming>| -> u64 {
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut port = FabricPort { fabric };
        run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        ctx.r(2)
    };

    // While the DMA is in flight, the interpreted read sees BUSY — the modeled
    // register value, not raw RDRAM (which would fault).
    assert_eq!(
        read_status(&mut fabric),
        u64::from(PI_STATUS_DMA_BUSY),
        "interpreted lw observed the fabric's busy PI_STATUS"
    );

    // Advance device time past the DMA's completion deadline: the SAME fabric
    // clears busy. A second interpreted read now observes idle — a real device
    // state transition seen entirely through the interpreter seam.
    let mut device_rdram = DeviceRdram::new(0x100);
    fabric
        .advance_to(Cycles::new(100), &mut device_rdram)
        .unwrap();
    assert_eq!(
        read_status(&mut fabric),
        0,
        "after the fabric advanced time, the interpreted lw observes idle"
    );
}

/// (2) An interpreted `sw` to PI_STATUS updates modeled device state. Writing
/// bit 1 (the PI interrupt-clear command) acknowledges a raised PI interrupt in
/// the fabric — a real modeled write effect, through the same authority.
#[test]
fn interpreted_sw_to_pi_status_updates_modeled_device_state() {
    // Drive a DMA to completion so the fabric raises a pending PI MI interrupt.
    let mut fabric = fabric();
    fabric
        .start_pi_dma(PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x40),
            device: fn64_runtime::PiDeviceAddress::RomOffset(0x10),
            len: 4,
        })
        .unwrap();
    let mut device_rdram = DeviceRdram::new(0x100);
    fabric
        .advance_to(Cycles::new(100), &mut device_rdram)
        .unwrap();
    assert!(
        fabric.interrupt_pending(fn64_runtime::InterruptSource::Pi),
        "the completed DMA raised a modeled PI interrupt"
    );

    // lui $t0,0xA460 ; ori $v0,$zero,2 ; sw $v0,0x10($t0) ; jr $ra ; nop
    // A store of 2 (PI interrupt-clear) to PI_STATUS acknowledges it.
    let catalog = catalog_of(&[
        0x3C08_A460, // lui $t0,0xA460
        0x3402_0002, // ori $v0,$zero,2
        0xAD02_0010, // sw $v0,0x10($t0)
        0x03E0_0008, // jr $ra
        0x0000_0000, // nop
    ]);
    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    {
        let mut port = FabricPort {
            fabric: &mut fabric,
        };
        run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
    }
    assert!(
        !fabric.interrupt_pending(fn64_runtime::InterruptSource::Pi),
        "the interpreted sw cleared the modeled PI interrupt"
    );
}

#[test]
fn interpreted_ai_programming_uses_the_fabric_latches_and_timed_fifo() {
    // Enable DMA, program DACRATE, DRAM_ADDR, and LEN, then read STATUS and
    // LEN back.
    let catalog = catalog_of(&[
        0x3C08_A450, // lui $t0,0xA450
        0x3409_0001, // ori $t1,$zero,1
        0xAD09_0008, // sw $t1,AI_CONTROL($t0)
        0x3409_0097, // ori $t1,$zero,151
        0xAD09_0010, // sw $t1,AI_DACRATE($t0)
        0x3409_0200, // ori $t1,$zero,0x200
        0xAD09_0000, // sw $t1,AI_DRAM_ADDR($t0)
        0x3409_0080, // ori $t1,$zero,0x80
        0xAD09_0004, // sw $t1,AI_LEN($t0)
        0x8D02_000C, // lw $v0,AI_STATUS($t0)
        0x8D03_0004, // lw $v1,AI_LEN($t0)
        0x03E0_0008, // jr $ra
        0x0000_0000, // nop
    ]);
    let mut fabric = fabric();
    fabric
        .configure_tv_type(fn64_runtime::TvType::Ntsc)
        .unwrap();
    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };
    run_bank_with_mmio(
        &catalog,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(18).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();

    let snapshot = fabric.snapshot();
    assert_eq!(snapshot.ai_dram_addr, RdramAddr::from_offset(0x200));
    assert_eq!(snapshot.ai_dacrate, 151);
    assert_eq!(snapshot.ai_length, 0x80);
    assert_eq!(
        ctx.r(2) as u32,
        fn64_runtime::AI_STATUS_ENABLED | fn64_runtime::AI_STATUS_BUSY
    );
    assert_eq!(ctx.r(3), 0x80);
}

#[test]
fn interpreted_dpc_end_waits_for_commit_and_identical_end_does_not_replay() {
    let submit = catalog_of(&[
        0x3C08_A410, // lui $t0,0xA410
        0x3409_0100, // ori $t1,$zero,0x100
        0xAD09_0000, // sw $t1,DPC_START($t0)
        0x3409_0140, // ori $t1,$zero,0x140
        0xAD09_0004, // sw $t1,DPC_END($t0)
        0x8D02_0008, // lw $v0,DPC_CURRENT($t0)
        0x8D03_000C, // lw $v1,DPC_STATUS($t0)
        0x03E0_0008, // jr $ra
        0x0000_0000, // nop
    ]);
    let mut fabric = fabric();
    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };
    run_bank_with_mmio(
        &submit,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(16).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();
    let transaction = fabric
        .pending_dpc_submission()
        .expect("interpreted END must retain a renderer transaction");
    assert_eq!(transaction.source, fn64_runtime::DpcSubmissionSource::Rdram);
    assert_eq!((transaction.start, transaction.end), (0x100, 0x140));
    assert_eq!(
        ctx.r(2),
        0x100,
        "CURRENT stops at START before renderer commit"
    );
    assert_ne!(ctx.r(3) as u32 & fn64_runtime::DPC_STATUS_DMA_BUSY, 0);

    fabric.commit_dpc_submission(transaction.token).unwrap();
    assert_eq!(fabric.snapshot().dpc_current, 0x140);

    // A later interpreted write of the same END pointer is a valid no-op,
    // not a second submission of the already consumed command bytes.
    let repeat = catalog_of(&[
        0x3C08_A410,
        0x3409_0140,
        0xAD09_0004,
        0x8D02_0008,
        0x03E0_0008,
        0x0000_0000,
    ]);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };
    run_bank_with_mmio(
        &repeat,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(12).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();
    assert_eq!(ctx.r(2), 0x140);
    assert_eq!(fabric.pending_dpc_submission(), None);
}

/// (3a) Canonical non-RDRAM KSEG windows use the same sparse backing layout as
/// generated C after the device port declines them. The first byte after the
/// physical 8 MiB prefix is sufficient to prove the general classifier without
/// allocating the much later DDROM offset used by `osDriveRomInit`.
#[test]
fn interpreted_non_mmio_direct_window_reads_supplied_sparse_backing() {
    // lui $t0,0x8080 ; lw $v0,0($t0) ; jr $ra ; nop. LUI sign-extends the
    // effective address to FFFFFFFF80800000, whose sparse backing offset is
    // exactly 0x00800000.
    let catalog = catalog_of(&[0x3C08_8080, 0x8D02_0000, 0x03E0_0008, 0x0000_0000]);
    let mut fabric = fabric();
    let mut storage = vec![0u8; RDRAM_LEN + 4];
    storage[RDRAM_LEN..RDRAM_LEN + 4].copy_from_slice(&0x1234_5678u32.to_ne_bytes());
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };

    let run = run_bank_with_mmio(
        &catalog,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(8).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();

    assert!(!matches!(run.exit, BlockExit::Fault(_)));
    assert_eq!(ctx.r(2), 0x1234_5678);
}

/// (3b) hole-stays-a-fault WITH an MMIO window present: an out-of-RDRAM,
/// out-of-MMIO address is still a typed `MemoryFault`. The MMIO seam must not
/// make an arbitrary out-of-RDRAM address succeed.
#[test]
fn non_mmio_out_of_rdram_still_faults_typed_with_the_fabric_port_present() {
    // lui $t0,0x8000 ; lw $v0,0x40($t0)  — 0x8000_0040, outside the 16-byte
    // rdram AND outside every modeled register window.
    let catalog = catalog_of(&[0x3C08_8000, 0x8D02_0040, 0x03E0_0008, 0x0000_0000]);
    let mut fabric = fabric();
    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };
    let run = run_bank_with_mmio(
        &catalog,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(8).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();
    assert!(
        matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault {
                    addr: 0xFFFF_FFFF_8000_0040
                },
                ..
            })
        ),
        "a non-MMIO out-of-RDRAM load is still a typed MemoryFault, got {:?}",
        run.exit
    );
    assert_eq!(ctx.r(2), 0, "the faulting load wrote no register");
}

/// (3b) An in-window but UNMODELED register (a PI-block offset the fabric does
/// not decode) is a typed fault, not a silent 0 — the loud-trap property
/// survives the seam.
#[test]
fn in_window_unmodeled_register_is_a_typed_fault_not_a_silent_zero() {
    // lui $t0,0xA460 ; lw $v0,0x08($t0)  — PI +0x08 is not a decoded register.
    let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0008, 0x03E0_0008, 0x0000_0000]);
    let mut fabric = fabric();
    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut port = FabricPort {
        fabric: &mut fabric,
    };
    let run = run_bank_with_mmio(
        &catalog,
        BANK,
        ExecutionKey::new(BANK, GuestPc::new(VA)),
        InstructionBudget::new(8).unwrap(),
        &mut ctx,
        &mut mem,
        &mut port,
    )
    .unwrap();
    assert!(
        matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { .. },
                ..
            })
        ),
        "an in-window unmodeled register is a typed fault, got {:?}",
        run.exit
    );
}
