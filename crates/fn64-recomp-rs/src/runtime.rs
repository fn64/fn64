//! The typed runtime that emitted Rust targets: [`RecompContext`] (the CPU
//! register file) and [`Rdram`] (a checked memory view).
//!
//! # Why this exists (the whole point of `-rs`)
//!
//! The N64Recomp C output reaches memory through raw macros like
//! `*(int16_t*)(rdram + (((reg + off) ^ 2) - 0x…80000000))` — a pointer cast
//! and a hand-written byte swizzle at every access. That is exactly the
//! byte-reinterpret bug class this project has been paying for. Here the
//! swizzle lives in ONE place, expressed as safe indexing on a `&mut [u8]`,
//! and every emitted access goes through a *typed method* (`load_w`,
//! `store_h`, …). No emitted code ever casts a pointer. `#![forbid(unsafe_code)]`
//! at the crate root makes that structural, not merely a convention.
//!
//! # Semantic model (matches N64Recomp's `recomp.h`, clean-room from the ISA)
//!
//! - A GPR is a 64-bit value (`gpr = uint64_t` in the C). 32-bit results are
//!   sign-extended into it (that is what `S32`/`ADD32` do). We store GPRs as
//!   `u64` and expose typed read/write helpers.
//! - Zero- or sign-extended KSEG0/KSEG1 addresses in the physical RDRAM
//!   window map through their shared low-29-bit physical offset. Canonical
//!   32-bit mapped data addresses use the context's recorded TLB entries;
//!   instruction fetch has a separate physical-address result so architectural
//!   PCs are never reused as admitted code identity. The arbitrary-PC data
//!   path classifies the VR4300's 32- and 64-bit segments before either TLB or
//!   direct-physical translation; instruction PCs remain a 32-bit schema.
//! - Word accesses use the host-native representation used by the ABI buffer;
//!   sub-word accesses XOR the byte offset: halfword `^2`, byte `^3`. This is
//!   the N64's big-endian view over a little-endian host buffer. It is applied
//!   here in one spot, in [`Rdram`].

/// The recompiled-CPU register context: 32 general-purpose registers plus the
/// HI/LO multiply-divide pair. `$zero` (index 0) reads as 0 and ignores writes.
///
/// GPRs are stored as `u64` to hold the sign-extended 64-bit values MIPS
/// keeps; the typed accessors ([`RecompContext::r`], [`RecompContext::set_r32`],
/// …) enforce the sign/zero-extension contract so emitted code never open-codes
/// a cast.
/// One raw TLB entry as staged by the COP0 registers at `tlbwi` time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TlbEntryRaw {
    pub page_mask: u32,
    pub entry_hi: u64,
    pub entry_lo0: u32,
    pub entry_lo1: u32,
}

/// One issued register-targeted control transfer retained for divergence
/// diagnosis. `target_pc` is captured before the delay slot; the architectural
/// snapshot is taken after that slot retires and immediately before dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndirectTransferObservation {
    pub source_bank: u64,
    pub source_pc: u32,
    pub source_register: u8,
    pub target_pc: u32,
    pub link_pc: Option<u32>,
    pub gprs: [u64; 32],
    pub hi: u64,
    pub lo: u64,
    pub cop0_status: u32,
    pub cop0_cause: u32,
    pub cop0_epc: u32,
}

/// Direction of one guest data-memory translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataAccessKind {
    Load,
    Store,
}

/// Why a mapped guest data address did not translate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlbFaultKind {
    Refill,
    Invalid,
    Modified,
}

/// Typed result of a failed TLB translation before any memory side effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlbFault {
    pub vaddr: u64,
    pub access: DataAccessKind,
    pub kind: TlbFaultKind,
    /// A first-level refill for this access uses the XTLB refill vector.
    pub extended: bool,
}

/// Address selected by the VR4300's 32-bit data-address translation rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslatedDataAddress {
    /// KSEG0/KSEG1 keep their existing direct virtual form.
    Direct(u64),
    /// XKPHYS, or ERL's low user window, selected this physical address
    /// without consulting the TLB.
    DirectPhysical(u32),
    /// A mapped segment selected this physical byte address through the TLB.
    Mapped(u32),
}

/// Physical instruction-word address selected by the VR4300's 32-bit fetch
/// translation rules.
///
/// This deliberately has no direct-segment variant: KSEG0/KSEG1 are aliases
/// only at the architectural VA layer. Code admission and generation identity
/// are always qualified by the resulting physical word address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranslatedInstructionAddress(u32);

impl TranslatedInstructionAddress {
    pub const fn new(physical: u32) -> Self {
        Self(physical)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Fail-closed result for tooling which must inspect TLB geometry without
/// selecting behavior the VR4300 leaves undefined.
///
/// The execution-facing translation methods retain their loud traps for the
/// undefined cases. Discovery diagnostics use this currency so an unsupported
/// raw PageMask or competing tag match remains evidence instead of unwinding
/// the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstructionTranslationDiagnosticErrorV1 {
    Access(DataAccessError),
    InvalidPageMaskEncoding {
        index: usize,
        page_mask_raw: u32,
    },
    MultipleTlbMatches {
        vaddr: u64,
        first_index: usize,
        second_index: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddressRoute {
    DirectVirtual(u64),
    DirectPhysical(u32),
    Mapped { extended: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CpuMode {
    Kernel,
    Supervisor,
    User,
}

/// Typed checked-memory failure shared by generated and interpreted blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataAccessError {
    /// Translation succeeded (or was direct), but fn64 owns no backing/device
    /// path for the resulting address.
    Unbacked {
        vaddr: u64,
    },
    AddressError {
        vaddr: u64,
        access: DataAccessKind,
    },
    Tlb(TlbFault),
}

impl DataAccessError {
    pub const fn is_architectural_exception(self) -> bool {
        matches!(self, Self::AddressError { .. } | Self::Tlb(_))
    }

    /// Convert a checked-memory failure into the arbitrary-PC lane's typed
    /// fault currency while retaining the instruction's precise EPC/BD state.
    pub fn into_cpu_fault_kind(
        self,
        epc: crate::execution::GuestPc,
        branch_delay: bool,
    ) -> crate::execution::CpuFaultKind {
        use crate::execution::{CpuException, CpuFaultKind};

        match self {
            Self::Unbacked { vaddr } => CpuFaultKind::MemoryFault { addr: vaddr },
            Self::AddressError { vaddr, access } => CpuFaultKind::Exception {
                exception: match access {
                    DataAccessKind::Load => CpuException::AddressErrorLoad,
                    DataAccessKind::Store => CpuException::AddressErrorStore,
                },
                epc,
                branch_delay,
                instruction_code: 0,
                bad_vaddr: Some(vaddr),
                coprocessor: None,
            },
            Self::Tlb(fault) => {
                let exception = match (fault.kind, fault.access) {
                    (TlbFaultKind::Refill, DataAccessKind::Load) if fault.extended => {
                        CpuException::XTlbRefillLoad
                    }
                    (TlbFaultKind::Refill, DataAccessKind::Store) if fault.extended => {
                        CpuException::XTlbRefillStore
                    }
                    (TlbFaultKind::Refill, DataAccessKind::Load) => CpuException::TlbRefillLoad,
                    (TlbFaultKind::Refill, DataAccessKind::Store) => CpuException::TlbRefillStore,
                    (TlbFaultKind::Invalid, DataAccessKind::Load) => CpuException::TlbInvalidLoad,
                    (TlbFaultKind::Invalid, DataAccessKind::Store) => CpuException::TlbInvalidStore,
                    (TlbFaultKind::Modified, DataAccessKind::Store) => CpuException::TlbModified,
                    (TlbFaultKind::Modified, DataAccessKind::Load) => {
                        unreachable!("a load cannot raise TLB Modified")
                    }
                };
                CpuFaultKind::Exception {
                    exception,
                    epc,
                    branch_delay,
                    instruction_code: 0,
                    bad_vaddr: Some(fault.vaddr),
                    coprocessor: None,
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FixedFloatFormat {
    Single,
    Double,
}

impl FixedFloatFormat {
    const fn fraction_bits(self) -> u32 {
        match self {
            Self::Single => 23,
            Self::Double => 52,
        }
    }

    const fn exponent_bias(self) -> u32 {
        match self {
            Self::Single => 127,
            Self::Double => 1023,
        }
    }
}

/// Encode one signed fixed-point integer as IEEE S or D without entering the
/// host floating-point environment. The returned boolean reports discarded
/// nonzero source bits, independently of whether rounding changes the retained
/// significand.
fn encode_fixed_float(value: i64, format: FixedFloatFormat, mode: u8) -> (u64, bool) {
    assert!(mode < 4, "FCSR.RM exceeds two bits");
    if value == 0 {
        return (0, false);
    }

    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let fraction_bits = format.fraction_bits();
    let mut exponent = 63 - magnitude.leading_zeros();
    let (mut significand, remainder, shift) = if exponent > fraction_bits {
        let shift = exponent - fraction_bits;
        (magnitude >> shift, magnitude & ((1u64 << shift) - 1), shift)
    } else {
        (magnitude << (fraction_bits - exponent), 0, 0)
    };
    let inexact = remainder != 0;
    let increment = if !inexact {
        false
    } else {
        match mode {
            0 => {
                let half = 1u64 << (shift - 1);
                remainder > half || (remainder == half && significand & 1 != 0)
            }
            1 => false,
            2 => !negative,
            3 => negative,
            _ => unreachable!("rounding mode was range-checked"),
        }
    };
    if increment {
        significand += 1;
        if significand == 1u64 << (fraction_bits + 1) {
            significand >>= 1;
            exponent += 1;
        }
    }

    let sign = u64::from(negative)
        << (fraction_bits
            + match format {
                FixedFloatFormat::Single => 8,
                FixedFloatFormat::Double => 11,
            });
    let exponent = u64::from(exponent + format.exponent_bias()) << fraction_bits;
    let fraction = significand & ((1u64 << fraction_bits) - 1);
    (sign | exponent | fraction, inexact)
}

const FPU_CAUSE_I: u8 = 1 << 0;
const FPU_CAUSE_U: u8 = 1 << 1;
const FPU_CAUSE_O: u8 = 1 << 2;

/// Round an unsigned significand after a right shift without consulting the
/// host floating-point environment. `mode` is FCSR.RM and `negative` selects
/// the direction for RP/RM.
fn round_shift_right(value: u64, shift: u32, mode: u8, negative: bool) -> (u64, bool) {
    assert!(mode < 4, "FCSR.RM exceeds two bits");
    if shift == 0 {
        return (value, false);
    }
    let (retained, remainder, half) = if shift < 64 {
        (
            value >> shift,
            value & ((1u64 << shift) - 1),
            Some(1u64 << (shift - 1)),
        )
    } else {
        (0, value, (shift == 64).then_some(1u64 << 63))
    };
    let inexact = remainder != 0;
    let increment = inexact
        && match mode {
            0 => half
                .is_some_and(|half| remainder > half || (remainder == half && retained & 1 != 0)),
            1 => false,
            2 => !negative,
            3 => negative,
            _ => unreachable!("rounding mode was range-checked"),
        };
    (retained + u64::from(increment), inexact)
}

fn overflowed_single(mode: u8, negative: bool, max: u32, infinity: u32) -> u32 {
    match (mode, negative) {
        (0, _) => infinity,
        (1, _) => max,
        (2, false) | (3, true) => infinity,
        (2, true) | (3, false) => max,
        _ => unreachable!("FCSR.RM exceeds two bits"),
    }
}

const COP0_STATUS_FR: u32 = 1 << 26;

/// View-independent contents of the 32 physical COP1 FGRs.
///
/// This is the state-transfer currency for ABI bridges, coroutine ownership,
/// and deterministic evidence. It deliberately does not expose an FR-shaped
/// array: FR is a view selected by the receiving [`RecompContext`]'s Status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalFgrState([u64; 32]);

impl PhysicalFgrState {
    pub const fn from_words(words: [u64; 32]) -> Self {
        Self(words)
    }

    pub const fn into_words(self) -> [u64; 32] {
        self.0
    }
}

/// The 32 physical COP1 Floating-Point General registers (FGRs).
///
/// VR4300 User's Manual section 5.2 defines each FGR as 32 bits wide when
/// Status.FR=0 and 64 bits wide when FR=1. Section 5.3 defines an FR=0
/// doubleword FPR as the low words of adjacent even/odd FGRs, while an FR=1
/// FPR is one complete 64-bit FGR. Retaining all 64 bits in every physical
/// FGR makes changing FR a view change: bits inaccessible in FR=0 survive and
/// become visible again when FR=1 is restored.
#[derive(Clone, Debug, Default)]
struct FprFile {
    fgr: [u64; 32],
}

impl FprFile {
    #[inline]
    fn word(&self, idx: u8) -> u32 {
        self.fgr[idx as usize] as u32
    }

    #[inline]
    fn set_word(&mut self, idx: u8, bits: u32) {
        let slot = &mut self.fgr[idx as usize];
        *slot = (*slot & 0xFFFF_FFFF_0000_0000) | u64::from(bits);
    }

    #[inline]
    fn doubleword(&self, idx: u8, fr: bool) -> u64 {
        if fr {
            self.fgr[idx as usize]
        } else {
            assert_eq!(idx & 1, 0, "FR=0 doubleword read from odd FPR f{idx}");
            u64::from(self.word(idx)) | (u64::from(self.word(idx + 1)) << 32)
        }
    }

    #[inline]
    fn set_doubleword(&mut self, idx: u8, bits: u64, fr: bool) {
        if fr {
            self.fgr[idx as usize] = bits;
        } else {
            assert_eq!(idx & 1, 0, "FR=0 doubleword write to odd FPR f{idx}");
            self.set_word(idx, bits as u32);
            self.set_word(idx + 1, (bits >> 32) as u32);
        }
    }

    fn physical_state(&self) -> PhysicalFgrState {
        PhysicalFgrState::from_words(self.fgr)
    }

    fn replace_physical_state(&mut self, state: PhysicalFgrState) {
        self.fgr = state.into_words();
    }
}
/// A precise COP1 operation requested guest floating-point exception entry.
/// The operation has already updated FCSR.Cause, but has not committed its
/// architectural destination or an enabled exception's sticky Flag bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FpuException;

/// One FCSR Cause/Enable/Flag lane. This is deliberately an enum rather than
/// a bit mask: [`RecompContext::record_fpu_exception`] is valid only for an
/// operation that raises one cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SingleFpuCause {
    Inexact,
    Invalid,
}

impl SingleFpuCause {
    const fn index(self) -> u8 {
        match self {
            Self::Inexact => 0,
            Self::Invalid => 4,
        }
    }
}

/// Pointer-free projection of every future-affecting field owned by one
/// [`RecompContext`].
///
/// The current execution destination and suspended host/coroutine
/// continuation are deliberately absent: neither is owned by
/// `RecompContext`. The bounded indirect-transfer history is also absent
/// because it is diagnostic-only and cannot affect later guest execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecompContextEvidenceSnapshotV1 {
    pub gprs: [u64; 32],
    pub hi: u64,
    pub lo: u64,
    pub physical_fgrs: [u64; 32],
    pub fpu_cond: bool,
    pub fcsr: u32,
    pub ll_reservation: Option<(u64, u8)>,
    pub cop0_count: u32,
    pub cop0_compare: u32,
    pub cop0_count_write: Option<u32>,
    pub cop0_compare_write: Option<u32>,
    pub cop0_cond: bool,
    pub cop0_status: u32,
    pub cop0_cause: u32,
    pub cop0_epc: u32,
    pub cop0_error_epc: u32,
    pub cop0_badvaddr: u64,
    pub cop0_context: u32,
    pub cop0_xcontext: u64,
    pub cop0_index: u32,
    pub tlb_entries: [TlbEntryRaw; 32],
    pub cop0_entry_lo0: u32,
    pub cop0_entry_lo1: u32,
    pub cop0_page_mask: u32,
    pub cop0_wired: u32,
    pub cop0_entry_hi: u64,
    pub cop0_random_phase: u32,
    pub cop0_watch_lo: u32,
    pub cop0_watch_hi: u32,
    pub os_interrupt_mask: u32,
    pub thread_return_pc: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct RecompContext {
    /// r[0] is `$zero`; kept in the array for uniform indexing but never
    /// observably nonzero (writes go through [`RecompContext::set_r`], which
    /// drops index 0).
    r: [u64; 32],
    /// The HI result register of MULT/DIV.
    pub hi: u64,
    /// The LO result register of MULT/DIV.
    pub lo: u64,

    /// Physical COP1 FGR state. Accessors select the FR=0 paired or FR=1
    /// independent view from `cop0_status`; no emitter or interpreter arm may
    /// open-code that mapping.
    fpr: FprFile,
    /// The FPU condition flag (FCSR bit 23). Set by the `C.cond.fmt` compares,
    /// tested by `BC1T`/`BC1F`. This is N64Recomp's per-function `c1cs`
    /// promoted to context state (equivalent: a compare always precedes the
    /// branch that reads it, so lifetime is irrelevant to the result).
    pub fpu_cond: bool,
    /// FCSR bits other than condition bit 23, which is kept in `fpu_cond` so
    /// generated branch code can read it directly. VR4300 User's Manual
    /// section 6.3.2.2 defines FS(24), Cause(17:12), Enables(11:7),
    /// Flags(6:2), and RM(1:0); reserved bits read as zero.
    fcsr: u32,
    /// Address/width of the most recent LL/LLD reservation. There is only one
    /// architectural LLbit. A mismatched SC/SCD must fail and clear it.
    ll_reservation: Option<(u64, u8)>,
    /// COP0 register 9, `Count`: the free-running cycle counter that backs
    /// `osGetCount`. It is the one COP0 read a recompiled body legitimately
    /// performs (`MFC0 rt, $9`); the host advances it. Modeled as real state
    /// rather than trapped, unlike the libultra-managed Status/Cause/EPC.
    pub cop0_count: u32,
    /// Half-rate phase imported from the live CPU clock at each execution
    /// boundary. This makes an interior MFC0 Count observe a preceding odd
    /// number of retired CPU cycles without transferring clock ownership.
    cop0_count_phase: u8,
    /// COP0 register 11, `Compare`: the timer-interrupt threshold written via
    /// `MTC0 rt, $11` on the `osSetTimer` path. Stored so the write round-trips;
    /// the interrupt it would schedule is the host's concern.
    pub cop0_compare: u32,
    /// Writes are handed to the live CPU clock authority at the next block
    /// boundary. Options retain same-value writes, which are observable for
    /// Compare because every write acknowledges IP7.
    cop0_count_write: Option<u32>,
    cop0_compare_write: Option<u32>,
    /// COP0 condition bit used by BC0*. On VR4300 this reflects Status.CH.
    /// CACHE tag operations are host-modeled, so callers that exercise BC0
    /// explicitly supply the observed condition through this field.
    pub cop0_cond: bool,
    /// COP0 Status (register 12). Privileged libultra entry points are
    /// host-bound, but their typed adapters still need per-OSThread status
    /// state for `__osGetSR`/`__osSetSR` and interrupt-mask round trips.
    pub cop0_status: u32,
    /// COP0 Cause (register 13). The coroutine executor delivers events at
    /// explicit yield points rather than synthesizing CPU exceptions, so the
    /// normal value is zero; keeping the field makes `__osGetCause` an honest
    /// state read instead of a fabricated constant.
    pub cop0_cause: u32,
    /// COP0 EPC (register 14), written on precise exception entry when EXL was
    /// clear. Branch-delay exceptions hold the branch PC, not the delay PC.
    pub cop0_epc: u32,
    /// COP0 ErrorEPC (register 30), selected by ERET while Status.ERL is set.
    pub cop0_error_epc: u32,
    /// COP0 BadVAddr (register 8). The arbitrary-PC lane populates the complete
    /// effective address for instruction-fetch AdEL, aligned-memory AdEL/AdES,
    /// and typed data TLB exceptions.
    pub cop0_badvaddr: u64,
    /// Low 32 bits of COP0 Context (register 4). TLB exceptions replace
    /// BadVPN2 while retaining the software-owned PTEBase field.
    pub cop0_context: u32,
    /// COP0 XContext (register 20). TLB exceptions replace Region/BadVPN2
    /// while retaining the software-owned 31-bit PTEBase field.
    pub cop0_xcontext: u64,
    /// COP0 TLB registers (Index 0, EntryLo0/1 2/3, PageMask 5, Wired 6,
    /// EntryHi 10). Boot-time unmap-all loops save/clear these; TLBWI/TLBWR
    /// record entries and TLBR/TLBP inspect them. Canonical 32-bit mapped data
    /// accesses translate through the same entries.
    pub cop0_index: u32,
    /// Raw recorded TLB entries (see `tlbwi_record`, `tlbwr_record`,
    /// `tlbr_read`, and `tlbp_probe`).
    pub tlb_entries: [TlbEntryRaw; 32],
    pub cop0_entry_lo0: u32,
    pub cop0_entry_lo1: u32,
    pub cop0_page_mask: u32,
    cop0_wired: u32,
    pub cop0_entry_hi: u64,
    /// Instruction-coupled position within the inclusive Random countdown
    /// `[31, Wired]`. Zero denotes Random=31, including after reset and every
    /// Wired write. Keeping the phase rather than a second writable register
    /// makes the lower bound structural.
    cop0_random_phase: u32,
    /// COP0 WatchLo/WatchHi (registers 18/19). Stored round-trip state only:
    /// SDK boot code writes 0 to disarm the watchpoint on the way up, and
    /// nothing in this runtime models the watch exception itself (a set
    /// watchpoint simply never fires).
    pub cop0_watch_lo: u32,
    pub cop0_watch_hi: u32,
    /// Libultra's combined CPU/RCP interrupt mask associated with this
    /// OSThread. CPU gating is mirrored into `cop0_status`; the packed value
    /// is retained so `osSetIntMask` returns this context's prior mask rather
    /// than another coroutine's last hardware setting.
    os_interrupt_mask: u32,
    /// Explicit host-installed return sentinel for an OSThread entry. A
    /// generated `jr`/`jalr` may finish the coroutine only when its captured
    /// target equals this value; address zero or an unmapped PC remains a
    /// loud guest fault.
    thread_return_pc: Option<u32>,
    /// Bounded diagnostic history. It never participates in guest execution,
    /// pack identity, or generation selection.
    indirect_transfers: Vec<IndirectTransferObservation>,
}

impl RecompContext {
    const INDIRECT_TRANSFER_HISTORY_LIMIT: usize = 64;

    /// A fresh context with all registers zeroed.
    pub fn new() -> Self {
        RecompContext::default()
    }

    /// Capture every future-affecting CPU field owned by this context.
    ///
    /// This does not claim to capture the execution destination or a native
    /// coroutine/host continuation; those live above `RecompContext` and must
    /// be paired with this projection by their respective owners.
    pub fn evidence_snapshot_v1(&self) -> RecompContextEvidenceSnapshotV1 {
        RecompContextEvidenceSnapshotV1 {
            gprs: self.r,
            hi: self.hi,
            lo: self.lo,
            physical_fgrs: self.fpr.physical_state().into_words(),
            fpu_cond: self.fpu_cond,
            fcsr: self.fcsr,
            ll_reservation: self.ll_reservation,
            cop0_count: self.cop0_count,
            cop0_compare: self.cop0_compare,
            cop0_count_write: self.cop0_count_write,
            cop0_compare_write: self.cop0_compare_write,
            cop0_cond: self.cop0_cond,
            cop0_status: self.cop0_status,
            cop0_cause: self.cop0_cause,
            cop0_epc: self.cop0_epc,
            cop0_error_epc: self.cop0_error_epc,
            cop0_badvaddr: self.cop0_badvaddr,
            cop0_context: self.cop0_context,
            cop0_xcontext: self.cop0_xcontext,
            cop0_index: self.cop0_index,
            tlb_entries: self.tlb_entries,
            cop0_entry_lo0: self.cop0_entry_lo0,
            cop0_entry_lo1: self.cop0_entry_lo1,
            cop0_page_mask: self.cop0_page_mask,
            cop0_wired: self.cop0_wired,
            cop0_entry_hi: self.cop0_entry_hi,
            cop0_random_phase: self.cop0_random_phase,
            cop0_watch_lo: self.cop0_watch_lo,
            cop0_watch_hi: self.cop0_watch_hi,
            os_interrupt_mask: self.os_interrupt_mask,
            thread_return_pc: self.thread_return_pc,
        }
    }

    /// Reconstruct the architectural CPU state observed at the
    /// IPL3-to-header-entry handoff.
    ///
    /// This intentionally does not touch host-only OSThread state, the return
    /// sentinel, or an LL reservation. IPL3 precedes libultra thread
    /// ownership, and the debugger wire has no authority to manufacture those
    /// runtime concepts.
    pub fn restore_boot_context(
        &mut self,
        boot: &crate::boot::BootContext,
    ) -> Result<(), crate::boot::BootContextError> {
        boot.validate()?;

        self.set_gprs(boot.gprs);
        self.hi = boot.hi;
        self.lo = boot.lo;

        let cp0 = &boot.cp0.registers;
        self.cop0_index = cp0[0] as u32;
        self.cop0_entry_lo0 = cp0[2] as u32;
        self.cop0_entry_lo1 = cp0[3] as u32;
        self.cop0_context = cp0[4] as u32;
        self.cop0_page_mask = cp0[5] as u32;
        self.cop0_wired = cp0[6] as u32;
        self.cop0_random_phase = 31 - cp0[1] as u32;
        self.cop0_badvaddr = cp0[8];
        self.cop0_count = cp0[9] as u32;
        self.cop0_entry_hi = cp0[10];
        self.cop0_compare = cp0[11] as u32;
        self.cop0_status = cp0[12] as u32;
        self.cop0_cause = cp0[13] as u32;
        self.cop0_epc = cp0[14] as u32;
        self.cop0_cond = self.cop0_status & (1 << 18) != 0;
        self.cop0_watch_lo = cp0[18] as u32;
        self.cop0_watch_hi = cp0[19] as u32;
        self.cop0_xcontext = cp0[20];
        self.cop0_error_epc = cp0[30] as u32;
        self.cop0_count_write = None;
        self.cop0_compare_write = None;
        self.ll_reservation = None;
        Ok(())
    }

    /// Compare the live architectural state with a captured boot handoff.
    ///
    /// Callers use this at the generated runner's first-entry boundary. The
    /// complete mismatch set is returned so a failed black-box comparison
    /// identifies every divergent field without rerunning private input.
    pub fn boot_context_state_mismatches(
        &self,
        boot: &crate::boot::BootContext,
    ) -> Result<Vec<crate::boot::BootContextStateMismatch>, crate::boot::BootContextError> {
        use crate::boot::{BootContextStateField as Field, BootContextStateMismatch as Mismatch};

        boot.validate()?;
        let mut mismatches = Vec::new();
        let mut compare = |field, expected, actual| {
            if expected != actual {
                mismatches.push(Mismatch {
                    field,
                    expected,
                    actual,
                });
            }
        };
        for register in 0..32u8 {
            compare(
                Field::Gpr(register),
                boot.gprs[register as usize],
                self.r(register),
            );
        }
        compare(Field::Hi, boot.hi, self.hi);
        compare(Field::Lo, boot.lo, self.lo);

        let cp0 = &boot.cp0.registers;
        for (register, actual) in [
            (0, u64::from(self.cop0_index)),
            (1, u64::from(self.cop0_random())),
            (2, u64::from(self.cop0_entry_lo0)),
            (3, u64::from(self.cop0_entry_lo1)),
            (4, u64::from(self.cop0_context)),
            (5, u64::from(self.cop0_page_mask)),
            (6, u64::from(self.cop0_wired)),
            (8, self.cop0_badvaddr),
            (9, u64::from(self.cop0_count)),
            (10, self.cop0_entry_hi),
            (11, u64::from(self.cop0_compare)),
            (12, u64::from(self.cop0_status)),
            (13, u64::from(self.cop0_cause)),
            (14, u64::from(self.cop0_epc)),
            (18, u64::from(self.cop0_watch_lo)),
            (19, u64::from(self.cop0_watch_hi)),
            (20, self.cop0_xcontext),
            (30, u64::from(self.cop0_error_epc)),
        ] {
            compare(Field::Cop0(register), cp0[register as usize], actual);
        }
        Ok(mismatches)
    }

    pub fn set_thread_return_pc(&mut self, pc: Option<u32>) {
        self.thread_return_pc = pc;
    }

    pub fn is_thread_return(&self, pc: u32) -> bool {
        self.thread_return_pc == Some(pc)
    }

    pub fn os_interrupt_mask(&self) -> u32 {
        self.os_interrupt_mask
    }

    pub fn replace_os_interrupt_mask(&mut self, mask: u32) -> u32 {
        std::mem::replace(&mut self.os_interrupt_mask, mask)
    }

    /// Refresh the block-local view from the live CPU clock without
    /// fabricating an architectural MTC0 write.
    pub fn synchronize_cop0_timing(&mut self, count: u32, count_phase: u8, compare: u32) {
        assert!(
            count_phase <= 1,
            "CP0 Count half-rate phase must be zero or one"
        );
        self.cop0_count = count;
        self.cop0_count_phase = count_phase;
        self.cop0_compare = compare;
    }

    /// Drain MTC0 Count/Compare writes for the live CPU clock authority.
    pub fn take_cop0_timing_writes(&mut self) -> (Option<u32>, Option<u32>) {
        (self.cop0_count_write.take(), self.cop0_compare_write.take())
    }

    /// Read GPR `idx` as a full 64-bit value. `$zero` reads 0.
    #[inline]
    pub fn r(&self, idx: u8) -> u64 {
        self.r[idx as usize]
    }

    /// Read GPR `idx` as a signed 32-bit value (the low word).
    #[inline]
    pub fn r_s32(&self, idx: u8) -> i32 {
        self.r[idx as usize] as u32 as i32
    }

    /// Read GPR `idx` as an unsigned 32-bit value (the low word).
    #[inline]
    pub fn r_u32(&self, idx: u8) -> u32 {
        self.r[idx as usize] as u32
    }

    /// Read GPR `idx` as a signed 64-bit value. This is the `SIGNED(reg)` /
    /// `ToS64` operand of the C oracle — MIPS III compares (SLT/SLTI, and the
    /// single-operand branches) operate on the full 64-bit register.
    #[inline]
    pub fn r_s64(&self, idx: u8) -> i64 {
        self.r[idx as usize] as i64
    }

    /// Read GPR `idx` as an unsigned 64-bit value (`ToU64`, for SLTU/SLTIU).
    #[inline]
    pub fn r_u64(&self, idx: u8) -> u64 {
        self.r[idx as usize]
    }

    /// Write a raw 64-bit value into GPR `idx`. Writes to `$zero` are dropped,
    /// upholding the hardwired-zero contract.
    #[inline]
    pub fn set_r(&mut self, idx: u8, val: u64) {
        if idx != 0 {
            self.r[idx as usize] = val;
        }
    }

    /// Snapshot all architectural GPRs for the audited rs/C ABI adapter.
    /// The returned copy preserves `$zero == 0` without exposing the backing
    /// array for unchecked mutation.
    pub fn gprs(&self) -> [u64; 32] {
        self.r
    }

    /// Restore a GPR snapshot after an fn64 host shim returns. `$zero` is
    /// forced back to zero even if a foreign ABI context contained garbage.
    pub fn set_gprs(&mut self, mut regs: [u64; 32]) {
        regs[0] = 0;
        self.r = regs;
    }

    /// Retain an issued `jr`/`jalr` at the last point before its destination
    /// enters the active-generation resolver.
    pub fn record_indirect_transfer(
        &mut self,
        source_bank: u64,
        source_pc: u32,
        source_register: u8,
        target_pc: u32,
        link_pc: Option<u32>,
    ) {
        if self.indirect_transfers.len() == Self::INDIRECT_TRANSFER_HISTORY_LIMIT {
            self.indirect_transfers.remove(0);
        }
        self.indirect_transfers.push(IndirectTransferObservation {
            source_bank,
            source_pc,
            source_register,
            target_pc,
            link_pc,
            gprs: self.gprs(),
            hi: self.hi,
            lo: self.lo,
            cop0_status: self.cop0_status,
            cop0_cause: self.cop0_cause,
            cop0_epc: self.cop0_epc,
        });
    }

    /// Exact retained order, oldest to newest.
    pub fn indirect_transfer_observations(&self) -> &[IndirectTransferObservation] {
        &self.indirect_transfers
    }

    /// Write a 32-bit result into GPR `idx`, sign-extending into the 64-bit
    /// register (the universal MIPS III rule for 32-bit ops: the result's
    /// bit 31 fills bits 63..32). This is the typed replacement for the C
    /// `S32(...)`/`ADD32(...)` casts.
    #[inline]
    pub fn set_r32(&mut self, idx: u8, val: i32) {
        self.set_r(idx, val as i64 as u64);
    }

    /// Read FCR0/FCR31. The VR4300 implements only those two control
    /// registers (User's Manual section 6.3.2); reserved FCR reads remain a
    /// loud, unverified host boundary.
    #[inline]
    pub fn read_fcr(&self, idx: u8) -> u32 {
        match idx {
            // VR4300 implementation number 0x0B, revision zero.
            0 => 0x0000_0B00,
            31 => (self.fcsr & !(1 << 23)) | ((self.fpu_cond as u32) << 23),
            _ => trap_unsupported(format!("reserved COP1 control register FCR{idx}")),
        }
    }

    /// Write FCR31. Writes to read-only FCR0 or reserved FCRs remain a loud,
    /// unverified host boundary. Reserved FCR31 bits are discarded rather than
    /// becoming hidden state.
    #[inline]
    pub fn write_fcr(&mut self, idx: u8, value: u32) {
        if idx != 31 {
            trap_unsupported(format!(
                "write to read-only/reserved COP1 control register FCR{idx}"
            ));
        }
        const WRITABLE: u32 = (1 << 24) | (1 << 23) | 0x0003_FFFF;
        self.fpu_cond = value & (1 << 23) != 0;
        self.fcsr = value & WRITABLE & !(1 << 23);
    }

    /// Whether the current FCSR value demands a precise floating-point
    /// exception. VR4300 User's Manual section 6.3.2.2 specifies that Cause.E is
    /// always enabled and each IEEE Cause bit traps when its matching Enable
    /// bit is set. CTC1 writes FCSR before this condition is observed.
    #[inline]
    pub fn fcsr_exception_pending(&self) -> bool {
        const CAUSE_E: u32 = 1 << 17;
        let ieee_causes = (self.fcsr >> 12) & 0x1f;
        let enables = (self.fcsr >> 7) & 0x1f;
        self.fcsr & CAUSE_E != 0 || ieee_causes & enables != 0
    }

    /// Establish the single architectural LLbit reservation.
    ///
    /// VR4300 User's Manual "Load Linked Address (LLAddr) Register" identifies
    /// LLAddr as diagnostic-only. Chapter 16's SC (pp. 486-488) and SCD
    /// (pp. 488-490) definitions consult only LLbit and declare a different
    /// address from the preceding LL/LLD undefined. fn64 therefore retains a
    /// bounded same-guest-address/same-width choice for that undefined domain;
    /// it is not a physical-alias or silicon-granule parity claim.
    #[inline]
    pub fn set_ll_reservation(&mut self, vaddr: u64, width: u8) {
        self.ll_reservation = Some((vaddr, width));
    }

    /// Test and clear the LLbit for SC/SCD under fn64's bounded policy above.
    #[inline]
    pub fn take_ll_reservation(&mut self, vaddr: u64, width: u8) -> bool {
        self.ll_reservation.take() == Some((vaddr, width))
    }

    /// Apply the VR4300 ERET state transition and return its virtual target.
    /// User's Manual section 6.3 specifies ErrorEPC/ERL precedence over
    /// EPC/EXL and clearing the architectural LLbit on exception return.
    #[inline]
    pub fn exception_return_pc(&mut self) -> u32 {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;

        self.ll_reservation = None;
        if self.cop0_status & STATUS_ERL != 0 {
            self.cop0_status &= !STATUS_ERL;
            self.cop0_error_epc
        } else {
            self.cop0_status &= !STATUS_EXL;
            self.cop0_epc
        }
    }

    /// `tlbwi`: record the indexed entry from the staged COP0 TLB registers.
    pub fn tlbwi_record(&mut self) {
        let index = (self.cop0_index & 31) as usize;
        self.tlb_entries[index] = TlbEntryRaw {
            page_mask: self.cop0_page_mask,
            entry_hi: self.cop0_entry_hi,
            entry_lo0: self.cop0_entry_lo0,
            entry_lo1: self.cop0_entry_lo1,
        };
    }

    /// Install the unique invalid 4 KiB entry layout established before
    /// libultra starts application OSThreads.
    ///
    /// Zeroing all raw entries is not an invalid TLB: it creates 32 matching
    /// VPN2/ASID entries for address zero, an architecturally undefined
    /// multiple-match condition. Distinct VPN2 values with V clear preserve
    /// the intended invalid/refill behavior without inventing a translation.
    pub fn initialize_invalid_tlb_entries(&mut self) {
        for (index, entry) in self.tlb_entries.iter_mut().enumerate() {
            *entry = TlbEntryRaw {
                page_mask: 0,
                entry_hi: (index as u64) << 13,
                entry_lo0: 0,
                entry_lo1: 0,
            };
        }
    }

    /// Current COP0 Random value. VR4300 User's Manual section 5.3.1 defines
    /// the inclusive range from 31 through Wired and the Wired-write reset to
    /// 31. Wired=31 therefore denotes the stable one-entry range containing 31.
    #[inline]
    pub fn cop0_random(&self) -> u32 {
        let span = 32u32
            .checked_sub(self.cop0_wired)
            .filter(|span| *span != 0)
            .unwrap_or_else(|| {
                trap_unsupported(format!(
                    "COP0 Wired value {} exceeds the 32-entry VR4300 TLB",
                    self.cop0_wired
                ))
            });
        31 - self.cop0_random_phase % span
    }

    /// Advance Random by fn64's charged guest-instruction units.
    ///
    /// The arbitrary-PC generated and interpreter lanes call this at their
    /// explicit instruction boundaries. This is a bounded deterministic clock
    /// policy, not a claim about the silicon cycle at which Random changes:
    /// an ordinary successful instruction advances once, a branch/delay pair
    /// advances twice, including the runner's charged unit for an annulled
    /// likely slot, and a faulting straight instruction does not advance.
    /// Whole-function execution has no such boundary and deliberately does
    /// not call it; TLBWR remains loud in that lane.
    #[inline]
    pub fn advance_cop0_random(&mut self, instructions: u32) {
        let span = 32u32
            .checked_sub(self.cop0_wired)
            .filter(|span| *span != 0)
            .unwrap_or_else(|| {
                trap_unsupported(format!(
                    "COP0 Wired value {} exceeds the 32-entry VR4300 TLB",
                    self.cop0_wired
                ))
            });
        self.cop0_random_phase = (self.cop0_random_phase + instructions % span) % span;
    }

    /// `tlbwr`: record the staged entry at the current Random index.
    ///
    /// Random is sampled before the TLBWR instruction itself advances the
    /// instruction clock, matching the ordinary read-before-retire ordering
    /// used by the arbitrary-PC lanes.
    pub fn tlbwr_record(&mut self) {
        let index = self.cop0_random() as usize;
        self.tlb_entries[index] = TlbEntryRaw {
            page_mask: self.cop0_page_mask,
            entry_hi: self.cop0_entry_hi,
            entry_lo0: self.cop0_entry_lo0,
            entry_lo1: self.cop0_entry_lo1,
        };
    }

    /// `tlbr`: load the staged COP0 registers from the indexed TLB entry.
    ///
    /// VR4300 User's Manual section 5.4.11 names exactly these four
    /// destinations. Index bit 5 and the probe-failure bit do not participate
    /// in the 32-entry array index.
    pub fn tlbr_read(&mut self) {
        let entry = self.tlb_entries[(self.cop0_index & 31) as usize];
        self.cop0_page_mask = entry.page_mask;
        self.cop0_entry_hi = entry.entry_hi;
        self.cop0_entry_lo0 = entry.entry_lo0;
        self.cop0_entry_lo1 = entry.entry_lo1;
    }

    /// `tlbp`: probe all recorded entries using VPN2/PageMask plus ASID or the
    /// entry's paired Global bits, and publish the result in COP0 Index.
    ///
    /// Valid and Dirty do not participate in a tag match. More than one match
    /// is architecturally undefined, so the deterministic runtime traps rather
    /// than selecting an arbitrary entry. On a miss the architecture leaves
    /// the low Index field unpredictable; fn64's bounded deterministic policy
    /// clears that field and sets only the probe-failure bit.
    pub fn tlbp_probe(&mut self) {
        const VPN2_MASK: u64 = 0xc000_00ff_ffff_e000;
        const PAGE_MASK: u64 = 0x01ff_e000;
        const ASID_MASK: u64 = 0x0000_00ff;
        const GLOBAL: u32 = 1;

        let probe = self.cop0_entry_hi;
        let mut matched = None;
        for (index, entry) in self.tlb_entries.iter().enumerate() {
            let compared_vpn = VPN2_MASK & !(u64::from(entry.page_mask) & PAGE_MASK);
            let vpn_matches = (probe ^ entry.entry_hi) & compared_vpn == 0;
            let global = entry.entry_lo0 & GLOBAL != 0 && entry.entry_lo1 & GLOBAL != 0;
            let asid_matches = (probe ^ entry.entry_hi) & ASID_MASK == 0;
            if vpn_matches && (global || asid_matches) && matched.replace(index).is_some() {
                trap_unsupported(
                    "TLBP found multiple matching entries; VR4300 behavior is undefined",
                );
            }
        }
        self.cop0_index = matched.map_or(1 << 31, |index| index as u32);
    }

    fn cpu_mode(&self) -> CpuMode {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;
        const STATUS_KSU_MASK: u32 = 0b11 << 3;

        if self.cop0_status & (STATUS_EXL | STATUS_ERL) != 0 {
            return CpuMode::Kernel;
        }
        match (self.cop0_status & STATUS_KSU_MASK) >> 3 {
            0 => CpuMode::Kernel,
            1 => CpuMode::Supervisor,
            2 => CpuMode::User,
            mode => trap_unsupported(format!(
                "reserved VR4300 Status.KSU={mode} cannot classify an address"
            )),
        }
    }

    /// Whether COP0 is usable for the current Status. EXL/ERL force kernel
    /// mode; ordinary KSU=Kernel is always authorized; User and Supervisor
    /// require CU0. This predicate is shared by emitted-bank and interpreter
    /// guards so privilege is checked before any COP0-specific effect.
    pub fn cop0_usable(&self) -> bool {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;
        const STATUS_KSU_MASK: u32 = 0b11 << 3;
        const STATUS_CU0: u32 = 1 << 28;

        self.cop0_status & (STATUS_EXL | STATUS_ERL) != 0
            || self.cop0_status & STATUS_KSU_MASK == 0
            || self.cop0_status & STATUS_CU0 != 0
    }

    /// Classify one effective address before translation.
    ///
    /// VR4300 User's Manual chapter 3, Tables 3-2 through 3-4, define the
    /// UX/SX/KX-selected user, supervisor, and kernel spaces. Kernel XKPHYS
    /// requires VA[58:32]=0 and supplies PA[31:0] directly; mapped 64-bit
    /// spaces implement VA[39:0] plus EntryHi.Region. The four sign-extended
    /// compatibility spaces retain the existing 32-bit behavior.
    fn classify_data_address(&self, vaddr: u64) -> Result<AddressRoute, ()> {
        const STATUS_ERL: u32 = 1 << 2;
        const STATUS_UX: u32 = 1 << 5;
        const STATUS_SX: u32 = 1 << 6;
        const STATUS_KX: u32 = 1 << 7;
        const LOW_40_MAX: u64 = 0x0000_00ff_ffff_ffff;
        const SUPERVISOR_64_START: u64 = 0x4000_0000_0000_0000;
        const SUPERVISOR_64_END: u64 = 0x4000_00ff_ffff_ffff;
        const KERNEL_64_START: u64 = 0xc000_0000_0000_0000;
        const KERNEL_64_END: u64 = 0xc000_00ff_7fff_ffff;

        let mode = self.cpu_mode();
        let extended = match mode {
            CpuMode::Kernel => self.cop0_status & STATUS_KX != 0,
            CpuMode::Supervisor => self.cop0_status & STATUS_SX != 0,
            CpuMode::User => self.cop0_status & STATUS_UX != 0,
        };
        if mode == CpuMode::Kernel && self.cop0_status & STATUS_ERL != 0 && vaddr <= 0x7fff_ffff {
            return Ok(AddressRoute::DirectPhysical(vaddr as u32));
        }
        if !extended {
            let upper = vaddr >> 32;
            let low = vaddr as u32;
            let compatibility = upper == 0 || (upper == u32::MAX as u64 && low & 0x8000_0000 != 0);
            if !compatibility {
                return Err(());
            }
            return match mode {
                CpuMode::User if low < 0x8000_0000 => Ok(AddressRoute::Mapped { extended: false }),
                CpuMode::Supervisor if low < 0x8000_0000 => {
                    Ok(AddressRoute::Mapped { extended: false })
                }
                CpuMode::Supervisor if (0xc000_0000..0xe000_0000).contains(&low) => {
                    Ok(AddressRoute::Mapped { extended: false })
                }
                CpuMode::Kernel if (0x8000_0000..0xc000_0000).contains(&low) => {
                    Ok(AddressRoute::DirectVirtual(vaddr))
                }
                CpuMode::Kernel => Ok(AddressRoute::Mapped { extended: false }),
                CpuMode::User | CpuMode::Supervisor => Err(()),
            };
        }

        match mode {
            CpuMode::User => (vaddr <= LOW_40_MAX)
                .then_some(AddressRoute::Mapped { extended: true })
                .ok_or(()),
            CpuMode::Supervisor => {
                if vaddr <= LOW_40_MAX
                    || (SUPERVISOR_64_START..=SUPERVISOR_64_END).contains(&vaddr)
                    || (0xffff_ffff_c000_0000..=0xffff_ffff_dfff_ffff).contains(&vaddr)
                {
                    Ok(AddressRoute::Mapped { extended: true })
                } else {
                    Err(())
                }
            }
            CpuMode::Kernel => {
                if vaddr <= LOW_40_MAX {
                    if self.cop0_status & STATUS_ERL != 0 {
                        return Err(());
                    }
                    return Ok(AddressRoute::Mapped { extended: true });
                }
                if (SUPERVISOR_64_START..=SUPERVISOR_64_END).contains(&vaddr) {
                    return Ok(AddressRoute::Mapped { extended: true });
                }
                if vaddr >> 62 == 0b10 {
                    return if vaddr & 0x07ff_ffff_0000_0000 == 0 {
                        Ok(AddressRoute::DirectPhysical(vaddr as u32))
                    } else {
                        Err(())
                    };
                }
                if (KERNEL_64_START..=KERNEL_64_END).contains(&vaddr)
                    || (0xffff_ffff_c000_0000..=u64::MAX).contains(&vaddr)
                {
                    return Ok(AddressRoute::Mapped { extended: true });
                }
                if (0xffff_ffff_8000_0000..=0xffff_ffff_bfff_ffff).contains(&vaddr) {
                    return Ok(AddressRoute::DirectVirtual(vaddr));
                }
                Err(())
            }
        }
    }

    /// Translate one VR4300 guest data address.
    ///
    /// Chapter 3 supplies the segment classifier above plus PageMask,
    /// EntryLo0/1, Region/VPN2, ASID/global, V, D, and PFN rules. Address-space
    /// and privilege failures return AdEL/AdES currency before any TLB lookup
    /// or memory side effect. Instruction fetch retains a separate 32-bit-PC
    /// boundary in [`Self::translate_instruction_address`].
    pub fn translate_data_address(
        &self,
        vaddr: u64,
        access: DataAccessKind,
    ) -> Result<TranslatedDataAddress, DataAccessError> {
        match self.translate_data_address_diagnostic(vaddr, access) {
            Ok(translated) => Ok(translated),
            Err(InstructionTranslationDiagnosticErrorV1::Access(error)) => Err(error),
            Err(InstructionTranslationDiagnosticErrorV1::InvalidPageMaskEncoding {
                index,
                page_mask_raw,
            }) => trap_unsupported(format!(
                "TLB entry {index} has unsupported VR4300 PageMask {page_mask_raw:#010x}"
            )),
            Err(InstructionTranslationDiagnosticErrorV1::MultipleTlbMatches {
                vaddr,
                ..
            }) => trap_unsupported(format!(
                "data translation for {vaddr:#018x} matched multiple TLB entries; VR4300 behavior is undefined"
            )),
        }
    }

    fn translate_data_address_diagnostic(
        &self,
        vaddr: u64,
        access: DataAccessKind,
    ) -> Result<TranslatedDataAddress, InstructionTranslationDiagnosticErrorV1> {
        const PAGE_MASK_BITS: u32 = 0x01ff_e000;
        const VPN2_32_BITS: u64 = 0x0000_0000_ffff_e000;
        const VPN2_64_BITS: u64 = 0x0000_00ff_ffff_e000;
        const REGION_BITS: u64 = 0xc000_0000_0000_0000;
        const ASID_BITS: u64 = 0xff;
        const GLOBAL: u32 = 1;
        const VALID: u32 = 1 << 1;
        const DIRTY: u32 = 1 << 2;

        let route = self.classify_data_address(vaddr).map_err(|()| {
            InstructionTranslationDiagnosticErrorV1::Access(DataAccessError::AddressError {
                vaddr,
                access,
            })
        })?;
        match route {
            AddressRoute::DirectVirtual(address) => {
                return Ok(TranslatedDataAddress::Direct(address));
            }
            AddressRoute::DirectPhysical(physical) => {
                return Ok(TranslatedDataAddress::DirectPhysical(physical));
            }
            AddressRoute::Mapped { .. } => {}
        }
        let AddressRoute::Mapped { extended } = route else {
            unreachable!("direct routes returned above")
        };
        let low = vaddr as u32;

        let mut matched = None;
        for (index, entry) in self.tlb_entries.iter().copied().enumerate() {
            let page_mask = entry.page_mask & PAGE_MASK_BITS;
            if !matches!(
                page_mask,
                0 | 0x0000_6000
                    | 0x0001_e000
                    | 0x0007_e000
                    | 0x001f_e000
                    | 0x007f_e000
                    | 0x01ff_e000
            ) {
                return Err(
                    InstructionTranslationDiagnosticErrorV1::InvalidPageMaskEncoding {
                        index,
                        page_mask_raw: entry.page_mask,
                    },
                );
            }
            let compared_vpn = if extended {
                (VPN2_64_BITS & !(u64::from(page_mask))) | REGION_BITS
            } else {
                VPN2_32_BITS & !(u64::from(page_mask))
            };
            let vpn_matches = (vaddr ^ entry.entry_hi) & compared_vpn == 0;
            let global = entry.entry_lo0 & GLOBAL != 0 && entry.entry_lo1 & GLOBAL != 0;
            let asid_matches = (self.cop0_entry_hi ^ entry.entry_hi) & ASID_BITS == 0;
            if vpn_matches && (global || asid_matches) {
                if let Some((first_index, _)) = matched {
                    return Err(
                        InstructionTranslationDiagnosticErrorV1::MultipleTlbMatches {
                            vaddr,
                            first_index,
                            second_index: index,
                        },
                    );
                }
                matched = Some((index, entry));
            }
        }

        let Some((_index, entry)) = matched else {
            return Err(InstructionTranslationDiagnosticErrorV1::Access(
                DataAccessError::Tlb(TlbFault {
                    vaddr,
                    access,
                    kind: TlbFaultKind::Refill,
                    extended,
                }),
            ));
        };
        let page_mask = entry.page_mask & PAGE_MASK_BITS;
        let page_size = (page_mask + 0x2000) >> 1;
        let entry_lo = if low & page_size == 0 {
            entry.entry_lo0
        } else {
            entry.entry_lo1
        };
        if entry_lo & VALID == 0 {
            return Err(InstructionTranslationDiagnosticErrorV1::Access(
                DataAccessError::Tlb(TlbFault {
                    vaddr,
                    access,
                    kind: TlbFaultKind::Invalid,
                    extended,
                }),
            ));
        }
        if access == DataAccessKind::Store && entry_lo & DIRTY == 0 {
            return Err(InstructionTranslationDiagnosticErrorV1::Access(
                DataAccessError::Tlb(TlbFault {
                    vaddr,
                    access,
                    kind: TlbFaultKind::Modified,
                    extended,
                }),
            ));
        }

        // User's Manual figure 3-10 defines the VR4300 EntryLo PFN as the 20
        // bits 25:6 and bits 31:26 as zero. The accompanying LLAddr text
        // confirms that PA(31) is this processor's most-significant physical
        // address bit (unlike the 36-bit VR4000). Keep the complete 32-bit
        // result here; the backing boundary below rejects rather than aliases
        // physical addresses outside the N64's 29-bit direct window.
        let physical_page = ((entry_lo & 0x03ff_ffc0) << 6) & !(page_size - 1);
        Ok(TranslatedDataAddress::Mapped(
            physical_page | (low & (page_size - 1)),
        ))
    }

    /// Translate one canonical 32-bit architectural instruction address to
    /// physical instruction-word identity.
    ///
    /// VR4300 User's Manual chapter 3 supplies the same KSEG0/KSEG1 direct and
    /// KUSEG/KSSEG/KSEG3 TLB geometry used by data translation. Fetch differs
    /// at the result boundary: callers receive only the physical address and
    /// must retain the architectural VA separately for branch/link/EPC state.
    /// The currently unsupported 64-bit address spaces and non-kernel
    /// privilege modes trap here instead of being approximated.
    pub fn translate_instruction_address(
        &self,
        vaddr: u64,
    ) -> Result<TranslatedInstructionAddress, DataAccessError> {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        if upper != 0 && !(upper == u32::MAX as u64 && low & 0x8000_0000 != 0) {
            trap_unsupported(format!(
                "64-bit instruction address translation is unsupported for {vaddr:#018x}"
            ));
        }

        match self.translate_data_address(vaddr, DataAccessKind::Load)? {
            TranslatedDataAddress::Direct(_) => {
                Ok(TranslatedInstructionAddress::new(low & 0x1fff_ffff))
            }
            TranslatedDataAddress::DirectPhysical(physical) => {
                Ok(TranslatedInstructionAddress::new(physical))
            }
            TranslatedDataAddress::Mapped(physical) => {
                Ok(TranslatedInstructionAddress::new(physical))
            }
        }
    }

    /// Translate one 32-bit instruction address for diagnostic tooling.
    /// Unlike [`Self::translate_instruction_address`], architecturally
    /// undefined TLB inputs are returned as typed blockers rather than loud
    /// execution traps.
    pub fn translate_instruction_address_diagnostic_v1(
        &self,
        vaddr: u32,
    ) -> Result<TranslatedInstructionAddress, InstructionTranslationDiagnosticErrorV1> {
        match self.translate_data_address_diagnostic(u64::from(vaddr), DataAccessKind::Load)? {
            TranslatedDataAddress::Direct(_) => {
                Ok(TranslatedInstructionAddress::new(vaddr & 0x1fff_ffff))
            }
            TranslatedDataAddress::DirectPhysical(physical)
            | TranslatedDataAddress::Mapped(physical) => {
                Ok(TranslatedInstructionAddress::new(physical))
            }
        }
    }

    #[inline]
    pub fn read_cop0(&self, reg: u8) -> u32 {
        match reg {
            1 => self.cop0_random(),
            4 => self.cop0_context,
            8 => self.cop0_badvaddr as u32,
            9 => self.cop0_count,
            11 => self.cop0_compare,
            12 => self.cop0_status,
            13 => self.cop0_cause,
            14 => self.cop0_epc,
            0 => self.cop0_index,
            2 => self.cop0_entry_lo0,
            3 => self.cop0_entry_lo1,
            5 => self.cop0_page_mask,
            6 => self.cop0_wired,
            10 => self.cop0_entry_hi as u32,
            18 => self.cop0_watch_lo,
            19 => self.cop0_watch_hi,
            20 => self.cop0_xcontext as u32,
            30 => self.cop0_error_epc,
            _ => trap_unsupported(format!("unsupported MFC0 from COP0 register {reg}")),
        }
    }

    /// `MFC0 $9` (Count) with in-block interior visibility.
    ///
    /// # The boundary-authority contract this must not violate
    ///
    /// `self.cop0_count` is refreshed ONLY at block/checkpoint boundaries,
    /// from the live `fn64-runtime` `Executor`'s authoritative Count
    /// (`run_block_program`'s `ctx.synchronize_cop0_timing` call in
    /// `fn64-abi`, BEFORE that block's instructions run). The executor's own
    /// advance for the instructions THIS block is about to retire happens
    /// strictly AFTER the block returns, driven by its retired-instruction
    /// count (`Yield::InstructionCheckpoint` -> `Executor::advance_time`,
    /// which adds `retired_total / 2` — respecting an odd-cycle carry
    /// (the executor-owned `cp0_count_phase`). So a plain
    /// `self.cop0_count` read mid-block sees the value from block ENTRY,
    /// stale by however many instructions have already retired THIS turn.
    ///
    /// This method adds that missing interior delta to the RETURNED value
    /// only: `retired_since_entry / 2`, at the same half-CPU-rate the
    /// executor uses. It never mutates `self.cop0_count` — the boundary sync
    /// remains the sole writer, so the authoritative post-block advance
    /// (computed independently by the executor from the SAME retired count)
    /// is applied exactly once, at the boundary, regardless of what this
    /// method returned meanwhile. No cycle is double-counted because this
    /// method counts nothing into persistent state; it only offsets a read.
    ///
    /// The live executor supplies its retained half-rate phase at every
    /// boundary. This view therefore agrees with the authoritative clock for
    /// every interior instruction count while remaining a read-only offset;
    /// the executor still performs the sole persistent post-block advance.
    #[inline]
    pub fn read_cop0_count_interior(&self, retired_since_entry: u32) -> u32 {
        self.cop0_count
            .wrapping_add((u32::from(self.cop0_count_phase) + retired_since_entry) / 2)
    }

    /// Read one architecturally 64-bit COP0 address register for DMFC0.
    #[inline]
    pub fn read_cop0_64(&self, reg: u8) -> u64 {
        match reg {
            8 => self.cop0_badvaddr,
            10 => self.cop0_entry_hi,
            20 => self.cop0_xcontext,
            _ => trap_unsupported(format!("unsupported DMFC0 from COP0 register {reg}")),
        }
    }

    /// Write a modeled 32-bit COP0 register for MTC0. Cause permits only the
    /// two software-pending bits; hardware pending lines remain owned by the
    /// device/clock layer. Status is context state and is replaced as one
    /// architectural register so interrupt gating changes at the next block
    /// boundary.
    #[inline]
    pub fn write_cop0(&mut self, reg: u8, value: u32) {
        match reg {
            9 => {
                self.cop0_count = value;
                self.cop0_count_write = Some(value);
            }
            11 => {
                self.cop0_compare = value;
                self.cop0_compare_write = Some(value);
                self.cop0_cause &= !crate::execution::CpuInterruptLine::TIMER.cause_bit();
            }
            12 => self.cop0_status = value,
            13 => {
                const SOFTWARE_IP: u32 = 0b11 << 8;
                self.cop0_cause = (self.cop0_cause & !SOFTWARE_IP) | (value & SOFTWARE_IP);
            }
            14 => self.cop0_epc = value,
            0 => self.cop0_index = value,
            2 => self.cop0_entry_lo0 = value,
            3 => self.cop0_entry_lo1 = value,
            4 => self.cop0_context = value,
            5 => self.cop0_page_mask = value,
            6 => {
                if value > 31 {
                    trap_unsupported(format!(
                        "COP0 Wired value {value} exceeds the 32-entry VR4300 TLB"
                    ));
                }
                self.cop0_wired = value;
                self.cop0_random_phase = 0;
            }
            10 => self.cop0_entry_hi = u64::from(value),
            18 => self.cop0_watch_lo = value,
            19 => self.cop0_watch_hi = value,
            30 => self.cop0_error_epc = value,
            _ => trap_unsupported(format!("unsupported MTC0 to COP0 register {reg}")),
        }
    }

    /// Write one architecturally 64-bit COP0 address register for DMTC0.
    #[inline]
    pub fn write_cop0_64(&mut self, reg: u8, value: u64) {
        match reg {
            10 => self.cop0_entry_hi = value,
            20 => self.cop0_xcontext = value,
            _ => trap_unsupported(format!("unsupported DMTC0 to COP0 register {reg}")),
        }
    }

    /// VR4300 signed word division, including the implementation-defined
    /// divide-by-zero results documented in User's Manual appendix D.2.
    pub fn div_s32(&mut self, dividend: i32, divisor: i32) {
        if divisor == 0 {
            self.lo = if dividend < 0 {
                0xFFFF_FFFF_8000_0001
            } else {
                0x0000_0000_7FFF_FFFF
            };
            self.hi = dividend as i64 as u64;
        } else {
            self.lo = dividend.wrapping_div(divisor) as i64 as u64;
            self.hi = dividend.wrapping_rem(divisor) as i64 as u64;
        }
    }

    /// VR4300 unsigned word division. Word HI/LO results are sign-extended,
    /// including the all-ones quotient on divide by zero.
    pub fn div_u32(&mut self, dividend: u32, divisor: u32) {
        if let Some(quotient) = dividend.checked_div(divisor) {
            self.lo = quotient as i32 as i64 as u64;
            self.hi = (dividend % divisor) as i32 as i64 as u64;
        } else {
            self.lo = u64::MAX;
            self.hi = (dividend as i32) as i64 as u64;
        }
    }

    /// Signed doubleword division. INT64_MIN/-1 produces the architectural
    /// wrapped quotient and zero remainder. The public VR4300 appendix prints
    /// only word-sized divide-by-zero results, so DDIV-by-zero traps loudly
    /// rather than inventing a 64-bit constant.
    pub fn div_s64(&mut self, dividend: i64, divisor: i64) {
        assert_ne!(
            divisor, 0,
            "DDIV by zero: result is not specified by the public VR4300 manual"
        );
        if dividend == i64::MIN && divisor == -1 {
            self.lo = dividend as u64;
            self.hi = 0;
        } else {
            self.lo = dividend.wrapping_div(divisor) as u64;
            self.hi = dividend.wrapping_rem(divisor) as u64;
        }
    }

    /// Unsigned doubleword division. See [`RecompContext::div_s64`] for why a
    /// zero divisor is a loud uncertainty trap.
    pub fn div_u64(&mut self, dividend: u64, divisor: u64) {
        assert_ne!(
            divisor, 0,
            "DDIVU by zero: result is not specified by the public VR4300 manual"
        );
        self.lo = dividend / divisor;
        self.hi = dividend % divisor;
    }

    fn try_fixed_to_float_raw(
        &mut self,
        value: i64,
        format: FixedFloatFormat,
        signed_56_source: bool,
    ) -> Result<u64, FpuException> {
        const SIGNED_56_MIN: i64 = -(1i64 << 55);
        const SIGNED_56_MAX: i64 = (1i64 << 55) - 1;

        self.fcsr &= !(0x3F << 12);
        if signed_56_source && !(SIGNED_56_MIN..=SIGNED_56_MAX).contains(&value) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let (bits, inexact) = encode_fixed_float(value, format, (self.fcsr & 3) as u8);
        if inexact {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(bits)
    }

    /// Exact typed CVT.S.W result. The source is the low word of `fs`; `fd`
    /// remains unmodified until the caller commits this immutable bit result.
    pub fn try_cvt_s_w_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        self.try_fixed_to_float_raw(
            i64::from(self.f_bits(fs) as i32),
            FixedFloatFormat::Single,
            false,
        )
        .map(|bits| bits as u32)
    }

    /// Exact typed CVT.D.W result.
    pub fn try_cvt_d_w_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        self.try_fixed_to_float_raw(
            i64::from(self.f_bits(fs) as i32),
            FixedFloatFormat::Double,
            false,
        )
    }

    /// Exact typed CVT.S.L result. VR4300's L-format conversion accepts only
    /// values representable as a signed 56-bit integer; other sources raise
    /// the always-enabled Unimplemented Operation cause.
    pub fn try_cvt_s_l_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        self.try_fixed_to_float_raw(self.d_bits(fs) as i64, FixedFloatFormat::Single, true)
            .map(|bits| bits as u32)
    }

    /// Exact typed CVT.D.L result with the same signed-56 admission rule.
    pub fn try_cvt_d_l_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        self.try_fixed_to_float_raw(self.d_bits(fs) as i64, FixedFloatFormat::Double, true)
    }

    fn whole_function_fixed_to_float<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => {
                trap_unsupported("enabled COP1 fixed-to-float exception in whole-function lane")
            }
        }
    }

    pub fn cvt_s_w_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_w_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_d_w_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_w_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_s_l_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_l_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    pub fn cvt_d_l_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_l_bits(fs);
        Self::whole_function_fixed_to_float(result)
    }

    fn fpu_unimplemented(&mut self) -> FpuException {
        self.fcsr &= !(0x3F << 12);
        self.fcsr |= 1 << 17;
        FpuException
    }

    /// Exact CVT.D.S result. VR4300 treats a denormal or legacy QNaN operand
    /// as Unimplemented; an SNaN raises Invalid and otherwise produces the
    /// MIPS-IV canonical double QNaN. Every finite normal single widens
    /// exactly, so FCSR.RM is immaterial.
    pub fn try_cvt_d_s_bits(&mut self, fs: u8) -> Result<u64, FpuException> {
        const D_QNAN: u64 = 0x7FF7_FFFF_FFFF_FFFF;
        let bits = self.f_bits(fs);
        let sign = u64::from(bits >> 31) << 63;
        let exponent = (bits >> 23) & 0xFF;
        let fraction = bits & 0x007F_FFFF;
        self.fcsr &= !(0x3F << 12);

        if exponent == 0 {
            return if fraction == 0 {
                Ok(sign)
            } else {
                Err(self.fpu_unimplemented())
            };
        }
        if exponent == 0xFF {
            return if fraction == 0 {
                Ok(sign | 0x7FF0_0000_0000_0000)
            } else if is_snan32(bits) {
                self.record_fpu_exception(SingleFpuCause::Invalid)?;
                Ok(D_QNAN)
            } else {
                Err(self.fpu_unimplemented())
            };
        }

        let double_exponent = u64::from(exponent + (1023 - 127)) << 52;
        Ok(sign | double_exponent | (u64::from(fraction) << 29))
    }

    /// Exact CVT.S.D result using integer IEEE decoding and FCSR.RM. VR4300
    /// detects tininess after rounding. A denormal result is supported only
    /// when FS is set and U/I are both disabled; that path flushes to signed
    /// zero or signed minimum-normal and raises U+I together.
    pub fn try_cvt_s_d_bits(&mut self, fs: u8) -> Result<u32, FpuException> {
        const S_QNAN: u32 = 0x7FBF_FFFF;
        const S_MAX: u32 = 0x7F7F_FFFF;
        const S_INFINITY: u32 = 0x7F80_0000;
        const S_MIN_NORMAL: u32 = 0x0080_0000;
        const FCSR_FS: u32 = 1 << 24;
        let bits = self.d_bits(fs);
        let negative = bits >> 63 != 0;
        let sign = (bits >> 32) as u32 & 0x8000_0000;
        let exponent = ((bits >> 52) & 0x7FF) as u32;
        let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
        self.fcsr &= !(0x3F << 12);

        if exponent == 0 {
            return if fraction == 0 {
                Ok(sign)
            } else {
                Err(self.fpu_unimplemented())
            };
        }
        if exponent == 0x7FF {
            return if fraction == 0 {
                Ok(sign | S_INFINITY)
            } else if is_snan64(bits) {
                self.record_fpu_exception(SingleFpuCause::Invalid)?;
                Ok(S_QNAN)
            } else {
                Err(self.fpu_unimplemented())
            };
        }

        let mode = (self.fcsr & 3) as u8;
        let unbiased = exponent as i32 - 1023;
        let significand = (1u64 << 52) | fraction;
        if unbiased > 127 {
            self.record_fpu_exceptions(FPU_CAUSE_O | FPU_CAUSE_I)?;
            return Ok(sign | overflowed_single(mode, negative, S_MAX, S_INFINITY));
        }

        if unbiased >= -126 {
            let (mut rounded, inexact) = round_shift_right(significand, 29, mode, negative);
            let mut output_exponent = unbiased;
            if rounded == 1 << 24 {
                rounded >>= 1;
                output_exponent += 1;
            }
            if output_exponent > 127 {
                self.record_fpu_exceptions(FPU_CAUSE_O | FPU_CAUSE_I)?;
                return Ok(sign | overflowed_single(mode, negative, S_MAX, S_INFINITY));
            }
            if inexact {
                self.record_fpu_exception(SingleFpuCause::Inexact)?;
            }
            return Ok(sign
                | (((output_exponent + 127) as u32) << 23)
                | (rounded as u32 & 0x007F_FFFF));
        }

        let shift = (-unbiased - 97) as u32;
        let (rounded, inexact) = round_shift_right(significand, shift, mode, negative);
        if rounded == 1 << 23 {
            if inexact {
                self.record_fpu_exception(SingleFpuCause::Inexact)?;
            }
            return Ok(sign | S_MIN_NORMAL);
        }

        let enables = ((self.fcsr >> 7) & 0x1F) as u8;
        if self.fcsr & FCSR_FS == 0 || enables & (FPU_CAUSE_U | FPU_CAUSE_I) != 0 {
            return Err(self.fpu_unimplemented());
        }
        self.record_fpu_exceptions(FPU_CAUSE_U | FPU_CAUSE_I)?;
        let magnitude = match (mode, negative) {
            (2, false) | (3, true) => S_MIN_NORMAL,
            _ => 0,
        };
        Ok(sign | magnitude)
    }

    fn whole_function_float_to_float<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => trap_unsupported("COP1 float-to-float exception in whole-function lane"),
        }
    }

    pub fn cvt_d_s_bits(&mut self, fs: u8) -> u64 {
        let result = self.try_cvt_d_s_bits(fs);
        Self::whole_function_float_to_float(result)
    }

    pub fn cvt_s_d_bits(&mut self, fs: u8) -> u32 {
        let result = self.try_cvt_s_d_bits(fs);
        Self::whole_function_float_to_float(result)
    }

    fn try_fpu_to_i32_raw(
        &mut self,
        value: f64,
        signaling_nan: bool,
        unimplemented_operand: bool,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        self.fcsr &= !(0x3F << 12);
        if signaling_nan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
            return Ok(i32::MAX);
        }
        if unimplemented_operand {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let rounded = self.rounded_for_mode(value, fixed_mode);
        if !(-2_147_483_648.0..2_147_483_648.0).contains(&rounded) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        if rounded != value {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(rounded as i32)
    }

    fn try_fpu_to_i64_raw(
        &mut self,
        value: f64,
        signaling_nan: bool,
        unimplemented_operand: bool,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        self.fcsr &= !(0x3F << 12);
        if signaling_nan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
            return Ok(i64::MAX);
        }
        if unimplemented_operand {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        let rounded = self.rounded_for_mode(value, fixed_mode);
        if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&rounded) {
            self.fcsr |= 1 << 17;
            return Err(FpuException);
        }
        if rounded != value {
            self.record_fpu_exception(SingleFpuCause::Inexact)?;
        }
        Ok(rounded as i64)
    }

    /// Convert a raw single-precision operand to a W result. The immutable
    /// typed result is returned before the caller commits `fd`.
    pub fn try_fpu_to_i32_s(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        let bits = self.f_bits(fs);
        self.try_fpu_to_i32_raw(
            f32::from_bits(bits) as f64,
            is_snan32(bits),
            is_qnan32(bits) || is_subnormal32(bits) || f32::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Double-precision counterpart of [`RecompContext::try_fpu_to_i32_s`].
    pub fn try_fpu_to_i32_d(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i32, FpuException> {
        let bits = self.d_bits(fs);
        self.try_fpu_to_i32_raw(
            f64::from_bits(bits),
            is_snan64(bits),
            is_qnan64(bits) || is_subnormal64(bits) || f64::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Convert a raw single-precision operand to an L result.
    pub fn try_fpu_to_i64_s(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        let bits = self.f_bits(fs);
        self.try_fpu_to_i64_raw(
            f32::from_bits(bits) as f64,
            is_snan32(bits),
            is_qnan32(bits) || is_subnormal32(bits) || f32::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    /// Double-precision counterpart of [`RecompContext::try_fpu_to_i64_s`].
    pub fn try_fpu_to_i64_d(
        &mut self,
        fs: u8,
        fixed_mode: Option<u8>,
    ) -> Result<i64, FpuException> {
        let bits = self.d_bits(fs);
        self.try_fpu_to_i64_raw(
            f64::from_bits(bits),
            is_snan64(bits),
            is_qnan64(bits) || is_subnormal64(bits) || f64::from_bits(bits).is_infinite(),
            fixed_mode,
        )
    }

    fn whole_function_conversion<T>(result: Result<T, FpuException>) -> T {
        match result {
            Ok(value) => value,
            Err(_) => {
                trap_unsupported("enabled COP1 float-to-fixed exception in whole-function lane")
            }
        }
    }

    pub fn fpu_to_i32_s(&mut self, fs: u8, fixed_mode: Option<u8>) -> i32 {
        let result = self.try_fpu_to_i32_s(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i32_d(&mut self, fs: u8, fixed_mode: Option<u8>) -> i32 {
        let result = self.try_fpu_to_i32_d(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i64_s(&mut self, fs: u8, fixed_mode: Option<u8>) -> i64 {
        let result = self.try_fpu_to_i64_s(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    pub fn fpu_to_i64_d(&mut self, fs: u8, fixed_mode: Option<u8>) -> i64 {
        let result = self.try_fpu_to_i64_d(fs, fixed_mode);
        Self::whole_function_conversion(result)
    }

    fn rounded_for_mode(&self, value: f64, fixed_mode: Option<u8>) -> f64 {
        match fixed_mode.unwrap_or((self.fcsr & 3) as u8) {
            0 => value.round_ties_even(),
            1 => value.trunc(),
            2 => value.ceil(),
            3 => value.floor(),
            _ => unreachable!("FCSR.RM and fixed rounding modes are two bits"),
        }
    }

    /// Record one precise IEEE exception after the operation has cleared its
    /// per-operation Cause field. VR4300 User's Manual section 6.3.2.2: Cause
    /// is set in either case; an enabled exception traps without changing Flag,
    /// while a disabled exception completes and accumulates the sticky Flag.
    /// This helper is single-cause-only. A future operation with several
    /// simultaneous causes must set every Cause, test all matching Enables,
    /// and set no new Flags if any cause is enabled; sequential calls here
    /// would incorrectly commit a disabled cause's Flag before a later enabled
    /// cause is observed.
    #[inline]
    fn record_fpu_exception(&mut self, exception: SingleFpuCause) -> Result<(), FpuException> {
        self.record_fpu_exceptions(1 << exception.index())
    }

    fn record_fpu_exceptions(&mut self, exceptions: u8) -> Result<(), FpuException> {
        assert_eq!(exceptions & !0x1F, 0, "IEEE FPU cause mask exceeds VZOUI");
        self.fcsr |= u32::from(exceptions) << 12;
        if ((self.fcsr >> 7) as u8) & exceptions != 0 {
            Err(FpuException)
        } else {
            self.fcsr |= u32::from(exceptions) << 2;
            Ok(())
        }
    }

    /// Fold the IEEE conditions the soft-float shim reported into FCSR and decide
    /// whether the op traps, returning `true` when an ENABLED exception fired.
    ///
    /// The VR4300 (User's Manual section 6.6, "Floating-Point Exceptions")
    /// distinguishes two outcomes per operation, and the FCSR update differs:
    ///
    /// * **Trapped** — any raised condition whose FCSR Enable bit is set. The
    ///   FPU writes the FCSR **Cause** field (so the handler can read which
    ///   condition trapped) but leaves the sticky **Flags** field and the
    ///   destination register **unchanged**, then vectors to the ExcCode-15
    ///   general exception. The caller must NOT commit the computed result.
    /// * **Not trapped** — no enabled condition fired. The FPU writes both the
    ///   Cause field and ORs the sticky Flags bits, and the destination register
    ///   takes the computed result.
    ///
    /// The Cause field is fully rewritten every operation (cleared first, exactly
    /// as [`RecompContext::round_for_mode`] does for conversions); the sticky
    /// Flags bits are only OR-ed in on the not-trapped path.
    #[inline]
    fn apply_fpu_flags(&mut self, flags: crate::fpu::Flags) -> bool {
        // Assemble the Cause bits this op signalled. The five IEEE conditions
        // occupy Cause 16:12 (index 0..4); the Unimplemented Operation (E) bit
        // is Cause bit 17 (index 5). The full 6-bit Cause field is 17:12.
        let ieee = u32::from(flags.inexact)
            | (u32::from(flags.underflow) << 1)
            | (u32::from(flags.overflow) << 2)
            | (u32::from(flags.divbyzero) << 3)
            | (u32::from(flags.invalid) << 4);
        let cause = ieee | (u32::from(flags.unimplemented) << 5);

        // A trap fires iff any IEEE condition whose Enable bit (FCSR 11:7) is
        // set was signalled, OR the Unimplemented Operation bit is set. E has
        // NO Enable bit and is UNMASKABLE — it always vectors to ExcCode 15
        // (VR4300 User's Manual section 7.5). Enables never gate it.
        let enables = (self.fcsr >> 7) & 0x1F;
        let trapped = (ieee & enables != 0) || flags.unimplemented;

        // Cause is rewritten unconditionally (clear the old 17:12 field, install
        // the freshly signalled conditions) so the handler sees exactly what
        // this op raised, on both the trapped and untrapped paths.
        self.fcsr = (self.fcsr & !(0x3F << 12)) | (cause << 12);

        if !trapped {
            // No exception fired: accumulate the sticky Flag bits (6:2). E has
            // no sticky Flag bit (only bits 6:2 exist), and it always traps, so
            // it is never reached here — the IEEE bits are the only sticky ones.
            self.fcsr |= ieee << 2;
        }
        trapped
    }

    /// Read the two-bit FCSR rounding mode (RM) field.
    #[inline]
    fn fcsr_rm(&self) -> u8 {
        (self.fcsr & 3) as u8
    }

    // --- COP1 arithmetic routed through the IEEE soft-float shim (`fpu`). ---
    //
    // Each reads the operand bits, performs the op under FCSR.RM in `crate::fpu`
    // (host-independent, IEEE-exact), then folds the returned IEEE flags into
    // FCSR via `apply_fpu_flags`, which reports whether an ENABLED exception
    // fired.
    //
    // # Result-commit ordering (the enabled-exception rule)
    //
    // On an enabled FP exception the VR4300 traps BEFORE writing the destination
    // register (User's Manual section 6.6): the result is discarded and only the
    // FCSR Cause field records the condition. So these methods compute first,
    // fold the flags, and write the destination ONLY when no trap fired. Each
    // returns `true` when it trapped — the emitted block lane checks that return
    // and exits to the ExcCode-15 fault handler (exactly as the integer-overflow
    // lane checks `checked_add` and exits to the IntegerOverflow handler). The
    // straight-line / whole-function lane instead panics loudly on a trap
    // (mirroring the `.expect("MIPS ADD integer overflow")` shape), since that
    // lane has no exception-return ABI yet.
    //
    // Note the operand bits are sampled BEFORE the destination is written, so an
    // in-place `fd == fs`/`fd == ft` op reads the original inputs even on the
    // committed (non-trapping) path.

    /// ADD.S: `fd = fs + ft` honoring FCSR.RM, with IEEE flags. Returns `true` if
    /// an enabled exception trapped (destination left unwritten).
    #[inline]
    #[must_use]
    pub fn fpu_add_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::add_s(self.f_bits(fs), self.f_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// SUB.S: `fd = fs - ft`.
    #[inline]
    #[must_use]
    pub fn fpu_sub_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::sub_s(self.f_bits(fs), self.f_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// MUL.S: `fd = fs * ft`.
    #[inline]
    #[must_use]
    pub fn fpu_mul_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::mul_s(self.f_bits(fs), self.f_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// DIV.S: `fd = fs / ft`.
    #[inline]
    #[must_use]
    pub fn fpu_div_s(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::div_s(self.f_bits(fs), self.f_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// SQRT.S: `fd = sqrt(fs)`, correctly rounded under FCSR.RM.
    #[inline]
    #[must_use]
    pub fn fpu_sqrt_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::sqrt_s(self.f_bits(fs), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// ABS.S: `fd = |fs|` (sign-bit op; Invalid only on an SNaN operand).
    #[inline]
    #[must_use]
    pub fn fpu_abs_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::abs_s(self.f_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// NEG.S: `fd = -fs` (sign-bit op; Invalid only on an SNaN operand).
    #[inline]
    #[must_use]
    pub fn fpu_neg_s(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::neg_s(self.f_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_f_bits(fd, bits);
        false
    }

    /// ADD.D: `fd = fs + ft`.
    #[inline]
    #[must_use]
    pub fn fpu_add_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::add_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// SUB.D: `fd = fs - ft`.
    #[inline]
    #[must_use]
    pub fn fpu_sub_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::sub_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// MUL.D: `fd = fs * ft`.
    #[inline]
    #[must_use]
    pub fn fpu_mul_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::mul_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// DIV.D: `fd = fs / ft`.
    #[inline]
    #[must_use]
    pub fn fpu_div_d(&mut self, fd: u8, fs: u8, ft: u8) -> bool {
        let (bits, flags) = crate::fpu::div_d(self.d_bits(fs), self.d_bits(ft), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// SQRT.D: `fd = sqrt(fs)`, correctly rounded under FCSR.RM.
    #[inline]
    #[must_use]
    pub fn fpu_sqrt_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::sqrt_d(self.d_bits(fs), self.fcsr_rm());
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// ABS.D: `fd = |fs|`.
    #[inline]
    #[must_use]
    pub fn fpu_abs_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::abs_d(self.d_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    /// NEG.D: `fd = -fs`.
    #[inline]
    #[must_use]
    pub fn fpu_neg_d(&mut self, fd: u8, fs: u8) -> bool {
        let (bits, flags) = crate::fpu::neg_d(self.d_bits(fs));
        if self.apply_fpu_flags(flags) {
            return true;
        }
        self.set_d_bits(fd, bits);
        false
    }

    // --- FP conditional moves (MOVF/MOVT/MOVZ/MOVN.fmt). ---
    //
    // These copy the source FPR to the destination FPR only when a predicate
    // holds; when it does not, the destination is left UNCHANGED. They are pure
    // bit copies — no rounding, no IEEE exception, no FCSR effect (VR4300
    // User's Manual, MOVF/MOVT/MOVZ/MOVN.fmt). The move width follows the format
    // (single copies 32 bits through the FR-aware single accessor; double copies
    // 64 bits through the double accessor), so the FR even/odd model applies
    // uniformly.

    /// `MOVF.S`/`MOVT.S`: `fd = fs` (single) iff `fpu_cond == tf`.
    #[inline]
    pub fn fpu_movcf_s(&mut self, fd: u8, fs: u8, tf: bool) {
        if self.fpu_cond == tf {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVF.D`/`MOVT.D`: `fd = fs` (double) iff `fpu_cond == tf`.
    #[inline]
    pub fn fpu_movcf_d(&mut self, fd: u8, fs: u8, tf: bool) {
        if self.fpu_cond == tf {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// `MOVZ.S`: `fd = fs` (single) iff GPR `rt` reads zero (full 64 bits).
    #[inline]
    pub fn fpu_movz_s(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) == 0 {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVN.S`: `fd = fs` (single) iff GPR `rt` reads nonzero.
    #[inline]
    pub fn fpu_movn_s(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) != 0 {
            self.set_f_bits(fd, self.f_bits(fs));
        }
    }

    /// `MOVZ.D`: `fd = fs` (double) iff GPR `rt` reads zero.
    #[inline]
    pub fn fpu_movz_d(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) == 0 {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// `MOVN.D`: `fd = fs` (double) iff GPR `rt` reads nonzero.
    #[inline]
    pub fn fpu_movn_d(&mut self, fd: u8, fs: u8, rt: u8) {
        if self.r(rt) != 0 {
            self.set_d_bits(fd, self.d_bits(fs));
        }
    }

    /// Evaluate any of the sixteen C.cond.fmt predicates. The low three funct
    /// bits select unordered/equal/less participation; bit 3 selects signaling
    /// behavior. Quiet compares still signal on an SNaN.
    pub fn try_fpu_compare(
        &mut self,
        lhs: f64,
        rhs: f64,
        lhs_snan: bool,
        rhs_snan: bool,
        cond: u8,
    ) -> Result<(), FpuException> {
        assert!(cond < 16, "COP1 compare predicate exceeds four bits");
        self.fcsr &= !(0x3F << 12);
        let unordered = lhs.is_nan() || rhs.is_nan();
        let condition = (unordered && cond & 1 != 0)
            || (!unordered && lhs == rhs && cond & 2 != 0)
            || (!unordered && lhs < rhs && cond & 4 != 0);
        if (unordered && cond & 0x8 != 0) || lhs_snan || rhs_snan {
            self.record_fpu_exception(SingleFpuCause::Invalid)?;
        }
        self.fpu_cond = condition;
        Ok(())
    }

    #[inline]
    pub fn try_fpu_compare_s(&mut self, fs: u8, ft: u8, cond: u8) -> Result<(), FpuException> {
        let a = self.f_bits(fs);
        let b = self.f_bits(ft);
        self.try_fpu_compare(
            f32::from_bits(a) as f64,
            f32::from_bits(b) as f64,
            is_snan32(a),
            is_snan32(b),
            cond,
        )
    }

    #[inline]
    pub fn try_fpu_compare_d(&mut self, fs: u8, ft: u8, cond: u8) -> Result<(), FpuException> {
        let a = self.d_bits(fs);
        let b = self.d_bits(ft);
        self.try_fpu_compare(
            f64::from_bits(a),
            f64::from_bits(b),
            is_snan64(a),
            is_snan64(b),
            cond,
        )
    }

    /// Whole-function compatibility boundary. Arbitrary-PC lanes use the
    /// typed `try_` form so they can enter the guest exception vector.
    #[inline]
    pub fn fpu_compare_s(&mut self, fs: u8, ft: u8, cond: u8) {
        if self.try_fpu_compare_s(fs, ft, cond).is_err() {
            trap_unsupported("enabled COP1 compare exception in whole-function lane");
        }
    }

    /// Double-precision counterpart of [`RecompContext::fpu_compare_s`].
    #[inline]
    pub fn fpu_compare_d(&mut self, fs: u8, ft: u8, cond: u8) {
        if self.try_fpu_compare_d(fs, ft, cond).is_err() {
            trap_unsupported("enabled COP1 compare exception in whole-function lane");
        }
    }

    // ================================================================
    // COP1 / FPU register file.
    //
    // The VR4300 manual sections 5.2 and 5.3 define 32 physical FGRs. In FR=0
    // each contributes one 32-bit word and an even doubleword FPR joins the
    // adjacent even/odd words. In FR=1 each FPR is one independent 64-bit FGR.
    // Keeping that physical shape means toggling Status.FR never rearranges or
    // discards state. All typed operations route through these raw accessors.
    // ================================================================

    /// Snapshot every physical FGR without applying the active FR view.
    ///
    /// This compatibility accessor retains the legacy differential-test name,
    /// but its entries are physical registers rather than FR-shaped slots.
    pub fn fpr_slots(&self) -> [u64; 32] {
        self.fpr.physical_state().into_words()
    }

    /// Whether Status.FR selects 32 independent 64-bit FPRs.
    #[inline]
    pub fn fpu_fr(&self) -> bool {
        self.cop0_status & COP0_STATUS_FR != 0
    }

    /// Read the low word of physical FGR `idx`. Under FR=0 these 32 words are
    /// the complete FGR file; under FR=1 this is the single/W view of the same
    /// independent 64-bit register.
    #[inline]
    pub fn f_bits(&self, idx: u8) -> u32 {
        self.fpr.word(idx)
    }

    /// Write the low word of physical FGR `idx`, preserving the upper word
    /// that is latent in FR=0 and independently visible in FR=1.
    #[inline]
    pub fn set_f_bits(&mut self, idx: u8, bits: u32) {
        self.fpr.set_word(idx, bits);
    }

    /// Read single-precision FPR `idx` as an `f32`.
    #[inline]
    pub fn f_s(&self, idx: u8) -> f32 {
        f32::from_bits(self.f_bits(idx))
    }

    /// Write an `f32` into single-precision FPR `idx`.
    #[inline]
    pub fn set_f_s(&mut self, idx: u8, val: f32) {
        self.set_f_bits(idx, val.to_bits());
    }

    /// Read a doubleword FPR. FR=0 joins the low words of adjacent even/odd
    /// FGRs; FR=1 reads one complete FGR and permits odd indices.
    #[inline]
    pub fn d_bits(&self, idx: u8) -> u64 {
        self.fpr.doubleword(idx, self.fpu_fr())
    }

    /// Write a doubleword through the active FR view. An FR=0 paired write
    /// preserves both physical FGR upper words so a later FR=1 view recovers
    /// them unchanged.
    #[inline]
    pub fn set_d_bits(&mut self, idx: u8, bits: u64) {
        let fr = self.fpu_fr();
        self.fpr.set_doubleword(idx, bits, fr);
    }

    /// Complete physical FGR state for deterministic state/evidence snapshots.
    /// This is view-independent: unlike 32 single reads it retains every upper
    /// word that FR=0 makes temporarily inaccessible.
    pub fn physical_fgr_state(&self) -> PhysicalFgrState {
        self.fpr.physical_state()
    }

    /// Replace the complete physical FGR file without interpreting the active
    /// FR view. ABI adapters use this only after validating that their packed
    /// C context mode agrees with CP0.Status.FR.
    pub fn replace_physical_fgr_state(&mut self, state: PhysicalFgrState) {
        self.fpr.replace_physical_state(state);
    }

    /// Read double-precision FPR `idx` as an `f64`.
    #[inline]
    pub fn f_d(&self, idx: u8) -> f64 {
        f64::from_bits(self.d_bits(idx))
    }

    /// Write an `f64` into double-precision FPR `idx`.
    #[inline]
    pub fn set_f_d(&mut self, idx: u8, val: f64) {
        self.set_d_bits(idx, val.to_bits());
    }
}

#[inline]
fn is_snan32(bits: u32) -> bool {
    // VR4300 User's Manual p.151 uses the legacy convention: fraction MSB 1
    // denotes signaling NaN, opposite the modern IEEE host convention.
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x007F_FFFF != 0 && bits & 0x0040_0000 != 0
}

#[inline]
fn is_qnan32(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x003F_FFFF != 0
}

#[inline]
fn is_subnormal32(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0 && bits & 0x007F_FFFF != 0
}

#[inline]
fn is_snan64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000
        && bits & 0x000F_FFFF_FFFF_FFFF != 0
        && bits & 0x0008_0000_0000_0000 != 0
}

#[inline]
fn is_qnan64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000 && bits & 0x0007_FFFF_FFFF_FFFF != 0
}

#[inline]
fn is_subnormal64(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0 && bits & 0x000F_FFFF_FFFF_FFFF != 0
}

/// Round an `f32` to the nearest integer, ties to even — the FPU's default
/// (FCSR round-to-nearest) rounding mode, which every OoT thread boots into.
/// This is the `CVT.W.S`/`CVT.L.S` rounding: N64Recomp routes it through
/// `lrintf` under the C default rounding mode (round-to-nearest-even). Rust's
/// [`f32::round_ties_even`] is exactly that, with no global FP-environment
/// dependency. Returned as `f64` so the caller's `as i32`/`as i64` truncation
/// of an already-integral value is exact.
#[inline]
pub fn round_ties_even_f32(v: f32) -> f64 {
    v.round_ties_even() as f64
}

/// Round an `f64` to the nearest integer, ties to even (the `CVT.W.D`/
/// `CVT.L.D` rounding; see [`round_ties_even_f32`]).
#[inline]
pub fn round_ties_even_f64(v: f64) -> f64 {
    v.round_ties_even()
}

/// Number of bytes of rdram the N64 exposes (8 MiB with the Expansion Pak,
/// which is what recompiled titles assume). The checked accessors bound every
/// access against this.
pub const RDRAM_LEN: usize = 8 * 1024 * 1024;

/// The base virtual address that maps to rdram offset 0 (KSEG0). Sign-extended
/// to 64 bits, this is the `0xFFFF_FFFF_8000_0000` the C macros subtract.
pub const RDRAM_VBASE: u64 = 0xFFFF_FFFF_8000_0000;

/// A checked view over rdram. All emitted memory accesses go through these
/// typed methods; the address translation and the big-endian sub-word swizzle
/// live here and nowhere else.
pub struct Rdram<'a> {
    mem: &'a mut [u8],
}

/// The common signature of every typed-Rust recompiled function.
///
/// This is the safe-Rust equivalent of N64Recomp's MIT-licensed
/// `recomp_func_t = void(uint8_t *rdram, recomp_context *ctx)`
/// (`refs/N64RecompSource/include/recomp.h:443-451`). The three explicit
/// higher-ranked lifetimes keep the context borrow, the `Rdram` view borrow,
/// and the underlying byte-slice borrow independent; no pointer conversion or
/// lifetime erasure is involved.
pub type RecompFunc =
    for<'ctx, 'view, 'rdram> fn(&'ctx mut RecompContext, &'view mut Rdram<'rdram>);

/// Invalid input to [`HostFunctionCatalogV1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostFunctionCatalogErrorV1 {
    MisalignedTarget { target: u32 },
    DuplicateTarget { target: u32 },
}

/// Exact, enumerable host-function targets installed beside generated code.
///
/// This catalog deliberately carries no resolver-policy authority: it proves
/// only its own sorted target/function association. In particular, creating a
/// catalog neither installs nor supersedes the legacy [`HostLookup`] hook.
/// An empty catalog is valid and represents a program with no host targets.
#[derive(Clone, Debug)]
pub struct HostFunctionCatalogV1 {
    target_pcs: Vec<u32>,
    functions: Vec<RecompFunc>,
}

impl HostFunctionCatalogV1 {
    pub fn new(mut entries: Vec<(u32, RecompFunc)>) -> Result<Self, HostFunctionCatalogErrorV1> {
        if let Some(&(target, _)) = entries.iter().find(|(target, _)| !target.is_multiple_of(4)) {
            return Err(HostFunctionCatalogErrorV1::MisalignedTarget { target });
        }
        entries.sort_unstable_by_key(|(target, _)| *target);
        if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(HostFunctionCatalogErrorV1::DuplicateTarget { target: pair[0].0 });
        }
        let (target_pcs, functions) = entries.into_iter().unzip();
        Ok(Self {
            target_pcs,
            functions,
        })
    }

    /// Canonical ascending target inventory, independent of input order.
    pub fn target_pcs(&self) -> &[u32] {
        &self.target_pcs
    }

    pub fn is_empty(&self) -> bool {
        self.target_pcs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.target_pcs.len()
    }

    pub fn resolve(&self, target: u32) -> Option<RecompFunc> {
        self.target_pcs
            .binary_search(&target)
            .ok()
            .map(|index| self.functions[index])
    }
}

/// Host lookup hook used for functions that must be supplied by the runtime
/// instead of executing a recompiled body (libultra shims, exception/TLB
/// handling, and other host-owned boundaries).
pub type HostLookup = fn(u32) -> Option<RecompFunc>;
/// Cooperative-yield hook for the N64Recomp `pause_self` self-loop rule.
pub type HostPause = fn();
/// Optional raw word-MMIO read. `None` means the address is ordinary memory.
pub type MmioRead = fn(u64) -> Option<u32>;
/// Optional raw word-MMIO write. `true` means the device consumed the write.
pub type MmioWrite = fn(u64, u32) -> bool;
/// The exact byte-producing mechanism responsible for one committed guest
/// RDRAM mutation.
///
/// This is a fixed architectural denominator, not an open-ended diagnostic
/// label. Every public external-write gateway below selects exactly one
/// variant; callers cannot submit an unattributed write event.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WriterChannel {
    CpuInstructionStore,
    PiDma,
    SiDma,
    SpDma,
    RspExecutionOrHleWriteback,
    RdpRenderer,
    HostAbi,
    BootstrapOrImport,
}

/// One attributed post-commit guest write. Only aligned CPU halfword stores
/// carry a value because public RDRAM hidden-bit behavior assigns semantics
/// to that exact operation; other effects remain exact attributed ranges.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GuestWriteEvent {
    Range {
        channel: WriterChannel,
        physical_offset: u32,
        len: u32,
    },
    NonRdpWrite16 {
        channel: WriterChannel,
        logical_offset: u32,
        value: u16,
    },
}

impl GuestWriteEvent {
    pub const fn channel(self) -> WriterChannel {
        match self {
            Self::Range { channel, .. } | Self::NonRdpWrite16 { channel, .. } => channel,
        }
    }

    pub const fn range(self) -> (u32, u32) {
        match self {
            Self::Range {
                physical_offset,
                len,
                ..
            } => (physical_offset, len),
            Self::NonRdpWrite16 { logical_offset, .. } => (logical_offset, 2),
        }
    }
}

/// Post-commit physical RDRAM write observer. Executable invalidation and
/// renderer notification are multiplexed by the host callback.
pub type WriteObserver = fn(GuestWriteEvent);

/// One successful checked arbitrary-PC guest data load from physical RDRAM.
/// The length conservatively covers the bytes touched by the backing read.
/// Whole-function generated runners and host-side snapshots do not publish
/// these events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GuestReadEvent {
    pub physical_offset: u32,
    pub len: u32,
}

pub type ReadObserver = fn(GuestReadEvent);

/// Whether one committed guest write changed bytes owned by the active
/// executable image.
///
/// The ordinary observation callback above remains notification-only. A live
/// block-program owner installs this second callback only when it can prove
/// that a write intersects one of its registered executable regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestWriteBoundary {
    Continue,
    ExecutableChanged,
}

pub type GuestWriteBoundaryObserver = fn(GuestWriteEvent) -> GuestWriteBoundary;

/// Host callback reached immediately before translated code preserves a loud
/// panic for an instruction shape this runtime does not model.
pub type UnsupportedObserver = fn(&str);

/// Stable identity emitted at the first statement of every translated
/// whole-function body. The enclosing artifact identity remains host-owned;
/// `(vram, symbol)` distinguishes functions within that artifact without
/// depending on native addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TranslatedFunctionIdentity {
    pub vram: u32,
    pub symbol: &'static str,
}

impl TranslatedFunctionIdentity {
    pub const fn new(vram: u32, symbol: &'static str) -> Self {
        Self { vram, symbol }
    }
}

/// Opaque version marker exported by newly generated whole-function modules.
/// Passing a generated module's marker to the ABI is the explicit assertion
/// that every callable in that artifact contains the entry hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionEntryObservationSchema(u32);

/// Entry-observation schema implemented by this emitter/runtime pair.
pub const FUNCTION_ENTRY_OBSERVATION_SCHEMA: FunctionEntryObservationSchema =
    FunctionEntryObservationSchema(1);

/// Host callback reached by an emitted body before its first translated
/// instruction executes.
pub type FunctionEntryObserver = fn(TranslatedFunctionIdentity);

thread_local! {
    /// Recompiled execution is single-threaded by design (`docs/DESIGN.md` section
    /// 2), so the override belongs to the executing host thread. A
    /// thread-local `Cell` also lets tests install an isolated resolver
    /// without unsafe global mutation or cross-test serialization.
    static HOST_LOOKUP: std::cell::Cell<Option<HostLookup>> = const {
        std::cell::Cell::new(None)
    };
    static HOST_PAUSE: std::cell::Cell<Option<HostPause>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_READ: std::cell::Cell<Option<MmioRead>> = const {
        std::cell::Cell::new(None)
    };
    static MMIO_WRITE: std::cell::Cell<Option<MmioWrite>> = const {
        std::cell::Cell::new(None)
    };
    static WRITE_OBSERVER: std::cell::Cell<Option<WriteObserver>> = const {
        std::cell::Cell::new(None)
    };
    static READ_OBSERVER: std::cell::Cell<Option<ReadObserver>> = const {
        std::cell::Cell::new(None)
    };
    static GUEST_WRITE_BOUNDARY_OBSERVER:
        std::cell::Cell<Option<GuestWriteBoundaryObserver>> = const {
            std::cell::Cell::new(None)
        };
    static EXECUTABLE_WRITE_BOUNDARY: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
    static UNSUPPORTED_OBSERVER: std::cell::Cell<Option<UnsupportedObserver>> = const {
        std::cell::Cell::new(None)
    };
    static FUNCTION_ENTRY_OBSERVER: std::cell::Cell<Option<FunctionEntryObserver>> = const {
        std::cell::Cell::new(None)
    };
    static GUEST_WRITE_SESSION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static GUEST_WRITE_EPOCH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static GUEST_WRITE_PAGE_EPOCHS: std::cell::RefCell<Vec<u32>> =
        std::cell::RefCell::new(vec![0; RDRAM_LEN / 4096]);
}

/// Install (or clear) the current thread's host-function resolver, returning
/// the previous resolver.
///
/// Generated dispatchers consult this hook before their sorted recompiled table.
/// A host can therefore bind a vram to a safe typed adapter over an fn64 shim;
/// vrams deliberately omitted from the recompiled table fail loudly if the host
/// has not installed their adapter. The function-pointer seam itself is
/// entirely safe Rust: no `transmute`, raw pointer, or ABI cast is involved.
pub fn set_host_lookup(resolver: Option<HostLookup>) -> Option<HostLookup> {
    HOST_LOOKUP.with(|slot| slot.replace(resolver))
}

/// Install the host's cooperative-yield adapter for translated self-loops.
pub fn set_host_pause(pause: Option<HostPause>) -> Option<HostPause> {
    HOST_PAUSE.with(|slot| slot.replace(pause))
}

/// Install the raw word-MMIO boundary used by emitted `lw`/`sw` operations.
/// The hooks are thread-local like host lookup because guest execution is
/// single-threaded; ordinary RDRAM accesses remain direct checked slice I/O.
pub fn set_mmio_hooks(
    read: Option<MmioRead>,
    write: Option<MmioWrite>,
) -> (Option<MmioRead>, Option<MmioWrite>) {
    let previous_read = MMIO_READ.with(|slot| slot.replace(read));
    let previous_write = MMIO_WRITE.with(|slot| slot.replace(write));
    (previous_read, previous_write)
}

pub fn set_write_observer(observer: Option<WriteObserver>) -> Option<WriteObserver> {
    WRITE_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install (or clear) the current thread's checked arbitrary-PC data-load
/// observer, returning the previous observer. The copied callback may replace
/// or clear itself; recursive checked loads recursively invoke it. A callback
/// panic propagates after the backing read and leaves the callback installed.
pub fn set_read_observer(observer: Option<ReadObserver>) -> Option<ReadObserver> {
    READ_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the live executable-owner callback and clear any request belonging
/// to the previous owner.
pub fn set_guest_write_boundary_observer(
    observer: Option<GuestWriteBoundaryObserver>,
) -> Option<GuestWriteBoundaryObserver> {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(false));
    GUEST_WRITE_SESSION.with(|session| {
        let next = session
            .get()
            .checked_add(1)
            .expect("guest-write session overflow");
        session.set(next);
    });
    GUEST_WRITE_EPOCH.with(|epoch| epoch.set(0));
    GUEST_WRITE_PAGE_EPOCHS.with(|epochs| epochs.borrow_mut().fill(0));
    GUEST_WRITE_BOUNDARY_OBSERVER.with(|slot| slot.replace(observer))
}

fn mark_guest_write_pages(offset: u32, len: u32) {
    if len == 0 {
        return;
    }
    let start = offset as usize / 4096;
    let end = (offset as usize)
        .saturating_add(len as usize)
        .saturating_sub(1)
        / 4096;
    let epoch = GUEST_WRITE_EPOCH.with(|epoch| {
        let next = epoch
            .get()
            .checked_add(1)
            .expect("guest-write epoch overflow");
        epoch.set(next);
        next
    });
    GUEST_WRITE_PAGE_EPOCHS.with(|epochs| {
        let mut epochs = epochs.borrow_mut();
        for page in start..=end.min(epochs.len().saturating_sub(1)) {
            epochs[page] = epoch;
        }
    });
}

/// Session-qualified last-write token for a physical RDRAM range. A caller
/// may cache successful byte-image verification until this token changes.
pub fn guest_write_token(offset: u32, len: u32) -> u64 {
    assert!(len > 0, "guest-write token range must be nonempty");
    let start = offset as usize / 4096;
    let end = (offset as usize)
        .checked_add(len as usize)
        .and_then(|end| end.checked_sub(1))
        .expect("guest-write token range overflow")
        / 4096;
    let page_epoch = GUEST_WRITE_PAGE_EPOCHS.with(|epochs| {
        let epochs = epochs.borrow();
        assert!(end < epochs.len(), "guest-write token range exceeds RDRAM");
        epochs[start..=end].iter().copied().max().unwrap_or(0)
    });
    let session = GUEST_WRITE_SESSION.with(std::cell::Cell::get);
    (u64::from(session) << 32) | u64::from(page_epoch)
}

/// Consume one post-store executable invalidation request.
///
/// Generated and interpreted runners call this only at architectural
/// instruction boundaries. A control-transfer owner deliberately waits until
/// its delay slot has completed, so invalidation cannot split the pair.
#[inline]
pub fn take_executable_write_boundary() -> bool {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.replace(false))
}

/// Discard a boundary request after an external writer's owner has already
/// published the replacement executable generation.
///
/// Device DMA and RSP writeback use the same post-commit notification seam as
/// CPU stores, but execute while no translated runner is active. Their host
/// boundary processes the replacement directly; retaining the request would
/// make the next generation stop after its first instruction for a write that
/// has already been serviced.
pub fn discard_executable_write_boundary() {
    EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(false));
}

/// Install the host's unsupported-instruction evidence sink. The translated
/// lane remains independently usable: without a sink, the same named panic
/// still fires.
pub fn set_unsupported_observer(
    observer: Option<UnsupportedObserver>,
) -> Option<UnsupportedObserver> {
    UNSUPPORTED_OBSERVER.with(|slot| slot.replace(observer))
}

/// Install the current thread's translated-function entry observer.
pub fn set_function_entry_observer(
    observer: Option<FunctionEntryObserver>,
) -> Option<FunctionEntryObserver> {
    FUNCTION_ENTRY_OBSERVER.with(|slot| slot.replace(observer))
}

/// Record entry into one emitted whole-function body. Generated code places
/// this call before initializing its local dispatch PC, so direct calls,
/// lookup-resolved calls, tail calls, and root entry share one boundary.
#[inline]
pub fn notify_function_entry(identity: TranslatedFunctionIdentity) {
    FUNCTION_ENTRY_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(identity);
        }
    });
}

/// Record and preserve the loud endpoint for unsupported translated CPU
/// behavior. Generated bodies use this instead of open-coded panics so the
/// fixed-cycle journal cannot miss an early abort.
#[cold]
#[inline(never)]
pub fn trap_unsupported(context: impl Into<String>) -> ! {
    let context = context.into();
    UNSUPPORTED_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(&context);
        }
    });
    panic!("{context}")
}

/// Common post-commit implementation. It stays private so external producers
/// must choose one of the exact channel-specific gateways below.
fn notify_attributed_guest_write(channel: WriterChannel, offset: u32, len: u32) {
    if len != 0 {
        mark_guest_write_pages(offset, len);
        let event = GuestWriteEvent::Range {
            channel,
            physical_offset: offset,
            len,
        };
        WRITE_OBSERVER.with(|slot| {
            if let Some(observer) = slot.get() {
                observer(event);
            }
        });
        request_guest_write_boundary(event);
    }
}

/// Report a generated-C or other externally adapted CPU instruction store.
pub fn notify_cpu_instruction_store(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::CpuInstructionStore, offset, len);
}

pub fn notify_pi_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::PiDma, offset, len);
}

pub fn notify_si_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::SiDma, offset, len);
}

pub fn notify_sp_dma_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::SpDma, offset, len);
}

pub fn notify_rsp_execution_or_hle_writeback(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::RspExecutionOrHleWriteback, offset, len);
}

pub fn notify_rdp_renderer_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::RdpRenderer, offset, len);
}

pub fn notify_host_abi_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::HostAbi, offset, len);
}

pub fn notify_bootstrap_or_import_write(offset: u32, len: u32) {
    notify_attributed_guest_write(WriterChannel::BootstrapOrImport, offset, len);
}

/// Notify one aligned CPU halfword store after the visible bytes commit.
fn notify_cpu_instruction_store16(logical_offset: u32, value: u16) {
    mark_guest_write_pages(logical_offset, 2);
    let event = GuestWriteEvent::NonRdpWrite16 {
        channel: WriterChannel::CpuInstructionStore,
        logical_offset,
        value,
    };
    WRITE_OBSERVER.with(|slot| {
        if let Some(observer) = slot.get() {
            observer(event);
        }
    });
    request_guest_write_boundary(event);
}

#[inline]
fn request_guest_write_boundary(event: GuestWriteEvent) {
    GUEST_WRITE_BOUNDARY_OBSERVER.with(|slot| {
        if slot
            .get()
            .is_some_and(|observer| observer(event) == GuestWriteBoundary::ExecutableChanged)
        {
            // Closes the exact interleaving `generation-A store commits -> A
            // executes a later translated instruction -> host retires A`.
            // The runner consumes this only after the current instruction, or
            // after the complete branch/delay pair when the store is its slot.
            EXECUTABLE_WRITE_BOUNDARY.with(|pending| pending.set(true));
        }
    });
}

/// Yield the active emulated thread at an unconditional branch-to-self.
pub fn pause_self() {
    HOST_PAUSE.with(|slot| {
        slot.get()
            .unwrap_or_else(|| panic!("pause_self: rs host installed no coroutine-yield adapter"))(
        )
    });
}

/// Resolve `vram` through the current thread's host-function resolver.
#[inline]
pub fn resolve_host_function(vram: u32) -> Option<RecompFunc> {
    HOST_LOOKUP.with(|slot| slot.get().and_then(|resolver| resolver(vram)))
}

/// Invoke a statically-known recompiled target unless the host resolver overrides
/// its vram. This is the direct-JAL counterpart of generated `lookup(vram)`:
/// libultra functions whose bodies contain no privileged instruction still
/// must enter the executor-backed host shim rather than bypassing it merely
/// because the Rust recompiler could translate their machine code.
#[inline]
pub fn call_host_or_recompiled(
    vram: u32,
    recompiled: RecompFunc,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) {
    resolve_host_function(vram).unwrap_or(recompiled)(ctx, mem);
}

impl<'a> Rdram<'a> {
    /// Wrap a byte buffer as rdram. The buffer should be [`RDRAM_LEN`] bytes;
    /// shorter buffers simply make more addresses fall out of bounds (a loud
    /// panic on access) rather than corrupting host memory.
    pub fn new(mem: &'a mut [u8]) -> Self {
        Rdram { mem }
    }

    /// Borrow the shared backing allocation at the runtime ABI seam. Normal
    /// emitted code has no reason to use this; fn64's rs-lane host adapters use
    /// it to call the existing, audited `*_recomp` marshalling layer without
    /// allocating or copying a second RDRAM image.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.mem
    }

    /// Snapshot one physical RDRAM interval in guest byte order.
    ///
    /// Canonical executable-image reconciliation uses this read-only API so
    /// the ABI owner never needs another mutable slice or raw pointer merely
    /// to prove that no unjournaled writer changed a precompiled backing.
    pub fn copy_physical_bytes(&self, physical_start: u32, byte_len: u32) -> Vec<u8> {
        let physical_end = physical_start
            .checked_add(byte_len)
            .unwrap_or_else(|| panic!("physical RDRAM snapshot range overflow"));
        assert!(
            physical_end <= RDRAM_LEN as u32,
            "physical RDRAM snapshot [{physical_start:#010x}, {physical_end:#010x}) exceeds {RDRAM_LEN:#x} bytes"
        );
        (physical_start..physical_end)
            .map(|physical| self.load_physical_bu(physical))
            .collect()
    }

    /// Translate a canonical KSEG0/KSEG1 address to its generated-code backing
    /// offset. Physical RDRAM aliases share the low 29-bit device prefix;
    /// non-RDRAM direct windows retain N64Recomp's sparse `address - KSEG0`
    /// layout. Modeled RCP/PIF words are excluded because their installed hook
    /// is the sole device authority.
    #[inline]
    fn direct_storage_offset(vaddr: u64) -> Option<usize> {
        if Self::is_word_only_mmio(vaddr) {
            return None;
        }

        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        if !canonical_32 || !direct_segment {
            return None;
        }

        let physical = low & 0x1fff_ffff;
        Some(if physical < RDRAM_LEN as u32 {
            physical as usize
        } else {
            low.wrapping_sub(0x8000_0000) as usize
        })
    }

    #[inline]
    fn backing_offset(vaddr: u64) -> usize {
        if let Some(offset) = Self::direct_storage_offset(vaddr) {
            return offset;
        }
        let physical = (vaddr as u32) & 0x1fff_ffff;
        let reason = if Self::is_word_only_mmio(vaddr) {
            "modeled word-only device access was not consumed by the installed hook"
        } else {
            "only zero- or sign-extended KSEG0/KSEG1 are modeled"
        };
        trap_unsupported(format!(
            "Rdram: unsupported mapped address {vaddr:#018x} resolves to physical {physical:#x}; {reason}"
        ))
    }

    #[inline]
    fn read_mmio_word(vaddr: u64) -> Option<u32> {
        MMIO_READ.with(|slot| slot.get().and_then(|read| read(vaddr)))
    }

    #[inline]
    fn write_mmio_word(vaddr: u64, value: u32) -> bool {
        MMIO_WRITE.with(|slot| slot.get().is_some_and(|write| write(vaddr, value)))
    }

    #[inline]
    fn load_backed_word(&self, vaddr: u64) -> i32 {
        let p = Self::backing_offset(vaddr);
        i32::from_ne_bytes(self.mem[p..p + 4].try_into().unwrap())
    }

    #[inline]
    fn store_backed_word(&mut self, vaddr: u64, value: u32) {
        let p = Self::backing_offset(vaddr);
        self.mem[p..p + 4].copy_from_slice(&value.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store(offset, 4);
        }
    }

    /// Canonical physical RDRAM offset for cached or uncached CPU aliases.
    /// Only the direct segments are accepted: masking KUSEG would silently
    /// add unsupported TLB behavior. Device/renderer observations are named
    /// in the same physical RDRAM space as the visible backing bytes.
    #[inline]
    fn physical_rdram_offset(vaddr: u64) -> Option<u32> {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        let direct_segment = (0x8000_0000..0xc000_0000).contains(&low);
        let physical = low & 0x1fff_ffff;
        (canonical_32 && direct_segment && physical < RDRAM_LEN as u32).then_some(physical)
    }

    #[inline]
    fn notify_translated_rdram_read(vaddr: u64, len: u32) {
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            READ_OBSERVER.with(|slot| {
                if let Some(observer) = slot.get() {
                    observer(GuestReadEvent {
                        physical_offset: offset,
                        len,
                    });
                }
            });
        }
    }

    /// Generated-C's proxy exposes RCP registers and PIF RAM only as modeled
    /// word accesses. Keep the typed lane on that identical boundary instead
    /// of letting a subword operation fall through to sparse host storage.
    #[inline]
    fn is_word_only_mmio(vaddr: u64) -> bool {
        let upper = vaddr >> 32;
        let low = vaddr as u32;
        let canonical_32 = upper == 0 || upper == u32::MAX as u64;
        if canonical_32 && (0xa400_0000..0xa490_0000).contains(&low) {
            return true;
        }
        let physical = low & 0x1fff_ffff;
        canonical_32
            && (0x8000_0000..0xc000_0000).contains(&low)
            && (0x1fc0_07c0..0x1fc0_0800).contains(&physical)
    }

    #[inline]
    fn reject_nonword_mmio(vaddr: u64, width: u32, is_write: bool) {
        if Self::is_word_only_mmio(vaddr) {
            let operation = if is_write { "write" } else { "read" };
            trap_unsupported(format!(
                "Rdram: raw MMIO {operation} at {vaddr:#018x} used unsupported {width}-byte access; RCP/PIF registers require modeled word semantics"
            ));
        }
    }

    /// Effective virtual address of a `off(base)` operand: full-width MIPS III
    /// addition of the 64-bit base and sign-extended 16-bit offset.
    #[inline]
    pub fn eff_addr(base_val: u64, off: i16) -> u64 {
        base_val.wrapping_add(off as i64 as u64)
    }

    // --- Aligned loads ---

    /// Load a sign-extended word. Returns the `i32` the caller sign-extends
    /// into a GPR.
    ///
    /// Perf: read the 4 bytes as ONE slice range (`self.mem[p..p+4]`) rather
    /// than four `self.mem[p+i]` indexes. The range form does a SINGLE bounds
    /// check and lets the compiler emit one aligned 32-bit load; the byte-at-
    /// a-time form did 4 bounds checks + a byte-assemble in the hot loop
    /// (millions of accesses in collision init). Same value, safe indexing,
    /// still `#![forbid(unsafe_code)]`.
    #[inline]
    pub fn load_w(&self, vaddr: u64) -> i32 {
        assert_eq!(vaddr & 3, 0, "unaligned LW at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_word(vaddr) {
            return value as i32;
        }
        self.load_backed_word(vaddr)
    }

    /// Load a sign-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_h(&self, vaddr: u64) -> i16 {
        Self::reject_nonword_mmio(vaddr, 2, false);
        assert_eq!(vaddr & 1, 0, "unaligned LH at {vaddr:#018x}");
        let p = Self::backing_offset(vaddr) ^ 2;
        i16::from_ne_bytes(self.mem[p..p + 2].try_into().unwrap())
    }

    /// Load a zero-extended halfword (byte offset XOR 2).
    #[inline]
    pub fn load_hu(&self, vaddr: u64) -> u16 {
        self.load_h(vaddr) as u16
    }

    /// Load a sign-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_b(&self, vaddr: u64) -> i8 {
        Self::reject_nonword_mmio(vaddr, 1, false);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] as i8
    }

    /// Load a zero-extended byte (byte offset XOR 3).
    #[inline]
    pub fn load_bu(&self, vaddr: u64) -> u8 {
        Self::reject_nonword_mmio(vaddr, 1, false);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p]
    }

    /// Read one explicitly admitted physical RDRAM byte without reconstructing
    /// a virtual alias. Generation-digest selection uses this after validating
    /// its complete VA-to-physical backing map.
    #[inline]
    pub(crate) fn load_physical_bu(&self, physical: u32) -> u8 {
        assert!(
            physical < RDRAM_LEN as u32,
            "physical RDRAM byte {physical:#010x} exceeds the 8 MiB device"
        );
        let p = usize::try_from(physical).expect("physical RDRAM offset exceeds usize") ^ 3;
        *self.mem.get(p).unwrap_or_else(|| {
            panic!(
                "physical RDRAM byte {physical:#010x} exceeds the installed {}-byte backing",
                self.mem.len()
            )
        })
    }

    /// Read one physical byte only when it is present in the installed RDRAM
    /// allocation. Live mapped instruction admission uses this checked form so
    /// a translated fetch beyond backing becomes a typed CPU fault rather than
    /// an indexing panic.
    #[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
    #[inline]
    pub(crate) fn try_load_physical_bu(&self, physical: u32) -> Option<u8> {
        if physical >= RDRAM_LEN as u32 {
            return None;
        }
        let p = usize::try_from(physical).ok()? ^ 3;
        self.mem.get(p).copied()
    }

    // --- Aligned stores ---

    /// Store the low word of `val`.
    #[inline]
    pub fn store_w(&mut self, vaddr: u64, val: u32) {
        assert_eq!(vaddr & 3, 0, "unaligned SW at {vaddr:#018x}");
        if Self::write_mmio_word(vaddr, val) {
            return;
        }
        self.store_backed_word(vaddr, val);
    }

    /// Store the low halfword of `val` (byte offset XOR 2).
    #[inline]
    pub fn store_h(&mut self, vaddr: u64, val: u16) {
        Self::reject_nonword_mmio(vaddr, 2, true);
        assert_eq!(vaddr & 1, 0, "unaligned SH at {vaddr:#018x}");
        let p = Self::backing_offset(vaddr) ^ 2;
        self.mem[p..p + 2].copy_from_slice(&val.to_ne_bytes());
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store16(offset, val);
        }
    }

    /// Store the low byte of `val` (byte offset XOR 3).
    #[inline]
    pub fn store_b(&mut self, vaddr: u64, val: u8) {
        Self::reject_nonword_mmio(vaddr, 1, true);
        let p = Self::backing_offset(vaddr) ^ 3;
        self.mem[p] = val;
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            notify_cpu_instruction_store(offset, 1);
        }
    }

    // --- Unaligned word loads/stores (LWL/LWR/SWL/SWR) ---
    //
    // Semantics clean-roomed from the MIPS III ISA: the pair of instructions
    // together load/store a full word straddling an alignment boundary. We
    // mirror N64Recomp's `do_lwl`/`do_lwr`/`do_swl`/`do_swr` helper math,
    // which is itself the ISA definition.

    /// Load-word-left: merge the high bytes of the addressed word into the
    /// high end of `initial` (the current register value).
    #[inline]
    pub fn load_wl(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 << (misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded << (misalign * 8))) as i32
    }

    /// Load-word-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_wr(&self, initial: u64, vaddr: u64) -> i32 {
        let word_addr = vaddr & !0x3;
        let loaded = self.load_w(word_addr) as u32;
        let misalign = (vaddr & 0x3) as u32;
        let mask = !(0xFFFF_FFFFu32 >> (24 - misalign * 8));
        let masked = (initial as u32) & mask;
        (masked | (loaded >> (24 - misalign * 8))) as i32
    }

    /// Store-word-left.
    #[inline]
    pub fn store_wl(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_word_only_mmio(word_addr) {
            if misalign != 0 {
                Self::reject_nonword_mmio(vaddr, 4 - misalign, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    /// Store-word-right.
    #[inline]
    pub fn store_wr(&mut self, vaddr: u64, val: u32) {
        let word_addr = vaddr & !0x3;
        let misalign = (vaddr & 0x3) as u32;
        if Self::is_word_only_mmio(word_addr) {
            if misalign != 3 {
                Self::reject_nonword_mmio(vaddr, misalign + 1, true);
            }
            self.store_w(word_addr, val);
            return;
        }
        let initial = self.load_w(word_addr) as u32;
        let masked = initial & !(0xFFFF_FFFFu32 << (24 - misalign * 8));
        let shifted = val << (24 - misalign * 8);
        self.store_w(word_addr, masked | shifted);
    }

    // --- 64-bit doubleword loads/stores (LD/SD/LLD/SCD) ---
    //
    // Clean-roomed from the MIPS III ISA and matching N64Recomp's
    // `load_doubleword`/`SD` macros exactly: a doubleword is the two 32-bit
    // words at `vaddr+0` (the high half) and `vaddr+4` (the low half). Each
    // half goes through the ordinary native-endian word path
    // (`load_w`/`store_w`) with no sub-word swizzle. Logically, the high guest
    // word remains at `vaddr+0` and the low guest word at `vaddr+4`.

    /// Load a 64-bit doubleword: `(hi_word << 32) | lo_word` where `hi_word` is
    /// at `vaddr+0` and `lo_word` at `vaddr+4`.
    #[inline]
    pub fn load_d(&self, vaddr: u64) -> u64 {
        Self::reject_nonword_mmio(vaddr, 8, false);
        assert_eq!(vaddr & 7, 0, "unaligned LD at {vaddr:#018x}");
        let hi = self.load_w(vaddr) as u32 as u64;
        let lo = self.load_w(vaddr.wrapping_add(4)) as u32 as u64;
        (hi << 32) | lo
    }

    /// Store a 64-bit doubleword: the high word to `vaddr+0`, the low word to
    /// `vaddr+4`, followed by one post-commit eight-byte write range.
    #[inline]
    pub fn store_d(&mut self, vaddr: u64, val: u64) {
        Self::reject_nonword_mmio(vaddr, 8, true);
        assert_eq!(vaddr & 7, 0, "unaligned SD at {vaddr:#018x}");
        if let Some(offset) = Self::physical_rdram_offset(vaddr) {
            let high = Self::backing_offset(vaddr);
            let low = Self::backing_offset(vaddr.wrapping_add(4));
            // Match N64Recomp's low-word then high-word commit order. The
            // observer runs only after both halves are coherent.
            self.mem[low..low + 4].copy_from_slice(&(val as u32).to_ne_bytes());
            self.mem[high..high + 4].copy_from_slice(&((val >> 32) as u32).to_ne_bytes());
            notify_cpu_instruction_store(offset, 8);
        } else {
            self.store_w(vaddr.wrapping_add(4), val as u32);
            self.store_w(vaddr, (val >> 32) as u32);
        }
    }

    // --- Unaligned doubleword loads/stores (LDL/LDR/SDL/SDR) ---
    //
    // The 64-bit analogue of LWL/LWR/SWL/SWR: the pair together moves a full
    // doubleword straddling an 8-byte boundary. Math mirrors N64Recomp's
    // `do_ldl`/`do_ldr`/`do_sdl`/`do_sdr`, which is the ISA definition. The
    // aligned dword the shift operates on is at `vaddr & !7`, and the shift
    // distances use the 3-bit misalignment (0..7).

    /// Load-doubleword-left: merge the high bytes of the addressed doubleword
    /// into the high end of `initial` (the current register value).
    #[inline]
    pub fn load_dl(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (misalign * 8));
        masked | (loaded << (misalign * 8))
    }

    /// Load-doubleword-right: merge the low bytes into the low end of `initial`.
    #[inline]
    pub fn load_dr(&self, initial: u64, vaddr: u64) -> u64 {
        let dword_addr = vaddr & !0x7;
        let loaded = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (56 - misalign * 8));
        masked | (loaded >> (56 - misalign * 8))
    }

    /// Store-doubleword-left.
    #[inline]
    pub fn store_dl(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 >> (misalign * 8));
        let shifted = val >> (misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }

    /// Store-doubleword-right.
    #[inline]
    pub fn store_dr(&mut self, vaddr: u64, val: u64) {
        let dword_addr = vaddr & !0x7;
        let initial = self.load_d(dword_addr);
        let misalign = (vaddr & 0x7) as u32;
        let masked = initial & !(0xFFFF_FFFF_FFFF_FFFFu64 << (56 - misalign * 8));
        let shifted = val << (56 - misalign * 8);
        self.store_d(dword_addr, masked | shifted);
    }

    // --- Checked accessors for the bank/sparse block-runner lane (U4) ---
    //
    // The historical whole-function lane calls the unchecked accessors above:
    // an access outside backed generated-code storage is a host panic there,
    // and that panicking semantics is deliberately preserved. The block-runner
    // lane instead needs
    // a typed VR4300 memory fault it can turn into `BlockExit::Fault`, so it
    // calls these `try_` variants. On success they perform the identical access
    // as their unchecked twin; on an out-of-bounds effective address they
    // return `Err(vaddr)` carrying the faulting guest virtual address, and
    // touch no memory. This models "access outside supplied backing storage";
    // it is not full VR4300 address-error/TLB semantics (see U4 in
    // `docs/UNIVERSAL-RUNTIME-PLAN.md`).

    /// True iff the `width`-byte range beginning at storage offset `p` lies
    /// wholly inside backed storage. Every checked accessor reduces to
    /// this after applying its own swizzle so the admitted set matches exactly
    /// which unchecked accesses would not panic.
    #[inline]
    fn storage_range_backed(&self, p: usize, width: usize) -> bool {
        p.checked_add(width)
            .is_some_and(|end| end <= self.mem.len())
    }

    /// Translate and bound a checked-lane access without entering the
    /// unchecked lane's loud unsupported-address trap. `try_*` callers must
    /// return the original virtual address as a typed fault for every
    /// non-direct segment, including opt-in MMIO windows with no installed port.
    #[inline]
    fn virtual_range_backed(&self, vaddr: u64, lane_xor: usize, width: usize) -> bool {
        Self::direct_storage_offset(vaddr)
            .is_some_and(|p| self.storage_range_backed(p ^ lane_xor, width))
    }

    /// Resolve the typed MMU result into the direct alias understood by
    /// the shared RDRAM/device backing layout. Physical addresses beyond the
    /// N64's 29-bit direct window remain a loud unbacked boundary after a
    /// successful TLB lookup; they are never truncated into a different page.
    fn translated_backing_address(
        ctx: &RecompContext,
        vaddr: u64,
        access: DataAccessKind,
    ) -> Result<u64, DataAccessError> {
        match ctx.translate_data_address(vaddr, access)? {
            TranslatedDataAddress::Direct(address) => Ok(address),
            TranslatedDataAddress::DirectPhysical(physical)
            | TranslatedDataAddress::Mapped(physical)
                if physical < 0x2000_0000 =>
            {
                Ok(0xffff_ffff_a000_0000 | u64::from(physical))
            }
            TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                Err(DataAccessError::Unbacked { vaddr })
            }
        }
    }

    fn translated_load_address(ctx: &RecompContext, vaddr: u64) -> Result<u64, DataAccessError> {
        Self::translated_backing_address(ctx, vaddr, DataAccessKind::Load)
    }

    fn translated_store_address(ctx: &RecompContext, vaddr: u64) -> Result<u64, DataAccessError> {
        Self::translated_backing_address(ctx, vaddr, DataAccessKind::Store)
    }

    /// Perform the architectural translation/access-bit checks for a store
    /// without touching backing memory. SC/SCD use this before consulting the
    /// LLbit: a failed conditional store still addresses memory and can raise
    /// a TLB refill, invalid, or modified exception.
    pub fn check_store_translation(ctx: &RecompContext, vaddr: u64) -> Result<(), DataAccessError> {
        ctx.translate_data_address(vaddr, DataAccessKind::Store)
            .map(|_| ())
    }

    /// Whether the aligned-word effective address is backed (LW/LWU/LL/…).
    #[inline]
    fn word_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 4)
    }

    /// Whether the aligned-doubleword effective address is backed (LD/SD/…).
    #[inline]
    fn dword_backed(&self, vaddr: u64) -> bool {
        self.virtual_range_backed(vaddr, 0, 8)
    }

    /// Checked LW/LWU (aligned word). See the module note on the block lane.
    #[inline]
    pub fn try_load_w(&self, vaddr: u64) -> Result<i32, u64> {
        assert_eq!(vaddr & 3, 0, "unaligned LW at {vaddr:#018x}");
        if let Some(value) = Self::read_mmio_word(vaddr) {
            return Ok(value as i32);
        }
        if self.word_backed(vaddr) {
            Ok(self.load_backed_word(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_w_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_w(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 4);
        Ok(value)
    }

    /// Checked LH (aligned, sign-extended halfword).
    #[inline]
    pub fn try_load_h(&self, vaddr: u64) -> Result<i16, u64> {
        if self.virtual_range_backed(vaddr, 2, 2) {
            Ok(self.load_h(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_h_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i16, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_h(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 2);
        Ok(value)
    }

    /// Checked LHU (aligned, zero-extended halfword).
    #[inline]
    pub fn try_load_hu(&self, vaddr: u64) -> Result<u16, u64> {
        self.try_load_h(vaddr).map(|v| v as u16)
    }

    pub fn try_load_hu_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u16, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_hu(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 2);
        Ok(value)
    }

    /// Checked LB (sign-extended byte).
    #[inline]
    pub fn try_load_b(&self, vaddr: u64) -> Result<i8, u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_b(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_b_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<i8, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_b(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 1);
        Ok(value)
    }

    /// Checked LBU (zero-extended byte).
    #[inline]
    pub fn try_load_bu(&self, vaddr: u64) -> Result<u8, u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            Ok(self.load_bu(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_bu_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u8, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_bu(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 1);
        Ok(value)
    }

    /// Checked LWL (the aligned word it merges from must be backed).
    #[inline]
    pub fn try_load_wl(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_wl_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_wl(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x3, 4);
        Ok(value)
    }

    /// Checked LWR.
    #[inline]
    pub fn try_load_wr(&self, initial: u64, vaddr: u64) -> Result<i32, u64> {
        if self.word_backed(vaddr & !0x3) {
            Ok(self.load_wr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_wr_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<i32, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_wr(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x3, 4);
        Ok(value)
    }

    /// Checked LD/LLD (aligned doubleword).
    #[inline]
    pub fn try_load_d(&self, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr) {
            Ok(self.load_d(vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_d_translated(
        &self,
        ctx: &RecompContext,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_d(translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated, 8);
        Ok(value)
    }

    /// Checked LDL.
    #[inline]
    pub fn try_load_dl(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dl(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_dl_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_dl(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x7, 8);
        Ok(value)
    }

    /// Checked LDR.
    #[inline]
    pub fn try_load_dr(&self, initial: u64, vaddr: u64) -> Result<u64, u64> {
        if self.dword_backed(vaddr & !0x7) {
            Ok(self.load_dr(initial, vaddr))
        } else {
            Err(vaddr)
        }
    }

    pub fn try_load_dr_translated(
        &self,
        ctx: &RecompContext,
        initial: u64,
        vaddr: u64,
    ) -> Result<u64, DataAccessError> {
        let translated = Self::translated_load_address(ctx, vaddr)?;
        let value = self
            .try_load_dr(initial, translated)
            .map_err(|_| DataAccessError::Unbacked { vaddr })?;
        Self::notify_translated_rdram_read(translated & !0x7, 8);
        Ok(value)
    }

    /// Checked SW.
    #[inline]
    pub fn try_store_w(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        assert_eq!(vaddr & 3, 0, "unaligned SW at {vaddr:#018x}");
        if Self::write_mmio_word(vaddr, val) {
            return Ok(());
        }
        if self.word_backed(vaddr) {
            self.store_backed_word(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_w_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_w(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SH.
    #[inline]
    pub fn try_store_h(&mut self, vaddr: u64, val: u16) -> Result<(), u64> {
        if self.virtual_range_backed(vaddr, 2, 2) {
            self.store_h(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_h_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u16,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_h(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SB.
    #[inline]
    pub fn try_store_b(&mut self, vaddr: u64, val: u8) -> Result<(), u64> {
        if self.virtual_range_backed(vaddr, 3, 1) {
            self.store_b(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_b_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u8,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_b(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SWL (reads and writes the aligned word it straddles).
    #[inline]
    pub fn try_store_wl(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_wl_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_wl(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SWR.
    #[inline]
    pub fn try_store_wr(&mut self, vaddr: u64, val: u32) -> Result<(), u64> {
        if self.word_backed(vaddr & !0x3) {
            self.store_wr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_wr_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u32,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_wr(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SD/SCD (aligned doubleword).
    #[inline]
    pub fn try_store_d(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr) {
            self.store_d(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_d_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_d(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SDL.
    #[inline]
    pub fn try_store_dl(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dl(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_dl_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_dl(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }

    /// Checked SDR.
    #[inline]
    pub fn try_store_dr(&mut self, vaddr: u64, val: u64) -> Result<(), u64> {
        if self.dword_backed(vaddr & !0x7) {
            self.store_dr(vaddr, val);
            Ok(())
        } else {
            Err(vaddr)
        }
    }

    pub fn try_store_dr_translated(
        &mut self,
        ctx: &RecompContext,
        vaddr: u64,
        val: u64,
    ) -> Result<(), DataAccessError> {
        let translated = Self::translated_store_address(ctx, vaddr)?;
        self.try_store_dr(translated, val)
            .map_err(|_| DataAccessError::Unbacked { vaddr })
    }
}

#[cfg(test)]
mod tests {
    use crate::boot::{
        BootCicIdentity, BootContext, BootCop0Context, BootRegion, BootTvStandard, Sha256Digest,
        BOOT_CONTEXT_SCHEMA_V1,
    };

    use super::{
        resolve_host_function, set_host_lookup, set_unsupported_observer, trap_unsupported,
        DataAccessError, DataAccessKind, GuestReadEvent, GuestWriteEvent,
        HostFunctionCatalogErrorV1, HostFunctionCatalogV1, InstructionTranslationDiagnosticErrorV1,
        Rdram, RecompContext, RecompFunc, TlbEntryRaw, TlbFault, TlbFaultKind,
        TranslatedDataAddress, TranslatedInstructionAddress, WriterChannel, RDRAM_LEN,
    };

    type RdramOperation = for<'a> fn(&mut Rdram<'a>);

    thread_local! {
        static OBSERVED_WRITES: std::cell::RefCell<Vec<GuestWriteEvent>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static OBSERVED_READS: std::cell::RefCell<Vec<GuestReadEvent>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static MMIO_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static UNSUPPORTED_CONTEXTS: std::cell::RefCell<Vec<String>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn observe_write(event: GuestWriteEvent) {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().push(event));
    }

    fn observe_read(event: GuestReadEvent) {
        OBSERVED_READS.with(|reads| reads.borrow_mut().push(event));
    }

    fn consume_mmio(_vaddr: u64, _value: u32) -> bool {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        true
    }

    fn read_mmio(_vaddr: u64) -> Option<u32> {
        MMIO_CALLS.with(|calls| calls.set(calls.get() + 1));
        Some(0)
    }

    fn observe_unsupported(context: &str) {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().push(context.to_owned()));
    }

    fn first_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn second_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn legacy_host_lookup(target: u32) -> Option<RecompFunc> {
        (target == 0x8000_3000).then_some(second_host)
    }

    fn context_from_evidence_for_test(
        snapshot: &super::RecompContextEvidenceSnapshotV1,
    ) -> RecompContext {
        RecompContext {
            r: snapshot.gprs,
            hi: snapshot.hi,
            lo: snapshot.lo,
            fpr: super::FprFile {
                fgr: snapshot.physical_fgrs,
            },
            fpu_cond: snapshot.fpu_cond,
            fcsr: snapshot.fcsr,
            ll_reservation: snapshot.ll_reservation,
            cop0_count: snapshot.cop0_count,
            // Boundary-owned clock phase is synchronized by the executor and
            // deliberately absent from RecompContext-owned evidence.
            cop0_count_phase: 0,
            cop0_compare: snapshot.cop0_compare,
            cop0_count_write: snapshot.cop0_count_write,
            cop0_compare_write: snapshot.cop0_compare_write,
            cop0_cond: snapshot.cop0_cond,
            cop0_status: snapshot.cop0_status,
            cop0_cause: snapshot.cop0_cause,
            cop0_epc: snapshot.cop0_epc,
            cop0_error_epc: snapshot.cop0_error_epc,
            cop0_badvaddr: snapshot.cop0_badvaddr,
            cop0_context: snapshot.cop0_context,
            cop0_xcontext: snapshot.cop0_xcontext,
            cop0_index: snapshot.cop0_index,
            tlb_entries: snapshot.tlb_entries,
            cop0_entry_lo0: snapshot.cop0_entry_lo0,
            cop0_entry_lo1: snapshot.cop0_entry_lo1,
            cop0_page_mask: snapshot.cop0_page_mask,
            cop0_wired: snapshot.cop0_wired,
            cop0_entry_hi: snapshot.cop0_entry_hi,
            cop0_random_phase: snapshot.cop0_random_phase,
            cop0_watch_lo: snapshot.cop0_watch_lo,
            cop0_watch_hi: snapshot.cop0_watch_hi,
            os_interrupt_mask: snapshot.os_interrupt_mask,
            thread_return_pc: snapshot.thread_return_pc,
            indirect_transfers: Vec::new(),
        }
    }

    #[test]
    fn recomp_context_evidence_v1_round_trips_and_detects_each_owned_field() {
        let mut context = RecompContext::new();
        context.r = std::array::from_fn(|index| index as u64 * 0x101 + 7);
        context.r[0] = 0;
        context.hi = 0x0102_0304_0506_0708;
        context.lo = 0x1112_1314_1516_1718;
        context.fpr.fgr =
            std::array::from_fn(|index| 0x8000_0000_0000_0000u64 | (index as u64 * 0x0101_0101));
        context.fpu_cond = true;
        context.fcsr = 0x0102_0304;
        context.ll_reservation = Some((0xffff_ffff_8123_4560, 8));
        context.cop0_count = 0x1111_1111;
        context.cop0_compare = 0x2222_2222;
        context.cop0_count_write = Some(0x3333_3333);
        context.cop0_compare_write = Some(0x4444_4444);
        context.cop0_cond = true;
        context.cop0_status = 0x5555_5555;
        context.cop0_cause = 0x6666_6666;
        context.cop0_epc = 0x7777_7777;
        context.cop0_error_epc = 0x8888_8888;
        context.cop0_badvaddr = 0x9999_9999_aaaa_aaaa;
        context.cop0_context = 0xbbbb_bbbb;
        context.cop0_xcontext = 0xcccc_cccc_dddd_dddd;
        context.cop0_index = 17;
        context.tlb_entries = std::array::from_fn(|index| TlbEntryRaw {
            page_mask: index as u32 * 0x2000,
            entry_hi: 0x1000_0000_0000_0000 | index as u64,
            entry_lo0: 0x2000_0000 | index as u32,
            entry_lo1: 0x3000_0000 | index as u32,
        });
        context.cop0_entry_lo0 = 0xdddd_dddd;
        context.cop0_entry_lo1 = 0xeeee_eeee;
        context.cop0_page_mask = 0x01ff_e000;
        context.cop0_wired = 11;
        context.cop0_entry_hi = 0xffff_ffff_0123_4567;
        context.cop0_random_phase = 9;
        context.cop0_watch_lo = 0x1234_5678;
        context.cop0_watch_hi = 0x9abc_def0;
        context.os_interrupt_mask = 0x1357_9bdf;
        context.thread_return_pc = Some(0xffff_fffc);

        let baseline = context.evidence_snapshot_v1();
        let restored = context_from_evidence_for_test(&baseline);
        assert_eq!(restored.evidence_snapshot_v1(), baseline);

        macro_rules! changed {
            ($change:expr) => {{
                let mut candidate = baseline.clone();
                $change(&mut candidate);
                assert_ne!(candidate, baseline);
            }};
        }
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.gprs[1] ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.lo ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.physical_fgrs[31] ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.fpu_cond = !s.fpu_cond);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.fcsr ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.ll_reservation = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_count ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_compare ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_count_write = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_compare_write = None);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_cond = !s.cop0_cond);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_status ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_cause ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_epc ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_error_epc ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_badvaddr ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_context ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_xcontext ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_index ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].page_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_lo0 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.tlb_entries[31].entry_lo1 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_lo0 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_lo1 ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_page_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_wired ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_entry_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_random_phase ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_watch_lo ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.cop0_watch_hi ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.os_interrupt_mask ^= 1);
        changed!(|s: &mut super::RecompContextEvidenceSnapshotV1| s.thread_return_pc = None);

        context.record_indirect_transfer(1, 2, 3, 4, Some(5));
        assert_eq!(context.evidence_snapshot_v1(), baseline);
    }

    #[test]
    fn host_function_catalog_canonicalizes_and_resolves_exact_targets() {
        let catalog =
            HostFunctionCatalogV1::new(vec![(0x8000_2000, second_host), (0x8000_1000, first_host)])
                .unwrap();

        assert_eq!(catalog.target_pcs(), &[0x8000_1000, 0x8000_2000]);
        assert_eq!(catalog.len(), 2);
        assert!(!catalog.is_empty());
        assert!(std::ptr::fn_addr_eq(
            catalog.resolve(0x8000_1000).unwrap(),
            first_host as RecompFunc
        ));
        assert!(std::ptr::fn_addr_eq(
            catalog.resolve(0x8000_2000).unwrap(),
            second_host as RecompFunc
        ));
        assert!(catalog.resolve(0x8000_1004).is_none());
    }

    #[test]
    fn host_function_catalog_rejects_misaligned_and_duplicate_targets() {
        assert!(matches!(
            HostFunctionCatalogV1::new(vec![(0x8000_1002, first_host)]),
            Err(HostFunctionCatalogErrorV1::MisalignedTarget {
                target: 0x8000_1002
            })
        ));
        assert!(matches!(
            HostFunctionCatalogV1::new(
                vec![(0x8000_1000, first_host), (0x8000_1000, second_host),]
            ),
            Err(HostFunctionCatalogErrorV1::DuplicateTarget {
                target: 0x8000_1000
            })
        ));
    }

    #[test]
    fn empty_host_function_catalog_is_an_exact_empty_inventory() {
        let catalog = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        assert!(catalog.is_empty());
        assert_eq!(catalog.len(), 0);
        assert!(catalog.target_pcs().is_empty());
        assert!(catalog.resolve(0x8000_1000).is_none());
    }

    #[test]
    fn host_function_catalog_does_not_install_or_replace_legacy_lookup() {
        let previous = set_host_lookup(Some(legacy_host_lookup));
        let catalog = HostFunctionCatalogV1::new(vec![(0x8000_1000, first_host)]).unwrap();

        assert!(catalog.resolve(0x8000_3000).is_none());
        assert!(std::ptr::fn_addr_eq(
            resolve_host_function(0x8000_3000).unwrap(),
            second_host as RecompFunc
        ));
        set_host_lookup(previous);
    }

    #[test]
    fn boot_context_restores_gpr_hilo_and_modeled_cp0_state() {
        let mut gprs = [0u64; 32];
        gprs[20] = 0xffff_ffff_cafe_babe;
        gprs[29] = 0xffff_ffff_a400_1ff0;
        let mut cp0 = [0u64; 32];
        cp0[0] = 7;
        cp0[1] = 19;
        cp0[4] = 0x1234_5678;
        cp0[6] = 4;
        cp0[8] = 0xaaaa_bbbb_cccc_dddd;
        cp0[9] = 0x0102_0304;
        cp0[10] = 0xeeee_ffff_0102_0304;
        cp0[11] = 0x0506_0708;
        cp0[12] = 0x3404_0000;
        cp0[13] = 0x0000_0300;
        cp0[20] = 0x1111_2222_3333_4444;
        let boot = BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "synthetic debugger".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes([0x11; 32]),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0x22; 32]),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: 0x8000_0400,
            gprs,
            hi: 0x1234,
            lo: 0x5678,
            cp0: BootCop0Context { registers: cp0 },
        };

        let mut ctx = RecompContext::new();
        ctx.restore_boot_context(&boot).unwrap();

        assert_eq!(ctx.gprs(), gprs);
        assert_eq!(ctx.hi, 0x1234);
        assert_eq!(ctx.lo, 0x5678);
        assert_eq!(ctx.cop0_random(), 19);
        assert_eq!(ctx.cop0_index, 7);
        assert_eq!(ctx.cop0_context, 0x1234_5678);
        assert_eq!(ctx.cop0_badvaddr, 0xaaaa_bbbb_cccc_dddd);
        assert_eq!(ctx.cop0_count, 0x0102_0304);
        assert_eq!(ctx.cop0_entry_hi, 0xeeee_ffff_0102_0304);
        assert_eq!(ctx.cop0_compare, 0x0506_0708);
        assert_eq!(ctx.cop0_status, 0x3404_0000);
        assert_eq!(ctx.cop0_cause, 0x0000_0300);
        assert_eq!(ctx.cop0_xcontext, 0x1111_2222_3333_4444);
        assert!(ctx.cop0_cond);
        assert!(ctx.boot_context_state_mismatches(&boot).unwrap().is_empty());

        ctx.set_r(20, 0);
        assert_eq!(
            ctx.boot_context_state_mismatches(&boot).unwrap(),
            vec![crate::boot::BootContextStateMismatch {
                field: crate::boot::BootContextStateField::Gpr(20),
                expected: 0xffff_ffff_cafe_babe,
                actual: 0,
            }]
        );
    }

    #[test]
    fn unsupported_observer_runs_before_the_named_panic() {
        UNSUPPORTED_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
        let previous = set_unsupported_observer(Some(observe_unsupported));
        let panic = std::panic::catch_unwind(|| trap_unsupported("unsupported COP0 register 7"));
        set_unsupported_observer(previous);

        assert!(panic.is_err());
        UNSUPPORTED_CONTEXTS.with(|contexts| {
            assert_eq!(
                contexts.borrow().as_slice(),
                ["unsupported COP0 register 7"]
            );
        });
    }

    #[test]
    fn exception_return_prefers_error_epc_and_preserves_exl_under_erl() {
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 2);
        ctx.cop0_epc = 0x8000_1000;
        ctx.cop0_error_epc = 0xBFC0_0200;
        ctx.set_ll_reservation(0x8000_0040, 4);

        assert_eq!(ctx.exception_return_pc(), 0xBFC0_0200);
        assert_eq!(ctx.cop0_status & (1 << 2), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert!(!ctx.take_ll_reservation(0x8000_0040, 4));
    }

    #[test]
    fn cop0_status_and_software_interrupt_writes_preserve_hardware_pending() {
        let mut ctx = RecompContext::new();
        ctx.write_cop0(12, 0x3400_FF01);
        assert_eq!(ctx.read_cop0(12), 0x3400_FF01);

        ctx.cop0_cause = (1 << 10) | (9 << 2) | (1 << 31);
        ctx.write_cop0(13, 0b10 << 8);
        assert_eq!(ctx.cop0_cause & (0b11 << 8), 0b10 << 8);
        assert_ne!(ctx.cop0_cause & (1 << 10), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn cop0_timing_writes_retain_same_value_compare_acknowledgements() {
        let mut ctx = RecompContext::new();
        ctx.synchronize_cop0_timing(7, 0, 9);
        ctx.cop0_cause = 1 << 15;
        ctx.write_cop0(9, 7);
        ctx.write_cop0(11, 9);

        assert_eq!(ctx.cop0_cause & (1 << 15), 0);
        assert_eq!(ctx.take_cop0_timing_writes(), (Some(7), Some(9)));
        assert_eq!(ctx.take_cop0_timing_writes(), (None, None));
    }

    #[test]
    fn interior_count_reads_include_the_live_half_rate_phase() {
        let mut ctx = RecompContext::new();

        ctx.synchronize_cop0_timing(7, 0, 9);
        assert_eq!(ctx.read_cop0_count_interior(0), 7);
        assert_eq!(ctx.read_cop0_count_interior(1), 7);
        assert_eq!(ctx.read_cop0_count_interior(2), 8);

        ctx.synchronize_cop0_timing(7, 1, 9);
        assert_eq!(ctx.read_cop0_count_interior(0), 7);
        assert_eq!(ctx.read_cop0_count_interior(1), 8);
        assert_eq!(ctx.read_cop0_count_interior(2), 8);
    }

    #[test]
    #[should_panic(expected = "CP0 Count half-rate phase must be zero or one")]
    fn cop0_timing_sync_rejects_an_invalid_half_rate_phase() {
        let mut ctx = RecompContext::new();
        ctx.synchronize_cop0_timing(0, 2, 0);
    }

    #[test]
    fn rdram_write_observer_runs_after_committed_logical_ranges() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous = super::set_write_observer(Some(observe_write));
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_8000_0000, 0x1122_3344);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_h(0xFFFF_FFFF_8000_0004, 0x5566);
        mem.store_b(0xFFFF_FFFF_8000_0006, 0x77);
        mem.store_d(0xFFFF_FFFF_A000_0008, 0x8899_aabb_ccdd_eeff);

        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, 0x1122_3344);
        assert_eq!(mem.load_hu(0xFFFF_FFFF_8000_0004), 0x5566);
        assert_eq!(mem.load_bu(0xFFFF_FFFF_8000_0006), 0x77);
        assert_eq!(mem.load_d(0xFFFF_FFFF_8000_0008), 0x8899_aabb_ccdd_eeff);
        assert_eq!(
            OBSERVED_WRITES.with(|writes| writes.borrow().clone()),
            vec![
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 0,
                    len: 4,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    channel: WriterChannel::CpuInstructionStore,
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::NonRdpWrite16 {
                    channel: WriterChannel::CpuInstructionStore,
                    logical_offset: 4,
                    value: 0x5566,
                },
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 6,
                    len: 1,
                },
                GuestWriteEvent::Range {
                    channel: WriterChannel::CpuInstructionStore,
                    physical_offset: 8,
                    len: 8,
                },
            ]
        );
        super::set_write_observer(previous);
    }

    #[test]
    fn translated_rdram_read_observer_covers_every_ordinary_load() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 64];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();
        let base = 0xffff_ffff_8000_0000;

        assert!(mem.try_load_w_translated(&ctx, base).is_ok());
        assert!(mem.try_load_h_translated(&ctx, base + 4).is_ok());
        assert!(mem.try_load_hu_translated(&ctx, base + 6).is_ok());
        assert!(mem.try_load_b_translated(&ctx, base + 8).is_ok());
        assert!(mem.try_load_bu_translated(&ctx, base + 9).is_ok());
        assert!(mem.try_load_d_translated(&ctx, base + 16).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 4,
                    len: 2,
                },
                GuestReadEvent {
                    physical_offset: 6,
                    len: 2,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 1,
                },
                GuestReadEvent {
                    physical_offset: 9,
                    len: 1,
                },
                GuestReadEvent {
                    physical_offset: 16,
                    len: 8,
                },
            ]
        );
    }

    #[test]
    fn translated_rdram_read_observer_reports_tlb_mapped_physical_offset() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 0x2000];
        let mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: (1 << 6) | 0b111,
            entry_lo1: 0b111,
        };

        assert!(mem.try_load_w_translated(&ctx, 0x0040_0020).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![GuestReadEvent {
                physical_offset: 0x1020,
                len: 4,
            }]
        );
    }

    #[test]
    fn translated_rdram_read_observer_unaligned_loads_cover_aligned_backing_ranges() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 32];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();
        let base = 0xffff_ffff_a000_0000;

        assert!(mem.try_load_wl_translated(&ctx, 0, base + 1).is_ok());
        assert!(mem.try_load_wr_translated(&ctx, 0, base + 2).is_ok());
        assert!(mem.try_load_dl_translated(&ctx, 0, base + 11).is_ok());
        assert!(mem.try_load_dr_translated(&ctx, 0, base + 14).is_ok());
        super::set_read_observer(previous);

        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 8,
                },
                GuestReadEvent {
                    physical_offset: 8,
                    len: 8,
                },
            ]
        );
    }

    #[test]
    fn translated_rdram_read_observer_ignores_failed_loads_and_host_snapshots() {
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = super::set_read_observer(Some(observe_read));
        let mut bytes = [0u8; 16];
        let mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();

        assert!(mem.try_load_w_translated(&ctx, 0x0040_0000).is_err());
        assert!(mem
            .try_load_w_translated(&ctx, 0xffff_ffff_8000_0040)
            .is_err());
        assert_eq!(mem.copy_physical_bytes(0, 4), vec![0; 4]);
        super::set_read_observer(previous);

        assert!(OBSERVED_READS.with(|reads| reads.borrow().is_empty()));
    }

    #[test]
    fn external_write_gateways_attribute_the_exact_fixed_denominator() {
        let gateways: [(WriterChannel, fn(u32, u32)); 8] = [
            (
                WriterChannel::CpuInstructionStore,
                super::notify_cpu_instruction_store,
            ),
            (WriterChannel::PiDma, super::notify_pi_dma_write),
            (WriterChannel::SiDma, super::notify_si_dma_write),
            (WriterChannel::SpDma, super::notify_sp_dma_write),
            (
                WriterChannel::RspExecutionOrHleWriteback,
                super::notify_rsp_execution_or_hle_writeback,
            ),
            (WriterChannel::RdpRenderer, super::notify_rdp_renderer_write),
            (WriterChannel::HostAbi, super::notify_host_abi_write),
            (
                WriterChannel::BootstrapOrImport,
                super::notify_bootstrap_or_import_write,
            ),
        ];
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous = super::set_write_observer(Some(observe_write));

        for (index, (_, gateway)) in gateways.iter().enumerate() {
            gateway(0x1000 + index as u32 * 4, 4);
        }
        // Preserve the existing zero-length notification behavior: it is not
        // a byte-producing event and therefore enters neither observer.
        super::notify_host_abi_write(0x2000, 0);

        let observed = OBSERVED_WRITES.with(|writes| writes.borrow().clone());
        assert_eq!(observed.len(), gateways.len());
        for (index, (event, (expected_channel, _))) in observed.iter().zip(gateways).enumerate() {
            assert_eq!(event.channel(), expected_channel);
            assert_eq!(event.range(), (0x1000 + index as u32 * 4, 4));
        }
        super::set_write_observer(previous);
    }

    #[test]
    fn write_events_canonicalize_cached_and_uncached_rdram_aliases() {
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0xffff_ffff_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_a440_0000), None);
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_8000_1234),
            Some(0x1234)
        );
        assert_eq!(
            Rdram::physical_rdram_offset(0x0000_0000_a000_1234),
            Some(0x1234)
        );
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0000_0000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0xffff_ffff_c000_1234), None);
        assert_eq!(Rdram::physical_rdram_offset(0x0000_0001_8000_1234), None);
    }

    #[test]
    fn sparse_direct_windows_share_one_classifier_across_canonical_forms() {
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0x0000_0000_a600_0000),
            Some(0x2600_0000)
        );
        assert_eq!(
            Rdram::direct_storage_offset(0xffff_ffff_8600_0000),
            Some(0x0600_0000)
        );
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_a460_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0x0000_0001_a600_0000), None);
        assert_eq!(Rdram::direct_storage_offset(0xffff_ffff_c600_0000), None);

        let mut bytes = vec![0u8; RDRAM_LEN + 4];
        bytes[RDRAM_LEN..RDRAM_LEN + 4].copy_from_slice(&0x1234_5678u32.to_ne_bytes());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xffff_ffff_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0x0000_0000_8080_0000) as u32, 0x1234_5678);
        assert_eq!(mem.try_load_w(0xffff_ffff_8080_0000), Ok(0x1234_5678));
    }

    #[test]
    fn kseg0_and_kseg1_loads_and_stores_share_visible_bytes() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let kseg0 = 0xffff_ffff_8000_0000;
        let kseg1 = 0xffff_ffff_a000_0000;

        mem.store_w(kseg1, 0x1122_3344);
        assert_eq!(mem.load_w(kseg0) as u32, 0x1122_3344);
        mem.store_h(kseg0 + 4, 0x8567);
        assert_eq!(mem.load_hu(kseg1 + 4), 0x8567);
        mem.store_b(kseg1 + 6, 0xa9);
        assert_eq!(mem.load_bu(kseg0 + 6), 0xa9);

        mem.store_w(0x0000_0000_8000_0008, 0xdead_beef);
        assert_eq!(mem.load_w(0x0000_0000_a000_0008) as u32, 0xdead_beef);
    }

    #[test]
    fn mapped_data_translation_selects_page_half_size_asid_and_access_bits() {
        let mut ctx = RecompContext::new();
        ctx.cop0_entry_hi = 0x0000_002a;
        ctx.tlb_entries[3] = TlbEntryRaw {
            page_mask: 0x0000_6000, // paired 16 KiB pages
            entry_hi: 0x0040_002a,
            entry_lo0: (0x20 << 6) | 0x6,
            entry_lo1: (0x30 << 6) | 0x2,
        };

        assert_eq!(
            ctx.translate_data_address(0x0040_1234, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x0002_1234))
        );
        assert_eq!(
            ctx.translate_data_address(0x0040_5234, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x0003_1234))
        );
        assert_eq!(
            ctx.translate_data_address(0x0040_5234, DataAccessKind::Store),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_5234,
                access: DataAccessKind::Store,
                kind: TlbFaultKind::Modified,
                extended: false,
            }))
        );

        ctx.cop0_entry_hi = 0x0000_002b;
        assert_eq!(
            ctx.translate_data_address(0x0040_1234, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_1234,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: false,
            }))
        );
    }

    #[test]
    fn libultra_invalid_tlb_layout_does_not_create_a_zero_address_multi_match() {
        let mut ctx = RecompContext::new();
        ctx.initialize_invalid_tlb_entries();
        assert_eq!(
            ctx.translate_data_address(4, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 4,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Invalid,
                extended: false,
            }))
        );
    }

    #[test]
    fn mapped_physical_address_above_direct_window_is_unbacked_not_aliased() {
        let mut ctx = RecompContext::new();
        ctx.cop0_entry_hi = 0x0040_002a;
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_002a,
            // Figure 3-10 PFN bit 17 becomes PA(29), the first physical byte
            // beyond the N64's 29-bit direct window.
            entry_lo0: (0x0002_0000 << 6) | 0x7,
            entry_lo1: 0x7,
        };
        assert_eq!(
            ctx.translate_data_address(0x0040_0000, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x2000_0000))
        );

        let mut bytes = 0x1000_0000u32.to_ne_bytes();
        let mem = Rdram::new(&mut bytes);
        assert_eq!(
            mem.try_load_w_translated(&ctx, 0x0040_0000),
            Err(DataAccessError::Unbacked { vaddr: 0x0040_0000 })
        );
        assert_eq!(bytes, 0x1000_0000u32.to_ne_bytes());
    }

    #[test]
    fn direct_segments_bypass_tlb_while_mapped_invalid_is_typed() {
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0xc000_0000,
            entry_lo0: 1,
            entry_lo1: 1,
        };

        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_8000_0040, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Direct(0xffff_ffff_8000_0040))
        );
        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_a000_0040, DataAccessKind::Store),
            Ok(TranslatedDataAddress::Direct(0xffff_ffff_a000_0040))
        );
        assert_eq!(
            ctx.translate_data_address(0xffff_ffff_c000_0040, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0xffff_ffff_c000_0040,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Invalid,
                extended: false,
            }))
        );
    }

    #[test]
    fn extended_segments_enforce_region_privilege_and_xkphys_width() {
        const STATUS_KSU_USER: u32 = 0b10 << 3;
        const STATUS_KSU_SUPERVISOR: u32 = 0b01 << 3;
        const STATUS_UX: u32 = 1 << 5;
        const STATUS_SX: u32 = 1 << 6;
        const STATUS_KX: u32 = 1 << 7;
        const USER_VA: u64 = 0x0000_0012_3456_0040;
        const SUPERVISOR_VA: u64 = 0x4000_0012_3456_0040;

        let mut user = RecompContext::new();
        user.cop0_status = STATUS_KSU_USER | STATUS_UX;
        user.cop0_entry_hi = 0x2a;
        user.tlb_entries[4] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (USER_VA & 0xc000_00ff_ffff_e000) | 0x2a,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            user.translate_data_address(USER_VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );
        assert_eq!(
            user.translate_data_address(SUPERVISOR_VA, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: SUPERVISOR_VA,
                access: DataAccessKind::Load,
            })
        );
        assert_eq!(
            user.translate_data_address(0x9000_0000_0000_0040, DataAccessKind::Store),
            Err(DataAccessError::AddressError {
                vaddr: 0x9000_0000_0000_0040,
                access: DataAccessKind::Store,
            })
        );

        let mut supervisor = RecompContext::new();
        supervisor.cop0_status = STATUS_KSU_SUPERVISOR | STATUS_SX;
        supervisor.cop0_entry_hi = 0x2a;
        supervisor.tlb_entries[4] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (SUPERVISOR_VA & 0xc000_00ff_ffff_e000) | 0x2a,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            supervisor.translate_data_address(SUPERVISOR_VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );
        assert!(matches!(
            supervisor.translate_data_address(0xc000_0012_3456_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError { .. })
        ));

        let mut kernel = RecompContext::new();
        kernel.cop0_status = STATUS_KX;
        assert_eq!(
            kernel.translate_data_address(0x9000_0000_0000_0040, DataAccessKind::Load),
            Ok(TranslatedDataAddress::DirectPhysical(0x40))
        );
        assert_eq!(
            kernel.translate_data_address(0x9000_0001_0000_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: 0x9000_0001_0000_0040,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn extended_tlb_faults_retain_full_address_and_refill_class() {
        const STATUS_KSU_USER: u32 = 0b10 << 3;
        const STATUS_UX: u32 = 1 << 5;
        const VA: u64 = 0x0000_0088_7654_2040;

        let mut ctx = RecompContext::new();
        ctx.cop0_status = STATUS_KSU_USER | STATUS_UX;
        ctx.cop0_entry_hi = 0x51;
        assert_eq!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: VA,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: true,
            }))
        );

        ctx.tlb_entries[2] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: (VA & 0xc000_00ff_ffff_e000) | 0x51,
            entry_lo0: 0x6,
            entry_lo1: 0x46,
        };
        assert_eq!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Ok(TranslatedDataAddress::Mapped(0x40))
        );

        ctx.tlb_entries[2].entry_hi |= 0x4000_0000_0000_0000;
        assert!(matches!(
            ctx.translate_data_address(VA, DataAccessKind::Load),
            Err(DataAccessError::Tlb(TlbFault {
                kind: TlbFaultKind::Refill,
                extended: true,
                ..
            }))
        ));
    }

    #[test]
    fn erl_directs_only_the_low_user_segment_in_both_address_widths() {
        const STATUS_ERL: u32 = 1 << 2;
        const STATUS_KX: u32 = 1 << 7;

        for status in [STATUS_ERL, STATUS_ERL | STATUS_KX] {
            let mut ctx = RecompContext::new();
            ctx.cop0_status = status;
            assert_eq!(
                ctx.translate_data_address(0x1234_5040, DataAccessKind::Load),
                Ok(TranslatedDataAddress::DirectPhysical(0x1234_5040))
            );
        }

        let mut extended = RecompContext::new();
        extended.cop0_status = STATUS_ERL | STATUS_KX;
        assert_eq!(
            extended.translate_data_address(0x0000_0000_8000_0040, DataAccessKind::Load),
            Err(DataAccessError::AddressError {
                vaddr: 0x0000_0000_8000_0040,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn doubleword_cop0_moves_round_trip_entry_hi_and_xcontext() {
        let mut ctx = RecompContext::new();
        ctx.write_cop0_64(10, 0xc000_0088_7654_3051);
        ctx.write_cop0_64(20, 0x1234_5679_0abc_def0);
        assert_eq!(ctx.read_cop0_64(10), 0xc000_0088_7654_3051);
        assert_eq!(ctx.read_cop0_64(20), 0x1234_5679_0abc_def0);
        assert_eq!(ctx.read_cop0(10), 0x7654_3051);
        assert_eq!(ctx.read_cop0(20), 0x0abc_def0);
    }

    #[test]
    fn instruction_translation_returns_physical_identity_for_direct_and_mapped_aliases() {
        let mut ctx = RecompContext::new();
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: ((0x0010_0000 >> 6) & 0x03ff_ffc0) | 0b111,
            entry_lo1: ((0x0030_0000 >> 6) & 0x03ff_ffc0) | 0b111,
        };

        assert_eq!(
            ctx.translate_instruction_address(0x8000_0040),
            Ok(TranslatedInstructionAddress::new(0x40))
        );
        assert_eq!(
            ctx.translate_instruction_address(0xa000_0040),
            Ok(TranslatedInstructionAddress::new(0x40))
        );
        assert_eq!(
            ctx.translate_instruction_address(0x0040_0ffc),
            Ok(TranslatedInstructionAddress::new(0x0010_0ffc))
        );
        assert_eq!(
            ctx.translate_instruction_address(0x0040_1000),
            Ok(TranslatedInstructionAddress::new(0x0030_0000))
        );
    }

    #[test]
    fn diagnostic_instruction_translation_types_undefined_tlb_inputs() {
        let mut unsupported = RecompContext::new();
        unsupported.initialize_invalid_tlb_entries();
        unsupported.tlb_entries[4].page_mask = 0x0000_2000;
        assert_eq!(
            unsupported.translate_instruction_address_diagnostic_v1(0x0040_0000),
            Err(
                InstructionTranslationDiagnosticErrorV1::InvalidPageMaskEncoding {
                    index: 4,
                    page_mask_raw: 0x0000_2000,
                }
            )
        );

        let mut competing = RecompContext::new();
        competing.initialize_invalid_tlb_entries();
        let entry = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: ((0x0010_0000 >> 6) & 0x03ff_ffc0) | 0b111,
            entry_lo1: ((0x0030_0000 >> 6) & 0x03ff_ffc0) | 0b111,
        };
        competing.tlb_entries[1] = entry;
        competing.tlb_entries[2] = entry;
        assert_eq!(
            competing.translate_instruction_address_diagnostic_v1(0x0040_0040),
            Err(
                InstructionTranslationDiagnosticErrorV1::MultipleTlbMatches {
                    vaddr: 0x0040_0040,
                    first_index: 1,
                    second_index: 2,
                }
            )
        );
    }

    #[test]
    fn unsupported_instruction_width_stays_loud_while_privilege_is_typed() {
        let result = std::panic::catch_unwind(|| {
            RecompContext::new().translate_instruction_address(0x0000_0001_0000_0000)
        });
        let message = result
            .expect_err("64-bit instruction translation must remain loud")
            .downcast::<String>()
            .map(|message| *message)
            .unwrap_or_else(|payload| {
                payload
                    .downcast::<&'static str>()
                    .map(|message| (*message).to_owned())
                    .unwrap_or_default()
            });
        assert!(message.contains("64-bit instruction address translation is unsupported"));

        let mut user = RecompContext::new();
        user.cop0_status = 0b10 << 3;
        assert_eq!(
            user.translate_instruction_address(0x0040_0000),
            Err(DataAccessError::Tlb(TlbFault {
                vaddr: 0x0040_0000,
                access: DataAccessKind::Load,
                kind: TlbFaultKind::Refill,
                extended: false,
            }))
        );
        assert_eq!(
            user.translate_instruction_address(0xffff_ffff_8000_0000),
            Err(DataAccessError::AddressError {
                vaddr: 0xffff_ffff_8000_0000,
                access: DataAccessKind::Load,
            })
        );
    }

    #[test]
    fn mapped_low_physical_addresses_trap_instead_of_aliasing_rdram() {
        let mut bytes = [0u8; 4];
        let mem = Rdram::new(&mut bytes);
        for address in [
            0x0000_0000_0000_0000,
            0xffff_ffff_c000_0000,
            0x0000_0001_8000_0000,
        ] {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = mem.load_w(address);
            }));
            assert!(
                panic.is_err(),
                "mapped address {address:#018x} did not trap"
            );
        }
    }

    #[test]
    fn checked_accessors_return_typed_faults_for_non_rdram_segments() {
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let mmio = 0xffff_ffff_a460_0010;

        assert_eq!(mem.try_load_w(mmio), Err(mmio));
        assert_eq!(mem.try_load_h(mmio), Err(mmio));
        assert_eq!(mem.try_load_hu(mmio), Err(mmio));
        assert_eq!(mem.try_load_b(mmio), Err(mmio));
        assert_eq!(mem.try_load_bu(mmio), Err(mmio));
        assert_eq!(mem.try_load_wl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_wr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_load_d(mmio), Err(mmio));
        assert_eq!(mem.try_load_dl(0, mmio + 1), Err(mmio + 1));
        assert_eq!(mem.try_load_dr(0, mmio + 2), Err(mmio + 2));
        assert_eq!(mem.try_store_w(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_h(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_b(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_wl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_wr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.try_store_d(mmio, 0), Err(mmio));
        assert_eq!(mem.try_store_dl(mmio + 1, 0), Err(mmio + 1));
        assert_eq!(mem.try_store_dr(mmio + 2, 0), Err(mmio + 2));
        assert_eq!(mem.as_mut_slice(), [0; 16]);
    }

    #[test]
    fn checked_word_accessors_route_translated_mmio_before_backing_rejection() {
        const SI_STATUS: u64 = 0xffff_ffff_a480_0018;

        MMIO_CALLS.with(|calls| calls.set(0));
        let previous_mmio = super::set_mmio_hooks(Some(read_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 16];
        let mut mem = Rdram::new(&mut bytes);
        let ctx = RecompContext::new();

        assert_eq!(mem.try_load_w_translated(&ctx, SI_STATUS), Ok(0));
        assert_eq!(mem.try_store_w_translated(&ctx, SI_STATUS, 3), Ok(()));
        assert_eq!(MMIO_CALLS.with(std::cell::Cell::get), 2);
        assert_eq!(mem.as_mut_slice(), [0; 16]);

        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
    }

    #[test]
    fn nonword_rcp_and_pif_accesses_trap_before_any_side_effect() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        MMIO_CALLS.with(|calls| calls.set(0));
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(Some(read_mmio), Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        let operations: [RdramOperation; 8] = [
            |mem| {
                let _ = mem.load_h(0xffff_ffff_a400_0000);
            },
            |mem| {
                let _ = mem.load_b(0xffff_ffff_9fc0_07c0);
            },
            |mem| mem.store_h(0xffff_ffff_a440_0000, 1),
            |mem| mem.store_b(0xffff_ffff_bfc0_07c0, 1),
            |mem| {
                let _ = mem.load_d(0xffff_ffff_a400_0000);
            },
            |mem| mem.store_d(0xffff_ffff_bfc0_07c0, 1),
            |mem| mem.store_wl(0xffff_ffff_a440_0001, 1),
            |mem| mem.store_wr(0xffff_ffff_a440_0002, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "non-word MMIO access did not trap");
        }

        assert_eq!(MMIO_CALLS.with(std::cell::Cell::get), 0);
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(mem.as_mut_slice(), [0; 4]);

        mem.store_wl(0xffff_ffff_a440_0000, 0x1122_3344);
        mem.store_wr(0xffff_ffff_a440_0003, 0x5566_7788);
        assert_eq!(
            MMIO_CALLS.with(std::cell::Cell::get),
            2,
            "full-selector SWL/SWR must issue one write each with no MMIO pre-read"
        );
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn misaligned_aligned_accessors_trap_before_bytes_or_events_change() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let mut bytes = [0x5au8; 16];
        let before = bytes;
        let mut mem = Rdram::new(&mut bytes);
        let operations: [RdramOperation; 6] = [
            |mem| {
                let _ = mem.load_h(0xffff_ffff_8000_0001);
            },
            |mem| mem.store_h(0xffff_ffff_a000_0001, 1),
            |mem| {
                let _ = mem.load_w(0xffff_ffff_8000_0002);
            },
            |mem| mem.store_w(0xffff_ffff_a000_0002, 1),
            |mem| {
                let _ = mem.load_d(0xffff_ffff_8000_0004);
            },
            |mem| mem.store_d(0xffff_ffff_a000_0004, 1),
        ];
        for operation in operations {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mut mem);
            }));
            assert!(panic.is_err(), "misaligned access did not trap");
        }
        assert_eq!(bytes, before);
        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn consumed_mmio_store_does_not_report_an_rdram_write() {
        OBSERVED_WRITES.with(|writes| writes.borrow_mut().clear());
        let previous_observer = super::set_write_observer(Some(observe_write));
        let previous_mmio = super::set_mmio_hooks(None, Some(consume_mmio));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A460_0000, 0x1234_5678);

        assert!(OBSERVED_WRITES.with(|writes| writes.borrow().is_empty()));
        assert_eq!(bytes, [0; 4]);
        super::set_mmio_hooks(previous_mmio.0, previous_mmio.1);
        super::set_write_observer(previous_observer);
    }

    #[test]
    fn guest_write_tokens_change_only_for_intersecting_pages_and_new_sessions() {
        let previous = super::set_guest_write_boundary_observer(None);
        let first = super::guest_write_token(0x2000, 0x1000);
        super::notify_host_abi_write(0x5000, 4);
        assert_eq!(super::guest_write_token(0x2000, 0x1000), first);
        super::notify_host_abi_write(0x2fff, 2);
        let written = super::guest_write_token(0x2000, 0x1000);
        assert_ne!(written, first);

        super::set_guest_write_boundary_observer(None);
        assert_ne!(super::guest_write_token(0x2000, 0x1000), written);
        super::set_guest_write_boundary_observer(previous);
    }
}
