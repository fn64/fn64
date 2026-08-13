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
    #[inline]
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

    /// Classify `vaddr` and take the two direct routes inline; the mapped
    /// route (needing a TLB scan) is out of line in
    /// [`Self::translate_mapped_data_address_diagnostic`].
    ///
    /// Split from one function so the common case -- kseg0/kseg1 and
    /// XKPHYS/compatibility direct addresses, which is what every ordinary
    /// N64 title's load/store traffic resolves to -- stays small enough for
    /// LLVM to inline into its many `_translated` call sites. The undivided
    /// function's body (TLB linear scan, up to 32 entries, plus every TLB
    /// fault's error construction) was too large for automatic inlining to
    /// consider profitable regardless of `#[inline]` on the callers; a
    /// live `sample` on WM2000 (windowed RT64) found this address-
    /// translation cost the single largest per-instruction cost after
    /// verify_precompiled_instruction_word (see `wm2000-block-shards/
    /// build.rs`'s SMC-verify default) was already turned off for this
    /// title. Splitting does not change any observable result: every
    /// TranslatedDataAddress/error variant this used to return, it still
    /// returns, from whichever function now owns that branch.
    #[inline]
    fn translate_data_address_diagnostic(
        &self,
        vaddr: u64,
        access: DataAccessKind,
    ) -> Result<TranslatedDataAddress, InstructionTranslationDiagnosticErrorV1> {
        let route = self.classify_data_address(vaddr).map_err(|()| {
            InstructionTranslationDiagnosticErrorV1::Access(DataAccessError::AddressError {
                vaddr,
                access,
            })
        })?;
        match route {
            AddressRoute::DirectVirtual(address) => Ok(TranslatedDataAddress::Direct(address)),
            AddressRoute::DirectPhysical(physical) => {
                Ok(TranslatedDataAddress::DirectPhysical(physical))
            }
            AddressRoute::Mapped { extended } => {
                self.translate_mapped_data_address_diagnostic(vaddr, access, extended)
            }
        }
    }

    /// The out-of-line TLB-scan path `translate_data_address_diagnostic`
    /// delegates `AddressRoute::Mapped` to. See that function's doc for why
    /// this is a separate, deliberately non-`#[inline]` function.
    fn translate_mapped_data_address_diagnostic(
        &self,
        vaddr: u64,
        access: DataAccessKind,
        extended: bool,
    ) -> Result<TranslatedDataAddress, InstructionTranslationDiagnosticErrorV1> {
        const PAGE_MASK_BITS: u32 = 0x01ff_e000;
        const VPN2_32_BITS: u64 = 0x0000_0000_ffff_e000;
        const VPN2_64_BITS: u64 = 0x0000_00ff_ffff_e000;
        const REGION_BITS: u64 = 0xc000_0000_0000_0000;
        const ASID_BITS: u64 = 0xff;
        const GLOBAL: u32 = 1;
        const VALID: u32 = 1 << 1;
        const DIRTY: u32 = 1 << 2;

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
}

mod fpu_ops;
mod host;
pub use fpu_ops::{round_ties_even_f32, round_ties_even_f64};
pub use host::*;

#[cfg(test)]
mod tests;
