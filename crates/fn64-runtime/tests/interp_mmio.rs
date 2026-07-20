//! The device-fabric half of the interpreter→device seam, proven against the
//! crate's REAL modeled device state (`fn64_runtime::DeviceFabric`).
//!
//! `fn64-recomp-rs`'s `MmioPort` trait (see its `interp` module) is the door the
//! interpreter reaches a modeled hardware register through; `fn64-recomp-rs`
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
//!  3. hole-stays-a-fault survives WITH an MMIO window present: an out-of-RDRAM,
//!     out-of-MMIO address is still a typed `MemoryFault`, and an in-window but
//!     unmodeled/misaligned register is a typed fault too — never a silent 0.
//!
//! The device interaction happens inside one synchronous interpreter turn, on
//! the calling stack: no wall-clock, no second thread. `DeviceFabric` advances
//! guest device time only when the test explicitly calls `advance_to` between
//! turns, exactly the deterministic (deadline, sequence) model U2 describes.

use fn64_recomp_rs::execution::{
    BankId, BlockExit, CodeBank, CpuFault, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget,
};
use fn64_recomp_rs::interp::{run_bank_with_mmio, MmioOutcome, MmioPort};
use fn64_recomp_rs::runtime::{Rdram, RecompContext};
use fn64_recomp_rs::CodeCatalog;

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
            Ok(()) => MmioOutcome::Handled(()),
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
            cart_addr: 0x10,
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
            cart_addr: 0x10,
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

/// (3a) hole-stays-a-fault WITH an MMIO window present: an out-of-RDRAM,
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
