//! Bank-qualified execution identities and code-image admission.
//!
//! Historical function boundaries are useful decompilation evidence, but they
//! are not an architectural property of the VR4300.  A general translator
//! must be able to resume any aligned instruction in the *currently loaded*
//! code image, including when two overlays occupy the same virtual address.
//! This module establishes that identity for the block runner: every
//! destination is an [`ExecutionKey`] (`BankId`, `GuestPc`), and a
//! [`CodeCatalog`] resolves it without consulting a function symbol table.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::num::NonZeroUsize;

use sha2::{Digest, Sha256};

#[cfg(feature = "dev-interpreter")]
use crate::fetch::{admit_mapped_unit, run_admitted_mapped_unit};
use crate::fetch::{
    MappedAotBlock, MappedAotEvidenceSnapshot, PhysicalCodeBank, PhysicalCodeBankEvidenceSnapshot,
    PhysicalCodeCatalog, PhysicalCodeError,
};
use crate::generation::{BackedPrecompiledGenerationCatalogV1, GenerationCatalogError};
use crate::runtime::{HostFunctionCatalogV1, Rdram, RecompContext};
use crate::{static_execution_build_receipt, StaticExecutionBuildReceipt};

/// Decoder-level classification for one aligned bank word.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankWordKind {
    Straight,
    ControlTransfer,
    Unknown,
}

pub const CATALOG_RESOLVER_POLICY_NAME_V1: &str = "fn64_dense_aot_catalog_resolver_v1";

/// Every architectural exception destination selected by the arbitrary-PC
/// lane. Cache-error entry is absent because this CPU model cannot produce it.
/// The resolver policy evidence and the live exception selectors below share
/// this one implementation-owned denominator.
pub const CATALOG_RESOLVER_EXCEPTION_VECTORS_V1: [u32; 6] = [
    0x8000_0000,
    0x8000_0080,
    0x8000_0180,
    0xbfc0_0200,
    0xbfc0_0280,
    0xbfc0_0380,
];

/// Implementation-issued evidence for the callback-free catalog resolver.
///
/// Private fields prevent a caller from promoting booleans into resolver
/// authority. The sole constructor below is co-located with the admission,
/// fault, return-boundary, and exception-vector implementation it describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogResolverPolicyEvidenceV1 {
    policy: &'static str,
    exception_vectors: [u32; 6],
    aligned_pc_admission: bool,
    exact_active_owner_resolution: bool,
    explicit_thread_return_boundary: bool,
    misaligned_target_fault: bool,
    unmapped_or_ambiguous_target_fault: bool,
    traps_enter_shared_resolver: bool,
    build_receipt: StaticExecutionBuildReceipt,
}

impl CatalogResolverPolicyEvidenceV1 {
    pub const fn policy(&self) -> &'static str {
        self.policy
    }

    pub const fn exception_vectors(&self) -> &[u32; 6] {
        &self.exception_vectors
    }

    pub const fn aligned_pc_admission(&self) -> bool {
        self.aligned_pc_admission
    }

    pub const fn exact_active_owner_resolution(&self) -> bool {
        self.exact_active_owner_resolution
    }

    pub const fn explicit_thread_return_boundary(&self) -> bool {
        self.explicit_thread_return_boundary
    }

    pub const fn misaligned_target_fault(&self) -> bool {
        self.misaligned_target_fault
    }

    pub const fn unmapped_or_ambiguous_target_fault(&self) -> bool {
        self.unmapped_or_ambiguous_target_fault
    }

    pub const fn traps_enter_shared_resolver(&self) -> bool {
        self.traps_enter_shared_resolver
    }

    pub const fn build_receipt(&self) -> StaticExecutionBuildReceipt {
        self.build_receipt
    }
}

/// Issue resolver-policy evidence from the implementation that owns the
/// canonical catalog semantics. This describes the linked recompiler artifact;
/// it does not claim that any particular owner/host catalog is total.
pub const fn catalog_resolver_policy_evidence_v1() -> CatalogResolverPolicyEvidenceV1 {
    CatalogResolverPolicyEvidenceV1 {
        policy: CATALOG_RESOLVER_POLICY_NAME_V1,
        exception_vectors: CATALOG_RESOLVER_EXCEPTION_VECTORS_V1,
        aligned_pc_admission: true,
        exact_active_owner_resolution: true,
        explicit_thread_return_boundary: true,
        misaligned_target_fault: true,
        unmapped_or_ambiguous_target_fault: true,
        traps_enter_shared_resolver: true,
        build_receipt: static_execution_build_receipt(),
    }
}

/// Stable identity of one admitted code image.
///
/// The producer chooses the value from its bank/image lineage.  It must change
/// when executable bytes at an overlapping virtual address denote a different
/// image or generation; [`CodeCatalog`] rejects reusing an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BankId(u64);

impl BankId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for BankId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bank:{:016X}", self.0)
    }
}

/// A fetched executable range did not match the precompiled generation the
/// offline pack admitted. Production callers trap on this value; they never
/// translate the newly observed bytes or enter the development interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AotMiss {
    pub expected_bank: BankId,
    pub va_start: GuestPc,
    pub byte_len: u32,
    pub expected_sha256: [u8; 32],
    pub actual_sha256: [u8; 32],
}

impl fmt::Display for AotMiss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "AotMiss for {} range {}..{:#010X}: expected {:x}, observed {:x}",
            self.expected_bank,
            self.va_start,
            self.va_start.get().saturating_add(self.byte_len),
            Sha256Display(self.expected_sha256),
            Sha256Display(self.actual_sha256),
        )
    }
}

struct Sha256Display([u8; 32]);

impl fmt::LowerHex for Sha256Display {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Hash one completed live image at its fetch boundary and admit only the
/// exact precompiled generation. This performs no translation and has no
/// fallback path.
pub fn verify_precompiled_image(
    expected_bank: BankId,
    va_start: GuestPc,
    byte_len: u32,
    expected_sha256: [u8; 32],
    mem: &Rdram<'_>,
) -> Result<(), AotMiss> {
    assert!(byte_len > 0 && byte_len.is_multiple_of(4));
    va_start
        .get()
        .checked_add(byte_len)
        .expect("precompiled executable image range overflow");
    let canonical_start = 0xffff_ffff_0000_0000u64 | u64::from(va_start.get());
    let bytes = (0..byte_len)
        .map(|offset| mem.load_bu(canonical_start + u64::from(offset)))
        .collect::<Vec<_>>();
    let actual_sha256: [u8; 32] = Sha256::digest(bytes).into();
    if actual_sha256 == expected_sha256 {
        Ok(())
    } else {
        Err(AotMiss {
            expected_bank,
            va_start,
            byte_len,
            expected_sha256,
            actual_sha256,
        })
    }
}

/// Verify the exact instruction word about to execute from an immutable AOT
/// artifact. Neighboring mutable data is deliberately outside this identity:
/// a replacement is detected at the first changed instruction fetch, before
/// any stale instruction effect occurs.
pub fn verify_precompiled_instruction_word(
    expected_bank: BankId,
    pc: GuestPc,
    expected_word: u32,
    mem: &Rdram<'_>,
) -> Result<(), AotMiss> {
    assert!(pc.is_instruction_aligned());
    let address = 0xffff_ffff_0000_0000u64 | u64::from(pc.get());
    let actual_word = mem.load_w(address) as u32;
    if actual_word == expected_word {
        return Ok(());
    }
    Err(AotMiss {
        expected_bank,
        va_start: pc,
        byte_len: 4,
        expected_sha256: Sha256::digest(expected_word.to_be_bytes()).into(),
        actual_sha256: Sha256::digest(actual_word.to_be_bytes()).into(),
    })
}

/// A guest virtual program counter.
///
/// Alignment is checked at the execution boundary rather than hidden in this
/// constructor so malformed machine state becomes a typed [`CpuFault`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GuestPc(u32);

impl GuestPc {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn is_instruction_aligned(self) -> bool {
        self.0 & 3 == 0
    }
}

impl fmt::Display for GuestPc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0)
    }
}

/// Complete identity of one CPU execution destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionKey {
    pub bank: BankId,
    pub pc: GuestPc,
}

/// Identity of one admitted physical instruction word.
///
/// `BankId` is the immutable image/generation evidence; `physical_address`
/// selects the word inside that generation. This is intentionally not an
/// [`ExecutionKey`]: branch arithmetic, link registers, EPC, and Cause.BD use
/// the architectural virtual PC even when two VAs name this same identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstructionWordIdentity {
    pub bank: BankId,
    pub physical_address: u32,
}

impl InstructionWordIdentity {
    pub const fn new(bank: BankId, physical_address: u32) -> Self {
        Self {
            bank,
            physical_address,
        }
    }
}

impl ExecutionKey {
    pub const fn new(bank: BankId, pc: GuestPc) -> Self {
        Self { bank, pc }
    }
}

impl fmt::Display for ExecutionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, pc={})", self.bank, self.pc)
    }
}

/// Why CPU execution could not begin or continue at an [`ExecutionKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuFaultKind {
    /// Compatibility boundary used by the interpreter fallback for a computed
    /// instruction address that is not word aligned. Generated AOT runners use
    /// the architecturally precise [`Self::Exception`] form instead.
    UnalignedPc,
    UnknownBank,
    UnmappedPc {
        bank_start: u32,
        bank_end: u32,
    },
    /// A bankless virtual-address lookup matched more than one admitted code
    /// image. The first two candidates are ordered by [`BankId`]; the count
    /// preserves the complete ambiguity denominator without allocating in a
    /// CPU fault.
    AmbiguousPc {
        first_candidate: BankId,
        second_candidate: BankId,
        candidate_count: u32,
    },
    /// The target belongs to the closed precompiled generation inventory, but
    /// no digest-selected generation currently owns it. The outer canonical
    /// owner must activate from explicit physical backing before retrying.
    NoActiveGeneration,
    /// VA translation succeeded, but that physical word was not admitted in
    /// the selected immutable generation.
    UnmappedPhysicalInstruction {
        physical_address: u32,
    },
    /// A translated AOT unit was entered after its VA-to-physical binding
    /// changed. Retrying stale native code would execute the wrong word, so
    /// this remains a loud generation boundary for the mapping owner to
    /// rebuild and re-resolve.
    StaleInstructionIdentity {
        expected: InstructionWordIdentity,
        actual: InstructionWordIdentity,
    },
    /// A physical generation was admitted without a precompiled callable for
    /// the attempted entry. Production builds cannot interpret or translate
    /// this destination at runtime.
    MissingAotEntry,
    /// A guest data access whose effective address is outside the RDRAM bytes
    /// owned by the executing host. This remains distinct from architectural
    /// AdEL/AdES: the latter describes alignment, while this value names the
    /// host admission boundary shared by AOT and `dynamic_mips` lanes.
    MemoryFault {
        addr: u64,
    },
    /// A decoded instruction whose architecture is not yet modeled by the
    /// interpreter fallback. The raw word makes the unsupported frontier loud
    /// and deterministic instead of silently treating it as a nop.
    UnsupportedInstruction {
        word: u32,
    },
    Exception {
        exception: CpuException,
        epc: GuestPc,
        branch_delay: bool,
        instruction_code: u32,
        bad_vaddr: Option<u64>,
        coprocessor: Option<u8>,
    },
}

/// Architecturally defined synchronous exceptions currently produced by the
/// arbitrary-PC lane. Coprocessor and TLB exceptions join this enum as their
/// instruction paths stop using host panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuException {
    TlbModified,
    TlbRefillLoad,
    TlbRefillStore,
    XTlbRefillLoad,
    XTlbRefillStore,
    TlbInvalidLoad,
    TlbInvalidStore,
    AddressErrorLoad,
    AddressErrorStore,
    CoprocessorUnusable,
    Syscall,
    Breakpoint,
    ReservedInstruction,
    Trap,
    IntegerOverflow,
    /// An enabled COP1 (FPU) IEEE exception. The VR4300 raises ExcCode 15 (FPE)
    /// through the general exception vector when an arithmetic/conversion op sets
    /// an FCSR Cause bit whose matching Enable bit is set. Unlike
    /// [`Self::CoprocessorUnusable`] (ExcCode 11) it does NOT set Cause.CE — FPE
    /// is a normal general exception, and the handler reads FCSR.Cause to learn
    /// which IEEE condition trapped.
    FloatingPoint,
}

/// One of the VR4300 Cause.IP / Status.IM interrupt lines.
///
/// The N64's MIPS Interface drives [`Self::RCP`] (IP2). Keeping the CPU line
/// typed prevents device-specific MI bits from being confused with the CPU's
/// independently numbered pending bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CpuInterruptLine(u8);

impl CpuInterruptLine {
    pub const SOFTWARE_0: Self = Self(0);
    pub const SOFTWARE_1: Self = Self(1);
    pub const RCP: Self = Self(2);
    pub const CARTRIDGE: Self = Self(3);
    pub const PRE_NMI: Self = Self(4);
    pub const RDB_READ: Self = Self(5);
    pub const RDB_WRITE: Self = Self(6);
    pub const TIMER: Self = Self(7);

    pub const fn cause_bit(self) -> u32 {
        1 << (8 + self.0)
    }

    /// Drive this level-sensitive hardware line into Cause.IP.
    pub fn set_level(self, ctx: &mut RecompContext, asserted: bool) {
        if asserted {
            ctx.cop0_cause |= self.cause_bit();
        } else {
            ctx.cop0_cause &= !self.cause_bit();
        }
    }
}

/// Enter an enabled pending interrupt between translated instructions.
///
/// VR4300 User's Manual sections 6.2-6.3 define the gate as Status.IE set,
/// EXL/ERL clear, and a nonempty `Status.IM & Cause.IP`. Interrupts use
/// ExcCode 0 and the BEV-selected general exception vector. The arbitrary-PC
/// dispatcher calls this only at an instruction boundary, so BD is clear and
/// EPC is the instruction that would otherwise execute next.
pub fn enter_pending_interrupt(
    ctx: &mut RecompContext,
    interrupted_pc: GuestPc,
) -> Option<GuestPc> {
    const STATUS_IE: u32 = 1;
    const STATUS_EXL: u32 = 1 << 1;
    const STATUS_ERL: u32 = 1 << 2;
    const STATUS_IM_MASK: u32 = 0xFF << 8;
    const STATUS_BEV: u32 = 1 << 22;
    const CAUSE_IP_MASK: u32 = 0xFF << 8;
    const CAUSE_EXCCODE_MASK: u32 = 0x1F << 2;
    const CAUSE_BD: u32 = 1 << 31;

    let enabled = ctx.cop0_status & STATUS_IE != 0;
    let outside_exception = ctx.cop0_status & (STATUS_EXL | STATUS_ERL) == 0;
    let unmasked = (ctx.cop0_status & STATUS_IM_MASK) & (ctx.cop0_cause & CAUSE_IP_MASK) != 0;
    if !enabled || !outside_exception || !unmasked {
        return None;
    }

    ctx.cop0_epc = interrupted_pc.get();
    ctx.cop0_cause &= !(CAUSE_BD | CAUSE_EXCCODE_MASK);
    ctx.cop0_status |= STATUS_EXL;
    Some(GuestPc::new(if ctx.cop0_status & STATUS_BEV != 0 {
        CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[5]
    } else {
        CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[2]
    }))
}

/// A guest CPU fault with the exact bank-qualified destination that caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuFault {
    pub at: ExecutionKey,
    pub kind: CpuFaultKind,
}

impl fmt::Display for CpuFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CpuFaultKind::UnalignedPc => write!(f, "unaligned execution PC at {}", self.at),
            CpuFaultKind::UnknownBank => write!(f, "unknown executable bank at {}", self.at),
            CpuFaultKind::UnmappedPc {
                bank_start,
                bank_end,
            } => write!(
                f,
                "unmapped execution PC at {}; bank interval is {bank_start:#010X}..{bank_end:#010X}",
                self.at
            ),
            CpuFaultKind::AmbiguousPc {
                first_candidate,
                second_candidate,
                candidate_count,
            } => write!(
                f,
                "ambiguous execution PC at {}; {candidate_count} admitted banks match, beginning with {first_candidate} and {second_candidate}",
                self.at
            ),
            CpuFaultKind::NoActiveGeneration => write!(
                f,
                "precompiled execution PC at {} requires digest activation",
                self.at
            ),
            CpuFaultKind::UnmappedPhysicalInstruction { physical_address } => write!(
                f,
                "physical instruction word {physical_address:#010X} is not admitted at {}",
                self.at
            ),
            CpuFaultKind::StaleInstructionIdentity { expected, actual } => write!(
                f,
                "stale translated instruction at {}: expected {}:{:#010X}, fetched {}:{:#010X}",
                self.at,
                expected.bank,
                expected.physical_address,
                actual.bank,
                actual.physical_address
            ),
            CpuFaultKind::MissingAotEntry => {
                write!(f, "AotMiss: no precompiled entry exists at {}", self.at)
            }
            CpuFaultKind::MemoryFault { addr } => write!(
                f,
                "guest memory access outside backed RDRAM at {}; effective address {addr:#018X}",
                self.at
            ),
            CpuFaultKind::UnsupportedInstruction { word } => write!(
                f,
                "unsupported instruction {word:#010X} at {}; encoding decodes but its architecture is not modeled by the executing lane",
                self.at
            ),
            CpuFaultKind::Exception {
                exception,
                epc,
                branch_delay,
                instruction_code,
                bad_vaddr,
                coprocessor,
            } => write!(
                f,
                "CPU {exception:?} exception at {}; EPC={epc}, BD={branch_delay}, instruction code={instruction_code:#X}, BadVAddr={bad_vaddr:?}, coprocessor={coprocessor:?}",
                self.at
            ),
        }
    }
}

impl std::error::Error for CpuFault {}

impl CpuException {
    /// VR4300 Cause.ExcCode value (User's Manual, exception-code table).
    pub const fn cause_code(self) -> u32 {
        match self {
            Self::TlbModified => 1,
            Self::TlbRefillLoad | Self::XTlbRefillLoad | Self::TlbInvalidLoad => 2,
            Self::TlbRefillStore | Self::XTlbRefillStore | Self::TlbInvalidStore => 3,
            Self::AddressErrorLoad => 4,
            Self::AddressErrorStore => 5,
            Self::Syscall => 8,
            Self::Breakpoint => 9,
            Self::ReservedInstruction => 10,
            Self::CoprocessorUnusable => 11,
            Self::IntegerOverflow => 12,
            Self::Trap => 13,
            Self::FloatingPoint => 15,
        }
    }

    const fn is_tlb_exception(self) -> bool {
        matches!(
            self,
            Self::TlbModified
                | Self::TlbRefillLoad
                | Self::TlbRefillStore
                | Self::XTlbRefillLoad
                | Self::XTlbRefillStore
                | Self::TlbInvalidLoad
                | Self::TlbInvalidStore
        )
    }

    const fn is_tlb_refill(self) -> bool {
        matches!(
            self,
            Self::TlbRefillLoad
                | Self::TlbRefillStore
                | Self::XTlbRefillLoad
                | Self::XTlbRefillStore
        )
    }

    const fn is_xtlb_refill(self) -> bool {
        matches!(self, Self::XTlbRefillLoad | Self::XTlbRefillStore)
    }
}

impl CpuFault {
    /// Construct the AdEL raised when instruction fetch sees a PC that is not
    /// word-aligned. The fetch is not a branch delay instruction: EPC and
    /// BadVAddr both name the requested target and Cause.BD is clear.
    pub const fn instruction_address_error(at: ExecutionKey) -> Self {
        Self {
            at,
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: at.pc,
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(at.pc.get() as u64),
                coprocessor: None,
            },
        }
    }

    /// Apply a synchronous exception to CP0 and return its general exception
    /// vector. VR4300 User's Manual section 6.3 defines EXL, EPC, Cause.BD,
    /// Cause.ExcCode, BadVAddr for address exceptions, and the BEV-selected
    /// general vectors.
    ///
    /// Returns `None` for mapping/dispatcher faults, which are host execution
    /// defects rather than guest architectural exceptions.
    pub fn enter_exception(self, ctx: &mut RecompContext) -> Option<GuestPc> {
        let CpuFaultKind::Exception {
            exception,
            epc,
            branch_delay,
            bad_vaddr,
            coprocessor,
            ..
        } = self.kind
        else {
            return None;
        };
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_BEV: u32 = 1 << 22;
        const CAUSE_BD: u32 = 1 << 31;
        const CAUSE_CE_MASK: u32 = 0b11 << 28;
        const CAUSE_EXCCODE_MASK: u32 = 0x1F << 2;

        let was_exl = ctx.cop0_status & STATUS_EXL != 0;
        if !was_exl {
            ctx.cop0_epc = epc.get();
            if branch_delay {
                ctx.cop0_cause |= CAUSE_BD;
            } else {
                ctx.cop0_cause &= !CAUSE_BD;
            }
        }
        if let Some(bad_vaddr) = bad_vaddr {
            ctx.cop0_badvaddr = bad_vaddr;
            if exception.is_tlb_exception() {
                // VR4300 User's Manual TLB exception processing: Context gets
                // VA[31:13] as BadVPN2, XContext gets Region plus VA[39:13],
                // and EntryHi gets Region/VPN2. Both context registers retain
                // their software-owned PTEBase and EntryHi retains ASID.
                let low = bad_vaddr as u32;
                ctx.cop0_context = (ctx.cop0_context & 0xff80_0000) | ((low >> 9) & 0x007f_fff0);
                ctx.cop0_xcontext = (ctx.cop0_xcontext & 0xffff_fffe_0000_0000)
                    | ((bad_vaddr >> 31) & 0x0000_0001_8000_0000)
                    | ((bad_vaddr >> 9) & 0x0000_0000_7fff_fff0);
                ctx.cop0_entry_hi =
                    (bad_vaddr & 0xc000_00ff_ffff_e000) | (ctx.cop0_entry_hi & 0xff);
            }
        }
        if let Some(coprocessor) = coprocessor {
            assert!(
                coprocessor < 4,
                "Cause.CE coprocessor index exceeds two bits"
            );
            ctx.cop0_cause = (ctx.cop0_cause & !CAUSE_CE_MASK) | (u32::from(coprocessor) << 28);
        }
        ctx.cop0_cause = (ctx.cop0_cause & !CAUSE_EXCCODE_MASK) | (exception.cause_code() << 2);
        ctx.cop0_status |= STATUS_EXL;

        let refill_vector = exception.is_tlb_refill() && !was_exl;
        let extended_refill_vector = exception.is_xtlb_refill() && !was_exl;
        Some(GuestPc::new(if ctx.cop0_status & STATUS_BEV != 0 {
            if extended_refill_vector {
                CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[4]
            } else if refill_vector {
                CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[3]
            } else {
                CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[5]
            }
        } else if extended_refill_vector {
            CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[1]
        } else if refill_vector {
            CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[0]
        } else {
            CATALOG_RESOLVER_EXCEPTION_VECTORS_V1[2]
        }))
    }
}

/// Typed boundary between one translated block and its dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockExit {
    /// Destination was proven when the block was translated.
    Transfer(ExecutionKey),
    /// Machine code supplied only a virtual target (for example `jr $t9`).
    /// The active mapping layer must resolve it to exactly one bank-qualified
    /// key before another block may execute.
    ResolveTransfer {
        source_bank: BankId,
        target_pc: GuestPc,
    },
    /// A call target is computed or not statically classified. Unlike a jump,
    /// the resolver may identify this as a host ABI function; `resume` is the
    /// already-executed link address and remains bank-qualified.
    ResolveCall {
        source_bank: BankId,
        target_pc: GuestPc,
        resume: ExecutionKey,
    },
    HostCall {
        vram: GuestPc,
        resume: ExecutionKey,
    },
    /// A committed store changed the active executable image. The owner must
    /// publish the replacement generation before resolving `resume` again.
    ExecutableWrite {
        source_bank: BankId,
        resume: ExecutionKey,
    },
    /// An executable-changing store occurred in the delay slot of a call whose
    /// target still needs guest-versus-host classification. A dispatcher must
    /// resolve that classification without entering either target first.
    ExecutableWriteResolveCall {
        source_bank: BankId,
        target_pc: GuestPc,
        resume: ExecutionKey,
    },
    /// A delay-slot store changed executable bytes before the selected target
    /// raised an architectural fetch fault. Exception state must be applied,
    /// but its handler may not execute until the replacement generation is
    /// visible.
    ExecutableWriteFault(CpuFault),
    /// Live bytes at an attempted fetch match neither the runner's immutable
    /// generation nor any state the runner may execute. The outer mapping
    /// owner must select a precompiled generation by `miss.actual_sha256`,
    /// atomically activate it, and retry `at`.
    ImageChanged {
        at: ExecutionKey,
        miss: AotMiss,
    },
    Checkpoint(ExecutionKey),
    Yield(ExecutionKey),
    /// The guest thread entry returned through its configured sentinel. This
    /// is distinct from an unmapped-PC fault: live runtimes may only finish a
    /// coroutine when generated code or an explicit return adapter emits this
    /// boundary.
    ThreadReturn,
    Fault(CpuFault),
}

/// Drain a request left by a store in a control transfer's delay slot and
/// preserve the selected continuation without entering it.
///
/// Straight-line runners consume the request at `PC + 4` so they can stop
/// before their own loop advances. Control transfers already return a typed
/// exit after the indivisible branch/delay pair; this conversion makes direct
/// runner invocation just as leak-free as dispatcher-driven invocation.
pub fn finalize_executable_write_exit(source_bank: BankId, exit: BlockExit) -> BlockExit {
    if !crate::runtime::take_executable_write_boundary() {
        return exit;
    }
    match exit {
        BlockExit::Transfer(next) => BlockExit::ExecutableWrite {
            source_bank,
            resume: next,
        },
        BlockExit::ResolveTransfer {
            source_bank,
            target_pc,
        } => BlockExit::ExecutableWrite {
            source_bank,
            resume: ExecutionKey::new(source_bank, target_pc),
        },
        BlockExit::ResolveCall {
            source_bank,
            target_pc,
            resume,
        } => BlockExit::ExecutableWriteResolveCall {
            source_bank,
            target_pc,
            resume,
        },
        BlockExit::Fault(fault) => BlockExit::ExecutableWriteFault(fault),
        // Each of these already returns to the host owner before another guest
        // instruction can execute. Draining the request prevents it from
        // contaminating a later direct runner invocation; the host processes
        // committed executable writes at every such outer boundary.
        outer => outer,
    }
}

/// Select the host boundary, if any, after one ordinary instruction retires.
///
/// Generated arbitrary-PC runners call this shared post-step instead of
/// rebuilding the same executable-write and budget exits in every dispatch
/// arm. An executable write wins over a coincident budget checkpoint: the
/// active mapping owner must publish the replacement generation before the
/// continuation can be selected again. A runner that has reached its local
/// artifact edge still drains an executable-write request, but leaves its
/// already-proven cross-artifact transfer to the generated arm.
#[inline(never)]
pub fn post_straight_instruction_exit(
    source_bank: BankId,
    next_pc: GuestPc,
    executed: u32,
    budget: InstructionBudget,
    may_continue_locally: bool,
) -> Option<BlockExit> {
    let resume = ExecutionKey::new(source_bank, next_pc);
    if crate::runtime::take_executable_write_boundary() {
        return Some(BlockExit::ExecutableWrite {
            source_bank,
            resume,
        });
    }
    if may_continue_locally && executed >= budget.get() {
        return Some(BlockExit::Checkpoint(resume));
    }
    None
}

/// Maximum number of ordinary instructions a runner may execute before it
/// returns a deterministic checkpoint.
///
/// A single straight instruction is a valid turn. A control transfer and its
/// delay slot still require [`Self::CONTROL_TRANSFER_INSTRUCTIONS`] together;
/// runners checkpoint before that indivisible unit when it does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstructionBudget(u32);

impl InstructionBudget {
    pub const MIN: u32 = 1;
    pub const CONTROL_TRANSFER_INSTRUCTIONS: u32 = 2;

    pub const fn new(value: u32) -> Option<Self> {
        if value >= Self::MIN {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    /// Whether an indivisible unit can retire after `executed` instructions
    /// without exceeding this turn's total budget.
    pub const fn can_fit(self, executed: u32, unit_instructions: u32) -> bool {
        unit_instructions <= self.0.saturating_sub(executed)
    }
}

/// Result of one block-runner turn, including deterministic guest work for
/// the clock/device layer to charge before following the exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRun {
    pub exit: BlockExit,
    pub instructions: u32,
}

impl BlockRun {
    pub const fn new(exit: BlockExit, instructions: u32) -> Self {
        Self { exit, instructions }
    }
}

/// One installed bank/basic-block execution lane.
///
/// The trait keeps the dispatcher independent of how a block was produced:
/// generated Rust, a future dynamic translator, and an instrumented
/// interpreter can all satisfy the same contract.
pub trait BlockRunner {
    fn run(&mut self, entry: ExecutionKey, budget: InstructionBudget) -> BlockRun;
}

/// Callable shape emitted for one immutable sparse bank.
pub type GeneratedBankFn = for<'ctx, 'view, 'rdram> fn(
    ExecutionKey,
    InstructionBudget,
    &'ctx mut RecompContext,
    &'view mut Rdram<'rdram>,
) -> BlockRun;

/// A generated callable bound to the bank identity embedded in its body.
#[derive(Clone, Copy)]
pub struct GeneratedBankRunner {
    bank: BankId,
    run: GeneratedBankFn,
    artifact_identity: Option<ProgramArtifactIdentity>,
}

impl GeneratedBankRunner {
    /// Construct an executable runner without release-evidence identity.
    ///
    /// This compatibility path runs normally, but a containing
    /// [`BlockProgram`] cannot produce release evidence until every runner was
    /// installed through [`Self::new_with_artifact_identity`].
    pub const fn new(bank: BankId, run: GeneratedBankFn) -> Self {
        Self {
            bank,
            run,
            artifact_identity: None,
        }
    }

    /// Bind a generated callable to the stable build artifact which supplies
    /// its implementation. The identity is not derived from the function
    /// pointer and must describe the actual generated runner artifact.
    pub const fn new_with_artifact_identity(
        bank: BankId,
        run: GeneratedBankFn,
        artifact_identity: ProgramArtifactIdentity,
    ) -> Self {
        Self {
            bank,
            run,
            artifact_identity: Some(artifact_identity),
        }
    }

    pub const fn bank(self) -> BankId {
        self.bank
    }

    pub const fn artifact_identity(self) -> Option<ProgramArtifactIdentity> {
        self.artifact_identity
    }

    pub const fn callable(self) -> GeneratedBankFn {
        self.run
    }
}

impl<F> BlockRunner for F
where
    F: FnMut(ExecutionKey, InstructionBudget) -> BlockRun,
{
    fn run(&mut self, entry: ExecutionKey, budget: InstructionBudget) -> BlockRun {
        self(entry, budget)
    }
}

/// Resolves a machine-computed virtual target against the currently active
/// executable mapping. A virtual PC alone is never enough to choose between
/// overlapping banks.
pub trait TransferResolver {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault>;

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        self.resolve(source_bank, target_pc)
            .map(CallResolution::Guest)
    }
}

/// Typed result of resolving a call destination. Host functions are not fake
/// executable banks and guest banks are not host function pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallResolution {
    Guest(ExecutionKey),
    Host,
}

impl<F> TransferResolver for F
where
    F: FnMut(BankId, GuestPc) -> Result<ExecutionKey, CpuFault>,
{
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self(source_bank, target_pc)
    }
}

/// Work completed by [`dispatch_until_boundary`] before a device/scheduler
/// boundary, host call, yield, or fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchRun {
    pub exit: BlockExit,
    pub instructions: u32,
    pub blocks: u32,
}

/// A typed reason dispatch cannot complete the requested turn.
///
/// An indivisible-unit error reports a caller budget that cannot admit the
/// next architectural unit. The remaining variants are generated/dynamic
/// runner contract defects. None are guest CPU exceptions, so they remain
/// distinct from [`CpuFault`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The instruction at `at` begins an indivisible unit that cannot fit in
    /// the complete remaining dispatch budget. No instruction in the unit
    /// retired.
    IndivisibleUnitExceedsBudget {
        at: ExecutionKey,
        budget: InstructionBudget,
        required: u32,
    },
    ContinuingExitWithoutProgress {
        at: ExecutionKey,
        exit: BlockExit,
    },
    RunnerExceededBudget {
        at: ExecutionKey,
        budget: InstructionBudget,
        actual: u32,
    },
    InstructionCountOverflow,
    BlockCountOverflow,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::IndivisibleUnitExceedsBudget {
                at,
                budget,
                required,
            } => write!(
                f,
                "indivisible instruction unit at {at} requires {required} instructions but only {} remain",
                budget.get()
            ),
            Self::ContinuingExitWithoutProgress { at, exit } => {
                write!(f, "block runner made no progress at {at}: {exit:?}")
            }
            Self::RunnerExceededBudget { at, budget, actual } => write!(
                f,
                "block runner at {at} executed {actual} instructions with budget {}",
                budget.get()
            ),
            Self::InstructionCountOverflow => write!(f, "dispatch instruction count overflow"),
            Self::BlockCountOverflow => write!(f, "dispatch block count overflow"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// Follow translated block exits until guest execution must return to the
/// device/scheduler layer.
///
/// A total budget is enforced across direct and computed transfers. A final
/// one-instruction slice may retire a straight instruction. If that slice
/// reaches an indivisible branch/delay pair, the dispatcher returns
/// [`DispatchError::IndivisibleUnitExceedsBudget`] with no work from the pair
/// committed. Resolver failures become ordinary typed CPU-fault exits with all
/// work already completed preserved in the result.
pub fn dispatch_until_boundary<R, V>(
    mut entry: ExecutionKey,
    budget: InstructionBudget,
    runner: &mut R,
    resolver: &mut V,
) -> Result<DispatchRun, DispatchError>
where
    R: BlockRunner,
    V: TransferResolver,
{
    let mut instructions = 0u32;
    let mut blocks = 0u32;

    loop {
        let remaining = budget.get() - instructions;
        if remaining < InstructionBudget::MIN {
            return Ok(DispatchRun {
                exit: BlockExit::Checkpoint(entry),
                instructions,
                blocks,
            });
        }
        let turn_budget = InstructionBudget::new(remaining)
            .expect("remaining budget was checked against InstructionBudget::MIN");
        let run = runner.run(entry, turn_budget);
        let run = BlockRun::new(
            finalize_executable_write_exit(entry.bank, run.exit),
            run.instructions,
        );
        if run.instructions > remaining {
            return Err(DispatchError::RunnerExceededBudget {
                at: entry,
                budget: turn_budget,
                actual: run.instructions,
            });
        }
        if run.instructions == 0
            && run.exit == BlockExit::Checkpoint(entry)
            && !turn_budget.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
        {
            return Err(DispatchError::IndivisibleUnitExceedsBudget {
                at: entry,
                budget: turn_budget,
                required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
            });
        }
        if run.instructions == 0
            && matches!(
                run.exit,
                BlockExit::Checkpoint(_)
                    | BlockExit::Transfer(_)
                    | BlockExit::ResolveTransfer { .. }
                    | BlockExit::ResolveCall { .. }
                    | BlockExit::ExecutableWrite { .. }
                    | BlockExit::ExecutableWriteResolveCall { .. }
                    | BlockExit::ExecutableWriteFault(_)
            )
        {
            return Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: run.exit,
            });
        }
        instructions = instructions
            .checked_add(run.instructions)
            .ok_or(DispatchError::InstructionCountOverflow)?;
        blocks = blocks
            .checked_add(1)
            .ok_or(DispatchError::BlockCountOverflow)?;

        match run.exit {
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWrite {
                        source_bank,
                        resume,
                    },
                    instructions,
                    blocks,
                });
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWriteResolveCall {
                        source_bank,
                        target_pc,
                        resume,
                    },
                    instructions,
                    blocks,
                });
            }
            BlockExit::ExecutableWriteFault(fault) => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWriteFault(fault),
                    instructions,
                    blocks,
                });
            }
            BlockExit::Transfer(next) => entry = next,
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc,
            } => match resolver.resolve(source_bank, target_pc) {
                Ok(next) => entry = next,
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            },
            BlockExit::ResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolver.resolve_call(source_bank, target_pc, resume) {
                Ok(CallResolution::Guest(next)) => entry = next,
                Ok(CallResolution::Host) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::HostCall {
                            vram: target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            },
            exit => {
                return Ok(DispatchRun {
                    exit,
                    instructions,
                    blocks,
                });
            }
        }
    }
}

/// One owned, contiguous executable span within a bank.
///
/// Construction binds the span to its bank identity and proves nonempty,
/// aligned, non-overflowing geometry. Cross-span ordering and overlap are
/// validated by [`CodeBank::from_spans`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    bank: BankId,
    vram_start: GuestPc,
    words: Vec<u32>,
}

impl CodeSpan {
    pub fn new(bank: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        if !vram_start.is_instruction_aligned() {
            return Err(BankError::UnalignedStart {
                bank,
                start: vram_start,
            });
        }
        if words.is_empty() {
            return Err(BankError::Empty { bank });
        }
        let byte_len = u32::try_from(words.len())
            .ok()
            .and_then(|len| len.checked_mul(4))
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        vram_start
            .get()
            .checked_add(byte_len)
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        Ok(Self {
            bank,
            vram_start,
            words,
        })
    }

    pub const fn bank(&self) -> BankId {
        self.bank
    }

    pub const fn vram_start(&self) -> GuestPc {
        self.vram_start
    }

    pub fn vram_end(&self) -> GuestPc {
        GuestPc::new(self.vram_start.get() + self.words.len() as u32 * 4)
    }

    pub fn instruction_count(&self) -> usize {
        self.words.len()
    }

    /// Exact big-endian instruction words owned by this immutable span.
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let offset = pc.get().checked_sub(self.vram_start.get())?;
        self.words.get((offset / 4) as usize).copied()
    }
}

/// One immutable sparse executable image admitted to the block translator.
///
/// A bank owns sorted, disjoint [`CodeSpan`] values. Its lowest/highest
/// addresses are diagnostic bounds only; addresses in holes never resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBank {
    id: BankId,
    spans: Vec<CodeSpan>,
}

/// Stable 256-bit identity of the executable artifact installed by a host.
///
/// Function-lane native callables are opaque to safe Rust, so their producer
/// supplies the SHA-256 (or an equally stable 256-bit build identity) of the
/// actual generated artifact. Block programs derive their aggregate identity
/// from the canonical bank image plus each runner's supplied artifact
/// identity. Native addresses are never accepted as artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramArtifactIdentity([u8; 32]);

/// Callable shape installed around one generated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratedAdapterRole {
    DirectGenerated,
    EntryContextGate,
    DenseInstrumentationGate,
    OverlayGenerationGate,
    ExternalDigestGate,
}

impl GeneratedAdapterRole {
    const fn tag(self) -> u8 {
        match self {
            Self::DirectGenerated => 0,
            Self::EntryContextGate => 1,
            Self::DenseInstrumentationGate => 2,
            Self::OverlayGenerationGate => 3,
            Self::ExternalDigestGate => 4,
        }
    }
}

impl ProgramArtifactIdentity {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    /// Identity of an installed callable which combines a handwritten
    /// adapter with one exact generated bank runner.
    pub fn generated_adapter(
        adapter_source_identity: [u8; 32],
        generated_runner_source_identity: [u8; 32],
        bank: BankId,
        role: GeneratedAdapterRole,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"fn64:generated-runner-adapter:v1:");
        hasher.update(adapter_source_identity);
        hasher.update(generated_runner_source_identity);
        hasher.update(bank.get().to_be_bytes());
        hasher.update([role.tag()]);
        Self(hasher.finalize().into())
    }
}

pub const GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2: &str =
    "fn64.generated-runner-source-attestation.v2";
/// Canonical hash-domain prefix shared by the source-attestation issuer and
/// the independent selected-build verifier.
pub const GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2: &[u8] =
    b"fn64:cargo-generated-runner-source-attestation:v2:";
pub const GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1: &str =
    "fn64.generated-runner-runtime-source.v1";
pub const GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2: &str =
    "fn64.generated-runner-runtime-source.v2";

/// Exact source receipt for the implementation linked by typed arbitrary-PC
/// runners.
///
/// These files own typed RDRAM/MMIO routing, host-boundary exits, and
/// block-program admission. `fn64-recomp-rs-codegen` issues the separate
/// emitter-source receipt. Neither receipt says anything about a separately
/// compiled callable; only the external build owner proves that relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRuntimeSourceReceiptV1 {
    schema: &'static str,
    source_sha256: [u8; 32],
    typed_rdram: bool,
    typed_mmio: bool,
    typed_host_boundaries: bool,
}

impl GeneratedRunnerRuntimeSourceReceiptV1 {
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn typed_rdram(self) -> bool {
        self.typed_rdram
    }

    pub const fn typed_mmio(self) -> bool {
        self.typed_mmio
    }

    pub const fn typed_host_boundaries(self) -> bool {
        self.typed_host_boundaries
    }
}

pub fn generated_runner_runtime_source_receipt_v1() -> GeneratedRunnerRuntimeSourceReceiptV1 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        (b"src/execution.rs", include_bytes!("execution.rs")),
        (
            b"src/generated_support.rs",
            include_bytes!("generated_support.rs"),
        ),
        (b"src/runtime.rs", include_bytes!("runtime.rs")),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:generated-runner-runtime-source:v1:");
    for (label, source) in sources {
        hasher.update(
            u64::try_from(label.len())
                .expect("generated-runner source label length fits u64")
                .to_be_bytes(),
        );
        hasher.update(label);
        hasher.update(
            u64::try_from(source.len())
                .expect("generated-runner source length fits u64")
                .to_be_bytes(),
        );
        hasher.update(source);
    }
    GeneratedRunnerRuntimeSourceReceiptV1 {
        schema: GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1,
        source_sha256: hasher.finalize().into(),
        typed_rdram: true,
        typed_mmio: true,
        typed_host_boundaries: true,
    }
}

/// Source-complete runtime identity for typed arbitrary-PC runners.
///
/// V1 remains immutable for existing source-attestation V2 producers and
/// consumers. V2 adds `fpu.rs`, whose floating-point implementation is called
/// through `runtime.rs` and therefore changes generated-runner semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratedRunnerRuntimeSourceReceiptV2 {
    schema: &'static str,
    source_sha256: [u8; 32],
    typed_rdram: bool,
    typed_mmio: bool,
    typed_host_boundaries: bool,
}

impl GeneratedRunnerRuntimeSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn typed_rdram(self) -> bool {
        self.typed_rdram
    }

    pub const fn typed_mmio(self) -> bool {
        self.typed_mmio
    }

    pub const fn typed_host_boundaries(self) -> bool {
        self.typed_host_boundaries
    }
}

pub fn generated_runner_runtime_source_receipt_v2() -> GeneratedRunnerRuntimeSourceReceiptV2 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        (b"src/execution.rs", include_bytes!("execution.rs")),
        (
            b"src/generated_support.rs",
            include_bytes!("generated_support.rs"),
        ),
        (b"src/runtime.rs", include_bytes!("runtime.rs")),
        (b"src/fpu.rs", include_bytes!("fpu.rs")),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:generated-runner-runtime-source:v2:");
    for (label, source) in sources {
        hasher.update(
            u64::try_from(label.len())
                .expect("generated-runner source label length fits u64")
                .to_be_bytes(),
        );
        hasher.update(label);
        hasher.update(
            u64::try_from(source.len())
                .expect("generated-runner source length fits u64")
                .to_be_bytes(),
        );
        hasher.update(source);
    }
    GeneratedRunnerRuntimeSourceReceiptV2 {
        schema: GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2,
        source_sha256: hasher.finalize().into(),
        typed_rdram: true,
        typed_mmio: true,
        typed_host_boundaries: true,
    }
}

/// One callable/source relation exported by a repository-controlled generated
/// Cargo package and linked into the program-owning root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CargoGeneratedRunnerSourceBindingV1 {
    pub bank: BankId,
    pub generated_runner_source_sha256: [u8; 32],
    pub code_words_sha256: [u8; 32],
    pub vram_start: GuestPc,
    pub vram_end: GuestPc,
    pub composite_subrunner_count: u32,
    pub adapter_role: GeneratedAdapterRole,
}

/// Build-script measurements supplied at the Cargo source boundary.
///
/// Safe Rust cannot derive source identity from `GeneratedBankFn`. The caller
/// of this attestation is expected to be the checked-in root which
/// owns Cargo dependencies, generated includes, and the exported callable
/// table. Generic or third-party runner registration is deliberately outside
/// this source projection.
pub struct CargoGeneratedProgramSourceAttestationV2<'a> {
    pub root_adapter_source_sha256: [u8; 32],
    pub shard_cargo_source_tree_sha256: [u8; 32],
    pub expected_emitter_source_sha256: [u8; 32],
    /// Measured by the checked-in adapter and revalidated only by the outer
    /// verifier. This lower catalog is explicitly not an issuer of emitter
    /// source authority.
    pub externally_measured_emitter_source_sha256: [u8; 32],
    pub expected_runtime_source_sha256: [u8; 32],
    pub runtime_source_receipt: GeneratedRunnerRuntimeSourceReceiptV1,
    pub runners: &'a [CargoGeneratedRunnerSourceBindingV1],
}

/// Evidence projection claiming that one complete canonical block program was
/// paired with the measured Cargo source graph above.
///
/// This is deliberately named an attestation, not authority: this crate can
/// validate all pointer-free fields but cannot prove that rustc compiled a
/// supplied function pointer from particular bytes. SI or another completion
/// validator must not consume it. A verifier which owns an isolated Cargo
/// build (or an external build attestation) is required to mint authority.
#[derive(Debug)]
pub struct GeneratedRunnerSourceAttestationV2 {
    schema: &'static str,
    cargo_source_fields_validated: bool,
    program_identity: ProgramArtifactIdentity,
    root_adapter_source_sha256: [u8; 32],
    shard_cargo_source_tree_sha256: [u8; 32],
    emitter_source_sha256: [u8; 32],
    runtime_source_sha256: [u8; 32],
    binding_sha256: [u8; 32],
    build_receipt: StaticExecutionBuildReceipt,
}

impl GeneratedRunnerSourceAttestationV2 {
    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub const fn cargo_source_fields_validated(&self) -> bool {
        self.cargo_source_fields_validated
    }

    pub const fn program_identity(&self) -> ProgramArtifactIdentity {
        self.program_identity
    }

    pub const fn root_adapter_source_sha256(&self) -> [u8; 32] {
        self.root_adapter_source_sha256
    }

    pub const fn shard_cargo_source_tree_sha256(&self) -> [u8; 32] {
        self.shard_cargo_source_tree_sha256
    }

    pub const fn emitter_source_sha256(&self) -> [u8; 32] {
        self.emitter_source_sha256
    }

    pub const fn runtime_source_sha256(&self) -> [u8; 32] {
        self.runtime_source_sha256
    }

    pub const fn binding_sha256(&self) -> [u8; 32] {
        self.binding_sha256
    }

    pub const fn build_receipt(&self) -> StaticExecutionBuildReceipt {
        self.build_receipt
    }
}

/// Authority behind a program artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramIdentitySource {
    /// The host identified an opaque generated native artifact.
    CallerSupplied,
    /// fn64 hashed the complete canonical block code plus the stable artifact
    /// identity of every generated bank runner.
    CanonicalBlockProgramSha256,
}

/// Identity plus the authority which established it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramIdentityEvidenceSnapshot {
    pub identity: ProgramArtifactIdentity,
    pub source: ProgramIdentitySource,
}

/// Pointer-independent image of one contiguous executable span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpanEvidenceSnapshot {
    pub vram_start: GuestPc,
    pub words: Vec<u32>,
}

/// Pointer-independent image of one immutable sparse code bank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBankEvidenceSnapshot {
    pub id: BankId,
    pub runner_artifact_identity: ProgramArtifactIdentity,
    pub spans: Vec<CodeSpanEvidenceSnapshot>,
}

/// Complete canonical executable image owned by a [`BlockProgram`].
///
/// Virtual and physical banks, spans, and mapped AOT entries are sorted by
/// their typed identities/addresses. Instruction word order is architectural
/// and is retained verbatim. Generated runner pointers are deliberately
/// absent, but each generated unit retains its stable artifact identity: the
/// words alone cannot prove two native callables implement the same semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProgramEvidenceSnapshot {
    pub identity: ProgramIdentityEvidenceSnapshot,
    pub banks: Vec<CodeBankEvidenceSnapshot>,
    pub physical_banks: Vec<PhysicalCodeBankEvidenceSnapshot>,
    pub mapped_aot: Vec<MappedAotEvidenceSnapshot>,
}

/// One successfully entered bank-qualified guest execution destination.
///
/// The bank identity names the immutable code-image generation, while the
/// optional runner identity names the generated native artifact that was
/// actually entered. `None` is retained for the compatibility
/// [`GeneratedBankRunner::new`] path and the mapped-interpreter fallback;
/// neither may be promoted to release evidence without a typed artifact
/// authority.
/// Historical execution observations are intentionally separate from
/// [`BlockProgramEvidenceSnapshot`]: they describe what happened, not state
/// which can affect future execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionDestinationObservation {
    pub destination: ExecutionKey,
    pub runner_artifact_identity: Option<ProgramArtifactIdentity>,
    /// Architecturally retired instructions in this runner entry. Retaining
    /// the count lets a minimum-budget diagnostic reconstruct the exact
    /// straight-line PC sequence without instrumenting generated bodies.
    pub instructions: u32,
}

impl CodeBank {
    /// Convenience constructor for a single contiguous executable span.
    pub fn new(id: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        Self::from_spans(id, vec![CodeSpan::new(id, vram_start, words)?])
    }

    /// Admit sorted, disjoint executable spans under one immutable identity.
    pub fn from_spans(id: BankId, mut spans: Vec<CodeSpan>) -> Result<Self, BankError> {
        if spans.is_empty() {
            return Err(BankError::Empty { bank: id });
        }
        for span in &spans {
            if span.bank() != id {
                return Err(BankError::SpanBankMismatch {
                    bank: id,
                    span_bank: span.bank(),
                    start: span.vram_start(),
                });
            }
        }
        spans.sort_by_key(CodeSpan::vram_start);
        for pair in spans.windows(2) {
            let left_end = pair[0].vram_end();
            let right_start = pair[1].vram_start();
            if right_start < left_end {
                return Err(BankError::OverlappingSpans {
                    bank: id,
                    left_end,
                    right_start,
                });
            }
        }
        Ok(Self { id, spans })
    }

    pub const fn id(&self) -> BankId {
        self.id
    }

    pub fn vram_start(&self) -> GuestPc {
        self.spans[0].vram_start()
    }

    pub fn vram_end(&self) -> GuestPc {
        self.spans
            .last()
            .expect("CodeBank construction requires a span")
            .vram_end()
    }

    pub fn instruction_count(&self) -> usize {
        self.spans.iter().map(CodeSpan::instruction_count).sum()
    }

    pub fn spans(&self) -> &[CodeSpan] {
        &self.spans
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let candidate = self
            .spans
            .partition_point(|span| span.vram_start() <= pc)
            .checked_sub(1)?;
        let span = &self.spans[candidate];
        if pc < span.vram_end() {
            span.resolve(pc)
        } else {
            None
        }
    }
}

/// Failure to admit an executable image into a [`CodeCatalog`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankError {
    Empty {
        bank: BankId,
    },
    UnalignedStart {
        bank: BankId,
        start: GuestPc,
    },
    AddressOverflow {
        bank: BankId,
        start: GuestPc,
    },
    SpanBankMismatch {
        bank: BankId,
        span_bank: BankId,
        start: GuestPc,
    },
    OverlappingSpans {
        bank: BankId,
        left_end: GuestPc,
        right_start: GuestPc,
    },
    DuplicateId {
        bank: BankId,
    },
}

impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BankError::Empty { bank } => write!(f, "{bank} has no executable words"),
            BankError::UnalignedStart { bank, start } => {
                write!(f, "{bank} starts at unaligned PC {start}")
            }
            BankError::AddressOverflow { bank, start } => {
                write!(
                    f,
                    "{bank} starting at {start} exceeds the guest address space"
                )
            }
            BankError::SpanBankMismatch {
                bank,
                span_bank,
                start,
            } => write!(
                f,
                "{bank} cannot own span from {span_bank} starting at {start}"
            ),
            BankError::OverlappingSpans {
                bank,
                left_end,
                right_start,
            } => write!(
                f,
                "{bank} has overlapping executable spans at {left_end} and {right_start}"
            ),
            BankError::DuplicateId { bank } => {
                write!(f, "executable identity {bank} is already registered")
            }
        }
    }
}

impl std::error::Error for BankError {}

/// A resolved instruction word and the bank-qualified address that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedInstruction {
    pub key: ExecutionKey,
    pub word: u32,
}

/// Deterministic registry of immutable executable images.
///
/// Banks may overlap in virtual address space.  Only their identities must be
/// unique, which is exactly what prevents an overlay lookup from silently
/// selecting whichever same-VA image happened to be registered last.
#[derive(Clone, Debug, Default)]
pub struct CodeCatalog {
    banks: BTreeMap<BankId, CodeBank>,
}

impl CodeCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, bank: CodeBank) -> Result<(), BankError> {
        let id = bank.id();
        if self.banks.contains_key(&id) {
            return Err(BankError::DuplicateId { bank: id });
        }
        self.banks.insert(id, bank);
        Ok(())
    }

    pub fn bank(&self, id: BankId) -> Option<&CodeBank> {
        self.banks.get(&id)
    }

    pub fn banks(&self) -> impl Iterator<Item = &CodeBank> {
        self.banks.values()
    }

    fn unregister(&mut self, id: BankId) -> Option<CodeBank> {
        self.banks.remove(&id)
    }

    pub fn resolve(&self, key: ExecutionKey) -> Result<ResolvedInstruction, CpuFault> {
        if !key.pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(key));
        }
        let bank = self.banks.get(&key.bank).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnknownBank,
        })?;
        let start = bank.vram_start().get();
        let end = bank.vram_end().get();
        let word = bank.resolve(key.pc).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start,
                bank_end: end,
            },
        })?;
        Ok(ResolvedInstruction { key, word })
    }

    fn missing_virtual_mapping(&self, fault_bank: BankId, target_pc: GuestPc) -> CpuFault {
        let at = ExecutionKey::new(fault_bank, target_pc);
        match self.banks.get(&fault_bank) {
            Some(bank) => CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: bank.vram_start().get(),
                    bank_end: bank.vram_end().get(),
                },
            },
            None => CpuFault {
                at,
                kind: CpuFaultKind::UnknownBank,
            },
        }
    }

    fn resolve_unique_virtual(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.resolve_unique_virtual_where(fault_bank, target_pc, |_| true)
    }

    fn resolve_unique_virtual_where(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
        mut admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        let mut candidates = self
            .banks
            .values()
            .filter(|bank| admits_bank(bank.id()) && bank.resolve(target_pc).is_some())
            .map(CodeBank::id);
        let Some(first_candidate) = candidates.next() else {
            return Err(self.missing_virtual_mapping(fault_bank, target_pc));
        };
        let Some(second_candidate) = candidates.next() else {
            return Ok(ExecutionKey::new(first_candidate, target_pc));
        };
        let remaining = candidates.count();
        let candidate_count = u32::try_from(remaining)
            .ok()
            .and_then(|remaining| remaining.checked_add(2))
            .expect("virtual code-bank candidate count exceeds u32");
        Err(CpuFault {
            at: ExecutionKey::new(fault_bank, target_pc),
            kind: CpuFaultKind::AmbiguousPc {
                first_candidate,
                second_candidate,
                candidate_count,
            },
        })
    }

    /// Resolve a bankless static entry against every admitted virtual bank.
    /// `fault_bank` anchors typed failure context only; it receives no
    /// preference. Physical/mapped generations are outside this catalog.
    pub fn resolve_entry(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                fault_bank, target_pc,
            )));
        }
        self.resolve_unique_virtual(fault_bank, target_pc)
    }

    fn resolve_entry_where(
        &self,
        fault_bank: BankId,
        target_pc: GuestPc,
        admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                fault_bank, target_pc,
            )));
        }
        self.resolve_unique_virtual_where(fault_bank, target_pc, admits_bank)
    }

    /// Resolve one static guest transfer. The source bank wins when it admits
    /// the exact sparse target; otherwise resolution requires exactly one
    /// admitting virtual bank. Generated callbacks and physical generations
    /// are deliberately not consulted.
    pub fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        if self
            .banks
            .get(&source_bank)
            .is_some_and(|bank| bank.resolve(target_pc).is_some())
        {
            return Ok(ExecutionKey::new(source_bank, target_pc));
        }
        self.resolve_unique_virtual(source_bank, target_pc)
    }

    fn resolve_transfer_where(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
        mut admits_bank: impl FnMut(BankId) -> bool,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        if admits_bank(source_bank)
            && self
                .banks
                .get(&source_bank)
                .is_some_and(|bank| bank.resolve(target_pc).is_some())
        {
            return Ok(ExecutionKey::new(source_bank, target_pc));
        }
        self.resolve_unique_virtual_where(source_bank, target_pc, admits_bank)
    }

    /// Classify an admitted instruction for table-backed dispatch.  Resolution
    /// goes through the same sparse bank catalog as execution, so a data hole
    /// cannot acquire a classification merely because it lies inside a
    /// bounding interval.
    pub fn classify(&self, key: ExecutionKey) -> Result<BankWordKind, CpuFault> {
        let resolved = self.resolve(key)?;
        let instruction = crate::decode(resolved.word);
        Ok(
            if matches!(instruction, crate::decoder::Instruction::Unknown { .. }) {
                BankWordKind::Unknown
            } else if instruction.has_delay_slot() {
                BankWordKind::ControlTransfer
            } else {
                BankWordKind::Straight
            },
        )
    }
}

/// Failure to atomically pair admitted code with its generated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramError {
    RunnerBankMismatch {
        code_bank: BankId,
        runner_bank: BankId,
    },
    DuplicateBank {
        bank: BankId,
    },
    PhysicalCode(PhysicalCodeError),
    DuplicateMappedEntry {
        entry: ExecutionKey,
    },
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RunnerBankMismatch {
                code_bank,
                runner_bank,
            } => write!(
                f,
                "generated runner for {runner_bank} cannot execute code admitted as {code_bank}"
            ),
            Self::DuplicateBank { bank } => write!(f, "block program already contains {bank}"),
            Self::PhysicalCode(error) => error.fmt(f),
            Self::DuplicateMappedEntry { entry } => {
                write!(f, "block program already contains mapped AOT entry {entry}")
            }
        }
    }
}

impl std::error::Error for ProgramError {}

/// Immutable-code catalog and generated callables registered as one program.
///
/// The maps are private and registration validates both identities before
/// mutating either one. A call is admitted through [`CodeCatalog::resolve`]
/// before the generated function runs, so a broad generated match cannot
/// accidentally make a sparse-bank hole executable.
#[derive(Default)]
pub struct BlockProgram {
    code: CodeCatalog,
    runners: BTreeMap<BankId, (GeneratedBankFn, Option<ProgramArtifactIdentity>)>,
    physical_code: PhysicalCodeCatalog,
    mapped_aot: BTreeMap<ExecutionKey, MappedAotBlock>,
    execution_destinations: RefCell<VecDeque<ExecutionDestinationObservation>>,
    execution_destination_history_limit: Option<NonZeroUsize>,
    execution_destination_history_suppressed: bool,
}

impl BlockProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<(), ProgramError> {
        let code_bank = code.id();
        if runner.bank != code_bank {
            return Err(ProgramError::RunnerBankMismatch {
                code_bank,
                runner_bank: runner.bank,
            });
        }
        if self.code.bank(code_bank).is_some()
            || self.runners.contains_key(&code_bank)
            || self.physical_code.contains_bank(code_bank)
        {
            return Err(ProgramError::DuplicateBank { bank: code_bank });
        }
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.runners
            .insert(code_bank, (runner.run, runner.artifact_identity));
        Ok(())
    }

    /// Admit one immutable physical code generation for canonical 32-bit
    /// mapped fetch. Every aligned VA resolved to this `BankId` can execute
    /// immediately through the interpreter fallback; registered mapped AOT
    /// units override individual entries without changing the fetch contract.
    pub fn register_physical_code(&mut self, code: PhysicalCodeBank) -> Result<(), ProgramError> {
        let bank = code.id();
        if self.code.bank(bank).is_some() || self.runners.contains_key(&bank) {
            return Err(ProgramError::DuplicateBank { bank });
        }
        self.physical_code
            .register(code)
            .map_err(ProgramError::PhysicalCode)
    }

    /// Install one fetch-bound generated unit into the main program runner.
    /// The containing physical generation must already be registered so no
    /// optional side catalog can become a second execution authority.
    pub fn register_mapped_aot(&mut self, block: MappedAotBlock) -> Result<(), ProgramError> {
        let entry = ExecutionKey::new(block.bank(), block.entry());
        assert!(
            self.physical_code.contains_bank(block.bank()),
            "mapped AOT entry {entry} has no admitted physical code generation"
        );
        if self.mapped_aot.contains_key(&entry) {
            return Err(ProgramError::DuplicateMappedEntry { entry });
        }
        self.mapped_aot.insert(entry, block);
        Ok(())
    }

    pub fn code(&self) -> &CodeCatalog {
        &self.code
    }

    pub fn physical_code(&self) -> &PhysicalCodeCatalog {
        &self.physical_code
    }

    /// Copy retained execution history in authoritative entry order.
    ///
    /// Resolution and classification do not append here. An observation is
    /// added only after sparse code admission and runner lookup both succeed.
    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.execution_destinations
            .borrow()
            .iter()
            .copied()
            .collect()
    }

    /// Bound diagnostic execution history without changing executable state.
    /// `None` retains the complete history and remains the default required by
    /// certification evidence; a limit retains only the newest observations.
    pub fn set_execution_destination_history_limit(&mut self, limit: Option<NonZeroUsize>) {
        self.execution_destination_history_limit = limit;
        if let Some(limit) = limit {
            let destinations = self.execution_destinations.get_mut();
            while destinations.len() > limit.get() {
                destinations.pop_front();
            }
        }
    }

    /// Enable or suppress diagnostic execution history. Complete history is
    /// enabled by default; suppressing it also clears any retained entries.
    pub fn set_execution_destination_history_enabled(&mut self, enabled: bool) {
        self.execution_destination_history_suppressed = !enabled;
        if !enabled {
            self.execution_destinations.get_mut().clear();
        }
    }

    /// Start a new observation lifetime without changing executable state.
    pub fn clear_execution_destinations(&mut self) {
        self.execution_destinations.get_mut().clear();
    }

    fn observe_execution_destination(&self, observation: ExecutionDestinationObservation) {
        if self.execution_destination_history_suppressed {
            return;
        }
        let mut destinations = self.execution_destinations.borrow_mut();
        destinations.push_back(observation);
        if let Some(limit) = self.execution_destination_history_limit {
            while destinations.len() > limit.get() {
                destinations.pop_front();
            }
        }
    }

    /// Capture the complete immutable guest-code image without native
    /// callable addresses.
    ///
    /// Catalog maps sort bank/AOT identities and bank construction sorts
    /// spans, so equivalent registration order produces byte-identical
    /// evidence. The domain-separated SHA-256 covers every virtual and
    /// physical bank identity, span address, length, instruction word, mapped
    /// entry, translated instruction identity, and runner artifact identity,
    /// all encoded big-endian. Code words alone are insufficient because
    /// registration accepts independently generated native runners.
    pub fn evidence_snapshot(&self) -> BlockProgramEvidenceSnapshot {
        let banks = self
            .code
            .banks
            .values()
            .map(|bank| {
                let runner_artifact_identity = self
                    .runners
                    .get(&bank.id)
                    .and_then(|(_, identity)| *identity)
                    .unwrap_or_else(|| {
                        panic!(
                            "block-program release evidence requires a stable artifact identity for generated runner {}",
                            bank.id
                        )
                    });
                CodeBankEvidenceSnapshot {
                    id: bank.id,
                    runner_artifact_identity,
                    spans: bank
                        .spans
                        .iter()
                        .map(|span| CodeSpanEvidenceSnapshot {
                            vram_start: span.vram_start,
                            words: span.words.clone(),
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let physical_banks = self.physical_code.evidence_snapshot();
        let mapped_aot = self
            .mapped_aot
            .values()
            .map(MappedAotBlock::evidence_snapshot)
            .collect::<Vec<_>>();
        let mut hasher = Sha256::new();
        if physical_banks.is_empty() {
            hasher.update(b"fn64.block-program.identity.v1\0");
        } else {
            hasher.update(b"fn64.block-program.identity.v2\0");
        }
        hasher.update(
            u64::try_from(banks.len())
                .expect("block-program bank count exceeds identity wire")
                .to_be_bytes(),
        );
        for bank in &banks {
            hasher.update(bank.id.get().to_be_bytes());
            hasher.update(bank.runner_artifact_identity.bytes());
            hasher.update(
                u64::try_from(bank.spans.len())
                    .expect("block-program span count exceeds identity wire")
                    .to_be_bytes(),
            );
            for span in &bank.spans {
                hasher.update(span.vram_start.get().to_be_bytes());
                hasher.update(
                    u64::try_from(span.words.len())
                        .expect("block-program instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &span.words {
                    hasher.update(word.to_be_bytes());
                }
            }
        }
        if !physical_banks.is_empty() {
            hasher.update(
                u64::try_from(physical_banks.len())
                    .expect("physical block-program bank count exceeds identity wire")
                    .to_be_bytes(),
            );
            for bank in &physical_banks {
                hasher.update(bank.id.get().to_be_bytes());
                hasher.update(
                    u64::try_from(bank.spans.len())
                        .expect("physical block-program span count exceeds identity wire")
                        .to_be_bytes(),
                );
                for span in &bank.spans {
                    hasher.update(span.physical_start.to_be_bytes());
                    hasher.update(
                        u64::try_from(span.words.len())
                            .expect("physical block-program word count exceeds identity wire")
                            .to_be_bytes(),
                    );
                    for word in &span.words {
                        hasher.update(word.to_be_bytes());
                    }
                }
            }
            hasher.update(
                u64::try_from(mapped_aot.len())
                    .expect("mapped AOT unit count exceeds identity wire")
                    .to_be_bytes(),
            );
            for unit in &mapped_aot {
                hasher.update(unit.entry.bank.get().to_be_bytes());
                hasher.update(unit.entry.pc.get().to_be_bytes());
                hasher.update(unit.runner_artifact_identity.bytes());
                hasher.update(
                    u64::try_from(unit.instructions.len())
                        .expect("mapped AOT instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for instruction in &unit.instructions {
                    hasher.update(instruction.bank.get().to_be_bytes());
                    hasher.update(instruction.physical_address.to_be_bytes());
                }
                hasher.update(
                    u64::try_from(unit.expected_words.len())
                        .expect("mapped AOT expected-word count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &unit.expected_words {
                    hasher.update(word.to_be_bytes());
                }
            }
        }
        BlockProgramEvidenceSnapshot {
            identity: ProgramIdentityEvidenceSnapshot {
                identity: ProgramArtifactIdentity::new(hasher.finalize().into()),
                source: ProgramIdentitySource::CanonicalBlockProgramSha256,
            },
            banks,
            physical_banks,
            mapped_aot,
        }
    }

    /// Atomically retire one immutable code generation and its callable.
    /// Returning `false` means neither half existed; a one-sided presence is
    /// an internal invariant violation rather than a recoverable stale state.
    pub fn unregister(&mut self, bank: BankId) -> bool {
        if let Some(_physical) = self.physical_code.unregister(bank) {
            self.mapped_aot.retain(|entry, _| entry.bank != bank);
            return true;
        }
        let code = self.code.unregister(bank);
        let runner = self.runners.remove(&bank);
        assert_eq!(
            code.is_some(),
            runner.is_some(),
            "block program generation {bank} existed in only one ownership map"
        );
        code.is_some()
    }

    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if self.physical_code.contains_bank(entry.bank) {
            if let Some(block) = self.mapped_aot.get(&entry) {
                if let Err(run) = block.preflight(&self.physical_code, ctx) {
                    return run;
                }
                let result = block.run_preflighted(budget, ctx, mem);
                self.observe_execution_destination(ExecutionDestinationObservation {
                    destination: entry,
                    runner_artifact_identity: block.runner_artifact_identity(),
                    instructions: result.instructions,
                });
                return result;
            }
            #[cfg(not(feature = "dev-interpreter"))]
            return BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::MissingAotEntry,
                }),
                0,
            );
            #[cfg(feature = "dev-interpreter")]
            {
                let unit = match admit_mapped_unit(&self.physical_code, entry.bank, entry.pc, ctx) {
                    Ok(unit) => unit,
                    Err(run) => return run,
                };
                let result = run_admitted_mapped_unit(unit, budget, ctx, mem).unwrap_or_else(
                    |unsupported| BlockRun::new(BlockExit::Fault(unsupported.into_cpu_fault()), 0),
                );
                self.observe_execution_destination(ExecutionDestinationObservation {
                    destination: entry,
                    runner_artifact_identity: None,
                    instructions: result.instructions,
                });
                return result;
            }
        }
        if let Err(fault) = self.code.resolve(entry) {
            let attempted_fetch = u32::from(matches!(fault.kind, CpuFaultKind::Exception { .. }));
            return BlockRun::new(BlockExit::Fault(fault), attempted_fetch);
        }
        let (run, runner_artifact_identity) =
            self.runners.get(&entry.bank).copied().unwrap_or_else(|| {
                panic!(
                    "block program invariant violated: admitted {} has no generated runner",
                    entry.bank
                )
            });
        let result = run(entry, budget, ctx, mem);
        if !matches!(result.exit, BlockExit::ImageChanged { .. }) {
            self.observe_execution_destination(ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity,
                instructions: result.instructions,
            });
        }
        result
    }

    /// Run the registered arbitrary-PC program through transfers and
    /// synchronous architectural exception entry until execution reaches a
    /// scheduler/device boundary.
    ///
    /// Exception vectors are virtual addresses, so they go through the same
    /// active-mapping resolver as computed transfers. CP0 state is committed
    /// before vector resolution; a missing vector therefore returns the
    /// resolver's mapping fault without erasing the guest exception state.
    pub fn dispatch<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, true)
    }

    /// Dispatch while returning architectural exceptions to the live owner.
    /// Hosts whose typed scheduler replaces libultra's raw thread dispatcher
    /// need to publish fault events and stop the current coroutine themselves;
    /// they must not run a second scheduler through the guest vector first.
    pub fn dispatch_exposing_exceptions<V>(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        self.dispatch_with_exception_vectoring(entry, budget, ctx, mem, resolver, false)
    }

    fn dispatch_with_exception_vectoring<V>(
        &self,
        mut entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
        vector_exceptions: bool,
    ) -> Result<DispatchRun, DispatchError>
    where
        V: TransferResolver,
    {
        let mut instructions = 0u32;
        let mut blocks = 0u32;

        loop {
            let remaining = budget.get() - instructions;
            if remaining < InstructionBudget::MIN {
                return Ok(DispatchRun {
                    exit: BlockExit::Checkpoint(entry),
                    instructions,
                    blocks,
                });
            }
            let turn_budget = InstructionBudget::new(remaining)
                .expect("remaining budget was checked against InstructionBudget::MIN");
            let run = self.run(entry, turn_budget, ctx, mem);
            let run = BlockRun::new(
                finalize_executable_write_exit(entry.bank, run.exit),
                run.instructions,
            );
            if run.instructions > remaining {
                return Err(DispatchError::RunnerExceededBudget {
                    at: entry,
                    budget: turn_budget,
                    actual: run.instructions,
                });
            }
            if run.instructions == 0
                && run.exit == BlockExit::Checkpoint(entry)
                && !turn_budget.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
            {
                return Err(DispatchError::IndivisibleUnitExceedsBudget {
                    at: entry,
                    budget: turn_budget,
                    required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
                });
            }
            let continuing_without_progress = run.instructions == 0
                && matches!(
                    run.exit,
                    BlockExit::Checkpoint(_)
                        | BlockExit::Transfer(_)
                        | BlockExit::ResolveTransfer { .. }
                        | BlockExit::ResolveCall { .. }
                        | BlockExit::ExecutableWrite { .. }
                        | BlockExit::ExecutableWriteResolveCall { .. }
                        | BlockExit::ExecutableWriteFault(_)
                        | BlockExit::Fault(CpuFault {
                            kind: CpuFaultKind::Exception { .. },
                            ..
                        })
                );
            if continuing_without_progress {
                return Err(DispatchError::ContinuingExitWithoutProgress {
                    at: entry,
                    exit: run.exit,
                });
            }
            instructions = instructions
                .checked_add(run.instructions)
                .ok_or(DispatchError::InstructionCountOverflow)?;
            blocks = blocks
                .checked_add(1)
                .ok_or(DispatchError::BlockCountOverflow)?;

            let resolution = match run.exit {
                BlockExit::ExecutableWrite {
                    source_bank,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWrite {
                            source_bank,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteResolveCall {
                            source_bank,
                            target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteFault(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteFault(fault),
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ImageChanged { at, miss } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ImageChanged { at, miss },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::Transfer(next) => {
                    entry = next;
                    continue;
                }
                BlockExit::ResolveTransfer {
                    source_bank,
                    target_pc,
                } => resolver.resolve(source_bank, target_pc),
                BlockExit::ResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => match resolver.resolve_call(source_bank, target_pc, resume) {
                    Ok(CallResolution::Guest(next)) => {
                        entry = next;
                        continue;
                    }
                    Ok(CallResolution::Host) => {
                        return Ok(DispatchRun {
                            exit: BlockExit::HostCall {
                                vram: target_pc,
                                resume,
                            },
                            instructions,
                            blocks,
                        });
                    }
                    Err(fault) => Err(fault),
                },
                BlockExit::Fault(fault) => {
                    if !vector_exceptions && matches!(fault.kind, CpuFaultKind::Exception { .. }) {
                        return Ok(DispatchRun {
                            exit: BlockExit::Fault(fault),
                            instructions,
                            blocks,
                        });
                    }
                    let Some(vector) = fault.enter_exception(ctx) else {
                        return Ok(DispatchRun {
                            exit: run.exit,
                            instructions,
                            blocks,
                        });
                    };
                    resolver.resolve(fault.at.bank, vector)
                }
                exit => {
                    return Ok(DispatchRun {
                        exit,
                        instructions,
                        blocks,
                    });
                }
            };

            match resolution {
                Ok(next) => entry = next,
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            }
        }
    }
}

/// Failure to bind a [`BlockProgram`] to one canonical catalog entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogBlockProgramErrorV1 {
    EntryNotAdmitted(CpuFault),
    MissingRunnerArtifactIdentity { bank: BankId },
    NonCanonicalProgramEvidence,
    GeneratedRunnerSourceAttestation(GeneratedRunnerSourceAttestationErrorV1),
}

impl fmt::Display for CatalogBlockProgramErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryNotAdmitted(fault) => {
                write!(
                    formatter,
                    "catalog block-program entry is not admitted: {fault}"
                )
            }
            Self::MissingRunnerArtifactIdentity { bank } => write!(
                formatter,
                "catalog block-program runner {bank} has no stable artifact identity"
            ),
            Self::NonCanonicalProgramEvidence => write!(
                formatter,
                "catalog block-program evidence is not canonically derived"
            ),
            Self::GeneratedRunnerSourceAttestation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CatalogBlockProgramErrorV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratedRunnerSourceAttestationErrorV1 {
    ZeroSourceDigest { field: &'static str },
    EmitterSourceReceiptMismatch,
    NonVirtualExecutionNotAttested,
    RunnerBindingCount { expected: usize, actual: usize },
    DuplicateRunnerBinding { bank: BankId },
    MissingRunnerBinding { bank: BankId },
    UnknownRunnerBinding { bank: BankId },
    EmptyCompositeRunner { bank: BankId },
    RunnerArtifactMismatch { bank: BankId },
}

impl fmt::Display for GeneratedRunnerSourceAttestationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSourceDigest { field } => {
                write!(formatter, "generated-runner source digest {field} is zero")
            }
            Self::EmitterSourceReceiptMismatch => formatter.write_str(
                "generated-runner external emitter or linked runtime source receipt mismatch",
            ),
            Self::NonVirtualExecutionNotAttested => formatter.write_str(
                "generated-runner source attestation v2 admits only virtual CodeBank runners",
            ),
            Self::RunnerBindingCount { expected, actual } => write!(
                formatter,
                "generated-runner source binding count {actual} does not match program runner count {expected}"
            ),
            Self::DuplicateRunnerBinding { bank } => {
                write!(formatter, "generated-runner source bindings repeat {bank}")
            }
            Self::MissingRunnerBinding { bank } => {
                write!(formatter, "generated-runner source binding is missing for {bank}")
            }
            Self::UnknownRunnerBinding { bank } => {
                write!(formatter, "generated-runner source binding names unknown {bank}")
            }
            Self::EmptyCompositeRunner { bank } => write!(
                formatter,
                "generated-runner source binding for {bank} contains zero emitted subrunners"
            ),
            Self::RunnerArtifactMismatch { bank } => write!(
                formatter,
                "generated-runner source/adapter identity does not match installed artifact for {bank}"
            ),
        }
    }
}

impl std::error::Error for GeneratedRunnerSourceAttestationErrorV1 {}

/// One canonical, fixed-entry execution substrate for a future ABI install.
///
/// Construction captures the existing pointer-independent program evidence
/// and the feature receipt compiled into this crate. The wrapper deliberately
/// exposes neither `BlockProgram` mutation nor transfer-resolver dispatch: a
/// replacement must arrive as a complete independently constructed program
/// and pass the same admission/evidence checks before the old one is retired.
pub struct CatalogBlockProgramV1 {
    program: BlockProgram,
    entry: ExecutionKey,
    budget: InstructionBudget,
    evidence: BlockProgramEvidenceSnapshot,
    build_receipt: StaticExecutionBuildReceipt,
    generated_runner_source_attestation: Option<GeneratedRunnerSourceAttestationV2>,
}

impl CatalogBlockProgramV1 {
    pub fn new(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        Self::new_inner(program, entry, budget, None)
    }

    /// Construct a catalog with a checked pointer-free Cargo-source
    /// attestation. This validates source, role, and program agreement but is
    /// intentionally not generated-runner semantics authority: any caller can
    /// still pair an arbitrary `GeneratedBankFn` with matching public fields.
    pub fn new_with_cargo_generated_runner_source_attestation_v2(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
        sources: CargoGeneratedProgramSourceAttestationV2<'_>,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        let attestation = Self::validate_generated_runner_source_attestation(&program, sources)
            .map_err(CatalogBlockProgramErrorV1::GeneratedRunnerSourceAttestation)?;
        Self::new_inner(program, entry, budget, Some(attestation))
    }

    fn new_inner(
        program: BlockProgram,
        entry: ExecutionKey,
        budget: InstructionBudget,
        generated_runner_source_attestation: Option<GeneratedRunnerSourceAttestationV2>,
    ) -> Result<Self, CatalogBlockProgramErrorV1> {
        Self::validate_entry(&program, entry)?;
        for (&bank, (_, artifact_identity)) in &program.runners {
            if artifact_identity.is_none() {
                return Err(CatalogBlockProgramErrorV1::MissingRunnerArtifactIdentity { bank });
            }
        }
        let evidence = program.evidence_snapshot();
        if evidence.identity.source != ProgramIdentitySource::CanonicalBlockProgramSha256 {
            return Err(CatalogBlockProgramErrorV1::NonCanonicalProgramEvidence);
        }
        Ok(Self {
            program,
            entry,
            budget,
            evidence,
            build_receipt: static_execution_build_receipt(),
            generated_runner_source_attestation,
        })
    }

    fn validate_generated_runner_source_attestation(
        program: &BlockProgram,
        sources: CargoGeneratedProgramSourceAttestationV2<'_>,
    ) -> Result<GeneratedRunnerSourceAttestationV2, GeneratedRunnerSourceAttestationErrorV1> {
        for (field, digest) in [
            (
                "root_adapter_source_sha256",
                sources.root_adapter_source_sha256,
            ),
            (
                "shard_cargo_source_tree_sha256",
                sources.shard_cargo_source_tree_sha256,
            ),
        ] {
            if digest == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest { field });
            }
        }
        if sources.expected_emitter_source_sha256 == [0; 32] {
            return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                field: "expected_emitter_source_sha256",
            });
        }
        if sources.expected_emitter_source_sha256
            != sources.externally_measured_emitter_source_sha256
        {
            return Err(GeneratedRunnerSourceAttestationErrorV1::EmitterSourceReceiptMismatch);
        }
        let linked_runtime = generated_runner_runtime_source_receipt_v1();
        if sources.runtime_source_receipt != linked_runtime
            || sources.expected_runtime_source_sha256 != linked_runtime.source_sha256()
        {
            return Err(GeneratedRunnerSourceAttestationErrorV1::EmitterSourceReceiptMismatch);
        }
        if !program.physical_code.evidence_snapshot().is_empty() || !program.mapped_aot.is_empty() {
            return Err(GeneratedRunnerSourceAttestationErrorV1::NonVirtualExecutionNotAttested);
        }
        if sources.runners.len() != program.runners.len() {
            return Err(
                GeneratedRunnerSourceAttestationErrorV1::RunnerBindingCount {
                    expected: program.runners.len(),
                    actual: sources.runners.len(),
                },
            );
        }

        let mut bindings = sources.runners.to_vec();
        bindings.sort_unstable_by_key(|binding| binding.bank);
        for pair in bindings.windows(2) {
            if pair[0].bank == pair[1].bank {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::DuplicateRunnerBinding {
                        bank: pair[0].bank,
                    },
                );
            }
        }
        for (&bank, (_, artifact_identity)) in &program.runners {
            let binding = bindings
                .binary_search_by_key(&bank, |binding| binding.bank)
                .ok()
                .map(|index| bindings[index])
                .ok_or(GeneratedRunnerSourceAttestationErrorV1::MissingRunnerBinding { bank })?;
            if binding.generated_runner_source_sha256 == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                    field: "generated_runner_source_sha256",
                });
            }
            if binding.code_words_sha256 == [0; 32] {
                return Err(GeneratedRunnerSourceAttestationErrorV1::ZeroSourceDigest {
                    field: "code_words_sha256",
                });
            }
            if binding.composite_subrunner_count == 0 {
                return Err(GeneratedRunnerSourceAttestationErrorV1::EmptyCompositeRunner { bank });
            }
            let expected_artifact = ProgramArtifactIdentity::generated_adapter(
                sources.root_adapter_source_sha256,
                binding.generated_runner_source_sha256,
                bank,
                binding.adapter_role,
            );
            if *artifact_identity != Some(expected_artifact) {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::RunnerArtifactMismatch { bank },
                );
            }
            let code = program
                .code
                .bank(bank)
                .expect("every registered runner has one atomically registered code bank");
            let mut code_hasher = Sha256::new();
            for span in code.spans() {
                for word in span.words() {
                    code_hasher.update(word.to_be_bytes());
                }
            }
            let actual_code_sha256: [u8; 32] = code_hasher.finalize().into();
            if code.vram_start() != binding.vram_start
                || code.vram_end() != binding.vram_end
                || actual_code_sha256 != binding.code_words_sha256
            {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::RunnerArtifactMismatch { bank },
                );
            }
        }
        for binding in &bindings {
            if !program.runners.contains_key(&binding.bank) {
                return Err(
                    GeneratedRunnerSourceAttestationErrorV1::UnknownRunnerBinding {
                        bank: binding.bank,
                    },
                );
            }
        }

        let evidence = program.evidence_snapshot();
        let mut binding_hasher = Sha256::new();
        binding_hasher.update(GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2);
        binding_hasher.update(evidence.identity.identity.bytes());
        binding_hasher.update(sources.root_adapter_source_sha256);
        binding_hasher.update(sources.shard_cargo_source_tree_sha256);
        binding_hasher.update(sources.externally_measured_emitter_source_sha256);
        binding_hasher.update(linked_runtime.source_sha256());
        for binding in bindings {
            binding_hasher.update(binding.bank.get().to_be_bytes());
            binding_hasher.update(binding.generated_runner_source_sha256);
            binding_hasher.update(binding.code_words_sha256);
            binding_hasher.update(binding.vram_start.get().to_be_bytes());
            binding_hasher.update(binding.vram_end.get().to_be_bytes());
            binding_hasher.update(binding.composite_subrunner_count.to_be_bytes());
            binding_hasher.update([binding.adapter_role.tag()]);
        }
        let build_receipt = static_execution_build_receipt();
        binding_hasher.update(build_receipt.schema.to_be_bytes());
        binding_hasher.update([
            u8::from(build_receipt.aot_runtime),
            u8::from(build_receipt.production_aot),
            u8::from(build_receipt.dev_interpreter),
        ]);
        Ok(GeneratedRunnerSourceAttestationV2 {
            schema: GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2,
            cargo_source_fields_validated: true,
            program_identity: evidence.identity.identity,
            root_adapter_source_sha256: sources.root_adapter_source_sha256,
            shard_cargo_source_tree_sha256: sources.shard_cargo_source_tree_sha256,
            emitter_source_sha256: sources.externally_measured_emitter_source_sha256,
            runtime_source_sha256: linked_runtime.source_sha256(),
            binding_sha256: binding_hasher.finalize().into(),
            build_receipt,
        })
    }

    fn validate_entry(
        program: &BlockProgram,
        entry: ExecutionKey,
    ) -> Result<(), CatalogBlockProgramErrorV1> {
        if !entry.pc.is_instruction_aligned() {
            return Err(CatalogBlockProgramErrorV1::EntryNotAdmitted(
                CpuFault::instruction_address_error(entry),
            ));
        }
        if program.physical_code.contains_bank(entry.bank) {
            return program.mapped_aot.contains_key(&entry).then_some(()).ok_or(
                CatalogBlockProgramErrorV1::EntryNotAdmitted(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::MissingAotEntry,
                }),
            );
        }
        program
            .code
            .resolve(entry)
            .map(|_| ())
            .map_err(CatalogBlockProgramErrorV1::EntryNotAdmitted)
    }

    pub const fn entry(&self) -> ExecutionKey {
        self.entry
    }

    pub const fn budget(&self) -> InstructionBudget {
        self.budget
    }

    pub const fn identity(&self) -> ProgramIdentityEvidenceSnapshot {
        self.evidence.identity
    }

    pub fn evidence(&self) -> &BlockProgramEvidenceSnapshot {
        &self.evidence
    }

    pub const fn build_receipt(&self) -> StaticExecutionBuildReceipt {
        self.build_receipt
    }

    pub const fn generated_runner_source_attestation(
        &self,
    ) -> Option<&GeneratedRunnerSourceAttestationV2> {
        self.generated_runner_source_attestation.as_ref()
    }

    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.program.copy_execution_destinations()
    }

    /// Whether the immutable static install owns this bank identity through
    /// either virtual code or the physical mapped-code catalog.
    ///
    /// Dynamic operational catalogs use this complete query to keep their
    /// content-derived identities disjoint from every static execution lane.
    pub fn reserves_bank(&self, bank: BankId) -> bool {
        self.program.code.bank(bank).is_some() || self.program.physical_code.contains_bank(bank)
    }

    /// Whether either the immutable program or any precompiled generation,
    /// including an inactive generation, reserves this bank identity.
    pub fn reserves_bank_with_generations(
        &self,
        bank: BankId,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> bool {
        self.reserves_bank(bank) || generations.contains_reserved_bank(bank)
    }

    /// Resolve a static virtual entry without preferring the wrapper's entry
    /// bank. The owned entry bank is used only to retain typed fault context.
    pub fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        self.program.code.resolve_entry(self.entry.bank, target_pc)
    }

    /// Resolve a static virtual transfer with exact source-bank preference.
    /// Active physical/dynamic generation selection remains an outer owner.
    pub fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program.code.resolve_transfer(source_bank, target_pc)
    }

    pub fn validate_precompiled_generations(
        &self,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<(), GenerationCatalogError> {
        generations.validate_program(&self.program)
    }

    pub fn resolve_entry_with_generations(
        &self,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                self.entry.bank,
                target_pc,
            )));
        }
        match generations.resolve_active(target_pc) {
            Ok(entry) => Ok(entry),
            Err(crate::generation::GenerationLookupError::NoActiveGeneration { .. }) => {
                Err(CpuFault {
                    at: ExecutionKey::new(self.entry.bank, target_pc),
                    kind: CpuFaultKind::NoActiveGeneration,
                })
            }
            Err(crate::generation::GenerationLookupError::UnmappedPc { .. }) => self
                .program
                .code
                .resolve_entry_where(self.entry.bank, target_pc, |bank| {
                    !generations.contains_reserved_bank(bank)
                }),
            Err(error) => unreachable!("resolve_active returned activation-time error: {error}"),
        }
    }

    pub fn resolve_transfer_with_generations(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        if !target_pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(ExecutionKey::new(
                source_bank,
                target_pc,
            )));
        }
        match generations.resolve_active(target_pc) {
            Ok(entry) => Ok(entry),
            Err(crate::generation::GenerationLookupError::NoActiveGeneration { .. }) => {
                Err(CpuFault {
                    at: ExecutionKey::new(source_bank, target_pc),
                    kind: CpuFaultKind::NoActiveGeneration,
                })
            }
            Err(crate::generation::GenerationLookupError::UnmappedPc { .. }) => self
                .program
                .code
                .resolve_transfer_where(source_bank, target_pc, |bank| {
                    !generations.contains_reserved_bank(bank)
                }),
            Err(error) => unreachable!("resolve_active returned activation-time error: {error}"),
        }
    }

    /// Execute exactly the entry and budget owned by this substrate. Transfer
    /// resolution remains an outer ABI responsibility and is not accepted as
    /// a callback here.
    pub fn run(&self, ctx: &mut RecompContext, mem: &mut Rdram<'_>) -> BlockRun {
        self.program.run(self.entry, self.budget, ctx, mem)
    }

    /// Dispatch an arbitrary admitted continuation using only this owned
    /// static program and one exact host-function catalog. No resolver
    /// callback or ambient host lookup participates in the decision.
    pub fn dispatch_exposing_exceptions_at(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        self.dispatch_exposing_exceptions_at_budget(entry, hosts, self.budget, ctx, mem)
    }

    /// Dispatch with a caller-owned slice budget. This is the budget-preserving
    /// seam used when static and dynamic execution share one architectural
    /// checkpoint; it does not mutate the install's configured outer budget.
    pub fn dispatch_exposing_exceptions_at_budget(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        let mut resolver = CatalogStaticTransferResolverV1 {
            program: self,
            hosts,
        };
        self.program
            .dispatch_exposing_exceptions(entry, budget, ctx, mem, &mut resolver)
    }

    pub fn dispatch_exposing_exceptions_with_generations_at(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        generations: &BackedPrecompiledGenerationCatalogV1,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        self.dispatch_exposing_exceptions_with_generations_at_budget(
            entry,
            hosts,
            generations,
            self.budget,
            ctx,
            mem,
        )
    }

    pub fn dispatch_exposing_exceptions_with_generations_at_budget(
        &self,
        entry: ExecutionKey,
        hosts: &HostFunctionCatalogV1,
        generations: &BackedPrecompiledGenerationCatalogV1,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> Result<DispatchRun, DispatchError> {
        let mut resolver = CatalogGenerationTransferResolverV1 {
            program: self,
            hosts,
            generations,
        };
        self.program
            .dispatch_exposing_exceptions(entry, budget, ctx, mem, &mut resolver)
    }

    pub fn set_entry(&mut self, entry: ExecutionKey) -> Result<(), CatalogBlockProgramErrorV1> {
        Self::validate_entry(&self.program, entry)?;
        self.entry = entry;
        Ok(())
    }

    pub fn set_budget(&mut self, budget: InstructionBudget) {
        self.budget = budget;
    }

    /// Atomically replace the complete program and its entry. Validation and
    /// canonical evidence capture finish before the installed substrate is
    /// changed.
    pub fn replace_program(
        &mut self,
        program: BlockProgram,
        entry: ExecutionKey,
    ) -> Result<(), CatalogBlockProgramErrorV1> {
        let replacement = Self::new(program, entry, self.budget)?;
        *self = replacement;
        Ok(())
    }
}

struct CatalogStaticTransferResolverV1<'a> {
    program: &'a CatalogBlockProgramV1,
    hosts: &'a HostFunctionCatalogV1,
}

impl TransferResolver for CatalogStaticTransferResolverV1<'_> {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if self.hosts.resolve(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

struct CatalogGenerationTransferResolverV1<'a> {
    program: &'a CatalogBlockProgramV1,
    hosts: &'a HostFunctionCatalogV1,
    generations: &'a BackedPrecompiledGenerationCatalogV1,
}

impl TransferResolver for CatalogGenerationTransferResolverV1<'_> {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program
            .resolve_transfer_with_generations(source_bank, target_pc, self.generations)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if self.hosts.resolve(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

/// Failure to publish a new executable generation into a fixed virtual
/// region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationError {
    RegionMismatch {
        region_start: GuestPc,
        region_end: GuestPc,
        bank_start: GuestPc,
        bank_end: GuestPc,
    },
    Program(ProgramError),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RegionMismatch {
                region_start,
                region_end,
                bank_start,
                bank_end,
            } => write!(
                f,
                "executable generation [{bank_start}, {bank_end}) does not exactly replace region [{region_start}, {region_end})"
            ),
            Self::Program(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for GenerationError {}

/// One virtual code region with exactly one active immutable generation.
///
/// Installing a replacement removes the old `CodeBank` and generated runner
/// together before publishing the new pair. The region therefore never
/// resolves stale code by virtual address after a successful rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutableRegion {
    start: GuestPc,
    end: GuestPc,
    active: Option<BankId>,
}

impl ExecutableRegion {
    pub fn new(start: GuestPc, end: GuestPc) -> Self {
        assert!(start < end, "executable region must be nonempty");
        assert!(
            start.is_instruction_aligned() && end.is_instruction_aligned(),
            "executable region bounds must be instruction-aligned"
        );
        Self {
            start,
            end,
            active: None,
        }
    }

    pub const fn active_bank(self) -> Option<BankId> {
        self.active
    }

    pub const fn start(self) -> GuestPc {
        self.start
    }

    pub const fn end(self) -> GuestPc {
        self.end
    }

    pub fn resolve(self, pc: GuestPc) -> Option<ExecutionKey> {
        if pc < self.start || pc >= self.end {
            return None;
        }
        self.active.map(|bank| ExecutionKey::new(bank, pc))
    }

    pub fn install(
        &mut self,
        program: &mut BlockProgram,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<Option<BankId>, GenerationError> {
        if code.vram_start() != self.start || code.vram_end() != self.end {
            return Err(GenerationError::RegionMismatch {
                region_start: self.start,
                region_end: self.end,
                bank_start: code.vram_start(),
                bank_end: code.vram_end(),
            });
        }
        let bank = code.id();
        if runner.bank() != bank {
            return Err(GenerationError::Program(ProgramError::RunnerBankMismatch {
                code_bank: bank,
                runner_bank: runner.bank(),
            }));
        }
        if program.code().bank(bank).is_some() {
            return Err(GenerationError::Program(ProgramError::DuplicateBank {
                bank,
            }));
        }

        let retired = self.active;
        if let Some(previous) = retired {
            assert!(
                program.unregister(previous),
                "active executable region referenced missing generation {previous}"
            );
        }
        program
            .register(code, runner)
            .map_err(GenerationError::Program)?;
        self.active = Some(bank);
        Ok(retired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_runner_runtime_receipts_preserve_v1_and_issue_source_complete_v2() {
        let v1 = generated_runner_runtime_source_receipt_v1();
        assert_eq!(v1.schema(), GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1);
        assert_ne!(v1.source_sha256(), [0; 32]);
        assert_eq!(v1, generated_runner_runtime_source_receipt_v1());

        let v2 = generated_runner_runtime_source_receipt_v2();
        assert_eq!(v2.schema(), GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2);
        assert_ne!(v2.source_sha256(), [0; 32]);
        assert_ne!(v2.source_sha256(), v1.source_sha256());
        assert_eq!(v2, generated_runner_runtime_source_receipt_v2());
        assert!(v2.typed_rdram());
        assert!(v2.typed_mmio());
        assert!(v2.typed_host_boundaries());
    }

    #[test]
    fn precompiled_image_admission_hashes_live_architectural_bytes_and_fails_closed() {
        let words = [0x3c1a_8003u32, 0x275a_6790, 0x0340_0008, 0];
        let expected: [u8; 32] = Sha256::digest(
            words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect::<Vec<_>>(),
        )
        .into();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        for (index, word) in words.into_iter().enumerate() {
            mem.store_w(0xffff_ffff_8000_0180 + index as u64 * 4, word);
        }
        let bank = BankId::new(0x1234);
        assert_eq!(
            verify_precompiled_image(bank, GuestPc::new(0x8000_0180), 16, expected, &mem),
            Ok(())
        );

        mem.store_w(0xffff_ffff_8000_018c, 1);
        let miss = verify_precompiled_image(bank, GuestPc::new(0x8000_0180), 16, expected, &mem)
            .unwrap_err();
        assert_eq!(miss.expected_bank, bank);
        assert_ne!(miss.actual_sha256, miss.expected_sha256);
        assert!(miss
            .to_string()
            .starts_with("AotMiss for bank:0000000000001234"));
    }

    #[test]
    fn instruction_admission_ignores_neighbors_and_fails_before_a_changed_word() {
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        let pc = GuestPc::new(0x8000_0180);
        let bank = BankId::new(0x5678);
        mem.store_w(0xffff_ffff_8000_017c, 0xdead_beef);
        mem.store_w(0xffff_ffff_8000_0180, 0x2402_0001);
        mem.store_w(0xffff_ffff_8000_0184, 0xcafe_babe);
        assert_eq!(
            verify_precompiled_instruction_word(bank, pc, 0x2402_0001, &mem),
            Ok(())
        );

        mem.store_w(0xffff_ffff_8000_0180, 0x2402_0002);
        let miss = verify_precompiled_instruction_word(bank, pc, 0x2402_0001, &mem)
            .expect_err("changed fetched word must fail closed");
        assert_eq!(miss.expected_bank, bank);
        assert_eq!(miss.va_start, pc);
        assert_eq!(miss.byte_len, 4);
        assert_ne!(miss.actual_sha256, miss.expected_sha256);
    }

    #[test]
    fn synchronous_exception_entry_sets_epc_bd_exl_cause_and_vector() {
        let bank = BankId::new(7);
        let mut ctx = RecompContext::new();
        ctx.cop0_cause = 0x0000_0100; // preserve an unrelated pending bit
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Breakpoint,
                epc: GuestPc::new(0x8000_1000),
                branch_delay: true,
                instruction_code: 7,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1000);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & 0x100, 0);
    }

    #[test]
    fn floating_point_exception_enters_general_vector_with_exc_code_15() {
        let bank = BankId::new(0xF1);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1804)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::FloatingPoint,
                epc: GuestPc::new(0x8000_1800),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1800);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 15);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
    }

    #[test]
    fn nested_exception_preserves_first_epc_bd_and_bev_selects_boot_vector() {
        let bank = BankId::new(8);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 22); // EXL + BEV
        ctx.cop0_epc = 0x8000_2000;
        ctx.cop0_cause = 1 << 31;
        let nested = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_3000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Syscall,
                epc: GuestPc::new(0x8000_3000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            nested.enter_exception(&mut ctx),
            Some(GuestPc::new(0xBFC0_0380))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_2000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 8);
    }

    #[test]
    fn address_exception_commits_badvaddr_and_architectural_cause_code() {
        let bank = BankId::new(9);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0001),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0001);
        assert_eq!(ctx.cop0_epc, 0x8000_4000);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 4);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn tlb_refill_commits_translation_registers_and_selects_refill_vector() {
        let bank = BankId::new(0x71);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_entry_hi = 0x0000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x1234_5678),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0000))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x1234_5678);
        assert_eq!(ctx.cop0_context, 0xab89_1a20);
        assert_eq!(ctx.cop0_entry_hi, 0x1234_4042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0200))
        );
    }

    #[test]
    fn xtlb_refill_commits_full_translation_state_and_selects_extended_vector() {
        const BAD_VADDR: u64 = 0x4000_0088_7654_2040;
        let bank = BankId::new(0x73);
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::XTlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_xcontext = 0x1234_5678_0000_0000;
        ctx.cop0_entry_hi = 0x51;
        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0080))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context & 0xff80_0000, 0xab80_0000);
        assert_eq!(
            ctx.cop0_context & 0x007f_fff0,
            ((BAD_VADDR as u32) >> 9) & 0x007f_fff0
        );
        assert_eq!(
            ctx.cop0_xcontext & 0xffff_fffe_0000_0000,
            0x1234_5678_0000_0000 & 0xffff_fffe_0000_0000
        );
        assert_eq!((ctx.cop0_xcontext >> 31) & 0b11, BAD_VADDR >> 62);
        assert_eq!(
            (ctx.cop0_xcontext >> 4) & 0x07ff_ffff,
            (BAD_VADDR >> 13) & 0x07ff_ffff
        );
        assert_eq!(
            ctx.cop0_entry_hi,
            (BAD_VADDR & 0xc000_00ff_ffff_e000) | 0x51
        );
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0280))
        );

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        assert_eq!(
            fault.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, BAD_VADDR);
    }

    #[test]
    fn extended_address_error_retains_full_badvaddr_without_tlb_state_updates() {
        const BAD_VADDR: u64 = 0x9000_0001_0000_0040;
        let bank = BankId::new(0x74);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xabcd_1234;
        ctx.cop0_xcontext = 0x1234_5678_9abc_def0;
        ctx.cop0_entry_hi = 0x4000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context, 0xabcd_1234);
        assert_eq!(ctx.cop0_xcontext, 0x1234_5678_9abc_def0);
        assert_eq!(ctx.cop0_entry_hi, 0x4000_0042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 5);
    }

    #[test]
    fn invalid_modified_and_nested_refill_use_the_common_vector() {
        let bank = BankId::new(0x72);
        for (exception, expected_code) in [
            (CpuException::TlbInvalidStore, 3),
            (CpuException::TlbModified, 1),
        ] {
            let mut ctx = RecompContext::new();
            let fault = CpuFault {
                at: ExecutionKey::new(bank, GuestPc::new(0x8000_5000)),
                kind: CpuFaultKind::Exception {
                    exception,
                    epc: GuestPc::new(0x8000_5000),
                    branch_delay: false,
                    instruction_code: 0,
                    bad_vaddr: Some(0x0040_0000),
                    coprocessor: None,
                },
            };
            assert_eq!(
                fault.enter_exception(&mut ctx),
                Some(GuestPc::new(0x8000_0180))
            );
            assert_eq!((ctx.cop0_cause >> 2) & 0x1f, expected_code);
        }

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        let refill = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0xc001_2345),
                coprocessor: None,
            },
        };
        assert_eq!(
            refill.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, 0xc001_2345);
    }

    #[test]
    fn nested_address_exception_updates_badvaddr_without_replacing_epc_or_bd() {
        let bank = BankId::new(10);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 1;
        ctx.cop0_epc = 0x8000_5000;
        ctx.cop0_cause = 1 << 31;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0002),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0002);
        assert_eq!(ctx.cop0_epc, 0x8000_5000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 5);
    }

    #[test]
    fn coprocessor_unusable_exception_records_cause_ce() {
        let bank = BankId::new(11);
        for coprocessor in [0, 1] {
            let mut ctx = RecompContext::new();
            ctx.cop0_cause = 3 << 28;
            let fault = CpuFault {
                at: ExecutionKey::new(bank, GuestPc::new(0x8000_7000)),
                kind: CpuFaultKind::Exception {
                    exception: CpuException::CoprocessorUnusable,
                    epc: GuestPc::new(0x8000_7000),
                    branch_delay: false,
                    instruction_code: 0,
                    bad_vaddr: None,
                    coprocessor: Some(coprocessor),
                },
            };

            assert_eq!(
                fault.enter_exception(&mut ctx),
                Some(GuestPc::new(0x8000_0180))
            );
            assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 11);
            assert_eq!((ctx.cop0_cause >> 28) & 0b11, u32::from(coprocessor));
            assert_eq!(ctx.cop0_epc, 0x8000_7000);
        }
    }

    #[test]
    fn level_sensitive_interrupt_entry_obeys_ie_im_exl_and_erl() {
        let mut ctx = RecompContext::new();
        let interrupted = GuestPc::new(0x8000_1000);
        CpuInterruptLine::RCP.set_level(&mut ctx, true);
        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        ctx.cop0_status = 1 | CpuInterruptLine::RCP.cause_bit();
        ctx.cop0_cause |= (9 << 2) | (1 << 31);
        assert_eq!(
            enter_pending_interrupt(&mut ctx, interrupted),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, interrupted.get());
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 0);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        CpuInterruptLine::RCP.set_level(&mut ctx, false);
        assert_eq!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);
    }

    const VA: GuestPc = GuestPc::new(0x8000_1000);

    fn bank(id: u64, words: &[u32]) -> CodeBank {
        CodeBank::new(BankId::new(id), VA, words.to_vec()).unwrap()
    }

    fn instruction_entry_lo(physical_page: u32, valid: bool) -> u32 {
        ((physical_page >> 6) & 0x03ff_ffc0) | 1 | ((valid as u32) << 1) | (1 << 2)
    }

    fn map_instruction_pair(
        ctx: &mut RecompContext,
        virtual_pair: u32,
        even_physical: u32,
        odd_physical: u32,
        odd_valid: bool,
    ) {
        ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: u64::from(virtual_pair & 0xffff_e000),
            entry_lo0: instruction_entry_lo(even_physical, true),
            entry_lo1: instruction_entry_lo(odd_physical, odd_valid),
        };
    }

    fn first_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 1);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn second_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 2);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn catalog_dispatch_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if entry.pc == VA {
            return BlockRun::new(
                BlockExit::ResolveCall {
                    source_bank: entry.bank,
                    target_pc: GuestPc::new(VA.get() + 4),
                    resume: ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 8)),
                },
                1,
            );
        }
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn catalog_dispatch_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn catalog_budget_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, budget.get().try_into().unwrap());
        BlockRun::new(BlockExit::Yield(entry), budget.get())
    }

    fn catalog_test_program(
        id: BankId,
        runner: GeneratedBankFn,
        artifact_byte: u8,
    ) -> BlockProgram {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(id, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    id,
                    runner,
                    ProgramArtifactIdentity::new([artifact_byte; 32]),
                ),
            )
            .unwrap();
        program
    }

    #[test]
    fn catalog_block_program_captures_canonical_evidence_and_fixed_execution() {
        let id = BankId::new(0xc001);
        let entry = ExecutionKey::new(id, VA);
        let budget = InstructionBudget::new(2).unwrap();
        let program = catalog_test_program(id, first_runner, 0x11);
        let expected_evidence = program.evidence_snapshot();
        let catalog = CatalogBlockProgramV1::new(program, entry, budget).unwrap();

        assert_eq!(catalog.entry(), entry);
        assert_eq!(catalog.budget(), budget);
        assert_eq!(catalog.evidence(), &expected_evidence);
        assert_eq!(catalog.identity(), expected_evidence.identity);
        assert_eq!(catalog.build_receipt(), static_execution_build_receipt());
        assert!(catalog.reserves_bank(id));
        assert!(!catalog.reserves_bank(BankId::new(0xc0ff)));
        assert_eq!(catalog.resolve_entry(VA).unwrap(), entry);
        assert_eq!(catalog.resolve_transfer(id, VA).unwrap(), entry);

        let mut storage = [];
        let mut memory = Rdram::new(&mut storage);
        let mut context = RecompContext::new();
        assert_eq!(catalog.run(&mut context, &mut memory).instructions, 1);
        assert_eq!(context.r_u32(2), 1);
        assert_eq!(catalog.copy_execution_destinations()[0].destination, entry);
    }

    #[test]
    fn catalog_block_program_rejects_unadmitted_entry_and_unidentified_runner() {
        let id = BankId::new(0xc002);
        let hole = ExecutionKey::new(id, GuestPc::new(VA.get() + 8));
        assert!(matches!(
            CatalogBlockProgramV1::new(
                catalog_test_program(id, first_runner, 0x22),
                hole,
                InstructionBudget::new(2).unwrap(),
            ),
            Err(CatalogBlockProgramErrorV1::EntryNotAdmitted(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            }))
        ));

        let mut unidentified = BlockProgram::new();
        unidentified
            .register(
                CodeBank::new(id, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(id, first_runner),
            )
            .unwrap();
        assert!(matches!(
            CatalogBlockProgramV1::new(
                unidentified,
                ExecutionKey::new(id, VA),
                InstructionBudget::new(2).unwrap(),
            ),
            Err(CatalogBlockProgramErrorV1::MissingRunnerArtifactIdentity { bank })
                if bank == id
        ));
    }

    #[test]
    fn catalog_block_program_replacement_is_validated_before_installation() {
        let first = BankId::new(0xc003);
        let second = BankId::new(0xc004);
        let budget = InstructionBudget::new(2).unwrap();
        let mut catalog = CatalogBlockProgramV1::new(
            catalog_test_program(first, first_runner, 0x33),
            ExecutionKey::new(first, VA),
            budget,
        )
        .unwrap();
        let first_identity = catalog.identity();

        assert!(catalog
            .replace_program(
                catalog_test_program(second, second_runner, 0x44),
                ExecutionKey::new(second, GuestPc::new(VA.get() + 8)),
            )
            .is_err());
        assert_eq!(catalog.identity(), first_identity);
        assert_eq!(catalog.entry(), ExecutionKey::new(first, VA));

        catalog
            .replace_program(
                catalog_test_program(second, second_runner, 0x44),
                ExecutionKey::new(second, VA),
            )
            .unwrap();
        assert_ne!(catalog.identity(), first_identity);
        let mut storage = [];
        let mut memory = Rdram::new(&mut storage);
        let mut context = RecompContext::new();
        catalog.run(&mut context, &mut memory);
        assert_eq!(context.r_u32(2), 2);
    }

    #[test]
    fn catalog_block_dispatch_prefers_host_call_over_overlapping_guest_code() {
        let bank = BankId::new(0xc005);
        let entry = ExecutionKey::new(bank, VA);
        let mut block_program = BlockProgram::new();
        block_program
            .register(
                CodeBank::new(bank, VA, vec![0, 0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    catalog_dispatch_runner,
                    ProgramArtifactIdentity::new([0x55; 32]),
                ),
            )
            .unwrap();
        let program =
            CatalogBlockProgramV1::new(block_program, entry, InstructionBudget::new(4).unwrap())
                .unwrap();
        let target = GuestPc::new(VA.get() + 4);
        let resume = ExecutionKey::new(bank, GuestPc::new(VA.get() + 8));
        let hosts =
            HostFunctionCatalogV1::new(vec![(target.get(), catalog_dispatch_host)]).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        assert_eq!(
            program.resolve_transfer(bank, target),
            Ok(ExecutionKey::new(bank, target)),
            "the host target must also be admitted guest code for this precedence regression"
        );

        assert_eq!(
            program
                .dispatch_exposing_exceptions_at(entry, &hosts, &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::HostCall {
                vram: target,
                resume,
            }
        );

        let no_hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        assert_eq!(
            program
                .dispatch_exposing_exceptions_at(entry, &no_hosts, &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::Yield(ExecutionKey::new(bank, target))
        );
    }

    #[test]
    fn catalog_block_dispatch_accepts_an_explicit_slice_budget() {
        let bank = BankId::new(0xc006);
        let entry = ExecutionKey::new(bank, VA);
        let program = CatalogBlockProgramV1::new(
            catalog_test_program(bank, catalog_budget_runner, 0x56),
            entry,
            InstructionBudget::new(4).unwrap(),
        )
        .unwrap();
        let hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        let one = program
            .dispatch_exposing_exceptions_at_budget(
                entry,
                &hosts,
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
            )
            .unwrap();
        assert_eq!(one.instructions, 2);
        assert_eq!(ctx.r_u32(2), 2);
        assert_eq!(program.budget().get(), 4);

        let installed = program
            .dispatch_exposing_exceptions_at(entry, &hosts, &mut ctx, &mut mem)
            .unwrap();
        assert_eq!(installed.instructions, 4);
        assert_eq!(ctx.r_u32(2), 4);
    }

    #[test]
    fn catalog_reservation_includes_physical_code_banks() {
        let static_bank = BankId::new(0xc007);
        let physical_bank = BankId::new(0xc008);
        let mut block_program = catalog_test_program(static_bank, first_runner, 0x57);
        block_program
            .register_physical_code(mapped_observation_bank(physical_bank))
            .unwrap();
        let program = CatalogBlockProgramV1::new(
            block_program,
            ExecutionKey::new(static_bank, VA),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap();

        assert!(program.reserves_bank(static_bank));
        assert!(program.reserves_bank(physical_bank));
        assert!(!program.reserves_bank(BankId::new(0xc009)));
    }

    #[test]
    fn catalog_resolution_reserves_inactive_generation_banks_until_digest_activation() {
        let first = BankId::new(0xc101);
        let second = BankId::new(0xc102);
        let static_bank = BankId::new(0xc103);
        let static_pc = GuestPc::new(VA.get() + 0x100);
        let image_a = 0x2402_0001u32.to_be_bytes();
        let image_b = 0x2402_0002u32.to_be_bytes();
        let mut block_program = BlockProgram::new();
        for (bank, pc, word, identity) in [
            (first, VA, 0x2402_0001, 0x81),
            (second, VA, 0x2402_0002, 0x82),
            (static_bank, static_pc, 0, 0x83),
        ] {
            block_program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        first_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let program = CatalogBlockProgramV1::new(
            block_program,
            ExecutionKey::new(static_bank, static_pc),
            InstructionBudget::new(4).unwrap(),
        )
        .unwrap();
        let mut catalog = crate::generation::PrecompiledGenerationCatalog::new();
        for (id, bank, bytes) in [(1, first, image_a), (2, second, image_b)] {
            catalog
                .register(
                    crate::generation::PrecompiledGeneration::new(
                        crate::generation::GenerationId::new(id),
                        VA,
                        GuestPc::new(VA.get() + 4),
                        VA,
                        GuestPc::new(VA.get() + 4),
                        Sha256::digest(bytes).into(),
                        vec![crate::generation::PrecompiledShard::new(
                            bank,
                            VA,
                            GuestPc::new(VA.get() + 4),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let backing = |id| {
            crate::generation::PrecompiledGenerationBackingV1::new(
                crate::generation::GenerationId::new(id),
                vec![crate::generation::BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
            )
            .unwrap()
        };
        let mut generations = crate::generation::BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![backing(2), backing(1)],
        )
        .unwrap();
        program
            .validate_precompiled_generations(&generations)
            .unwrap();
        assert!(program.reserves_bank_with_generations(first, &generations));
        assert!(program.reserves_bank_with_generations(second, &generations));
        assert!(program.reserves_bank_with_generations(static_bank, &generations));
        assert!(!program.reserves_bank_with_generations(BankId::new(0xc104), &generations));

        assert!(matches!(
            program.resolve_entry_with_generations(VA, &generations),
            Err(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            })
        ));
        generations
            .activate_for_fetch_with_physical(VA, |physical| {
                image_a[usize::try_from(physical - 0x100).unwrap()]
            })
            .unwrap();
        assert_eq!(
            program
                .resolve_entry_with_generations(VA, &generations)
                .unwrap(),
            ExecutionKey::new(first, VA)
        );
        assert_eq!(
            program
                .resolve_transfer_with_generations(second, static_pc, &generations)
                .unwrap(),
            ExecutionKey::new(static_bank, static_pc)
        );
    }

    fn zero_progress_executable_write_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let resume = ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 12));
        let exit = match entry.pc.get() - VA.get() {
            0 => BlockExit::ExecutableWrite {
                source_bank: entry.bank,
                resume,
            },
            4 => BlockExit::ExecutableWriteResolveCall {
                source_bank: entry.bank,
                target_pc: GuestPc::new(0x8000_2000),
                resume,
            },
            8 => BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(entry)),
            _ => unreachable!("test runner received an unexpected entry"),
        };
        BlockRun::new(exit, 0)
    }

    fn observation_transfer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        match entry {
            key if key == ExecutionKey::new(first_bank, VA) => BlockRun::new(
                BlockExit::Transfer(ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4))),
                1,
            ),
            key if key == ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4)) => {
                BlockRun::new(
                    BlockExit::ResolveTransfer {
                        source_bank: first_bank,
                        target_pc: VA,
                    },
                    1,
                )
            }

            key if key == ExecutionKey::new(second_bank, VA) => {
                BlockRun::new(BlockExit::Yield(key), 1)
            }
            _ => unreachable!("observation runner received unexpected destination {entry}"),
        }
    }

    fn observation_host_call_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let bank = BankId::new(0x503);
        match entry.pc {
            pc if pc == VA => BlockRun::new(
                BlockExit::HostCall {
                    vram: GuestPc::new(0x8000_4000),
                    resume: ExecutionKey::new(bank, GuestPc::new(VA.get() + 4)),
                },
                1,
            ),
            pc if pc == GuestPc::new(VA.get() + 4) => BlockRun::new(BlockExit::Yield(entry), 1),
            _ => unreachable!("host-call runner received unexpected destination {entry}"),
        }
    }

    fn observation_image_changed_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if entry.pc == VA {
            return BlockRun::new(
                BlockExit::Transfer(ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 4))),
                3,
            );
        }
        let miss = AotMiss {
            expected_bank: entry.bank,
            va_start: VA,
            byte_len: 8,
            expected_sha256: [0x11; 32],
            actual_sha256: [0x22; 32],
        };
        BlockRun::new(BlockExit::ImageChanged { at: entry, miss }, 0)
    }

    #[test]
    fn resolves_an_interior_instruction_without_a_function_entry() {
        let mut catalog = CodeCatalog::new();
        catalog
            .register(bank(1, &[0x1111, 0x2222, 0x3333]))
            .unwrap();

        let key = ExecutionKey::new(BankId::new(1), GuestPc::new(VA.get() + 4));
        assert_eq!(catalog.resolve(key).unwrap().word, 0x2222);
    }

    #[test]
    fn static_transfer_resolution_prefers_an_admitting_source_bank() {
        let first = BankId::new(0xd001);
        let second = BankId::new(0xd002);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(first.get(), &[1, 2])).unwrap();
        catalog.register(bank(second.get(), &[3, 4])).unwrap();
        let target = GuestPc::new(VA.get() + 4);

        assert_eq!(
            catalog.resolve_transfer(second, target).unwrap(),
            ExecutionKey::new(second, target)
        );
        assert!(matches!(
            catalog.resolve_entry(first, target),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 2,
                },
            }) if at == ExecutionKey::new(first, target)
                && first_candidate == first
                && second_candidate == second
        ));
    }

    #[test]
    fn catalog_resolver_policy_evidence_is_implementation_issued_and_build_bound() {
        let evidence = catalog_resolver_policy_evidence_v1();
        assert_eq!(evidence.policy(), CATALOG_RESOLVER_POLICY_NAME_V1);
        assert_eq!(
            evidence.exception_vectors(),
            &CATALOG_RESOLVER_EXCEPTION_VECTORS_V1
        );
        assert!(evidence.aligned_pc_admission());
        assert!(evidence.exact_active_owner_resolution());
        assert!(evidence.explicit_thread_return_boundary());
        assert!(evidence.misaligned_target_fault());
        assert!(evidence.unmapped_or_ambiguous_target_fault());
        assert!(evidence.traps_enter_shared_resolver());
        assert_eq!(evidence.build_receipt(), static_execution_build_receipt());
    }

    #[test]
    fn static_transfer_resolution_admits_one_cross_bank_target() {
        let source = BankId::new(0xd010);
        let destination = BankId::new(0xd011);
        let target = GuestPc::new(0x8000_2000);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(source.get(), &[1])).unwrap();
        catalog
            .register(CodeBank::new(destination, target, vec![2]).unwrap())
            .unwrap();

        assert_eq!(
            catalog.resolve_transfer(source, target).unwrap(),
            ExecutionKey::new(destination, target)
        );
        assert_eq!(
            catalog.resolve_entry(source, target).unwrap(),
            ExecutionKey::new(destination, target)
        );
    }

    #[test]
    fn static_resolution_reports_ordered_complete_ambiguity() {
        let first = BankId::new(0xd020);
        let second = BankId::new(0xd021);
        let third = BankId::new(0xd022);
        let fault_bank = BankId::new(0xd0ff);
        let mut catalog = CodeCatalog::new();
        for id in [third, first, second] {
            catalog.register(bank(id.get(), &[1])).unwrap();
        }

        assert!(matches!(
            catalog.resolve_entry(fault_bank, VA),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 3,
                },
            }) if at == ExecutionKey::new(fault_bank, VA)
                && first_candidate == first
                && second_candidate == second
        ));
    }

    #[test]
    fn static_resolution_fails_typed_for_unmapped_unknown_and_misaligned_targets() {
        let known = BankId::new(0xd030);
        let unknown = BankId::new(0xd031);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(known.get(), &[1])).unwrap();
        let unmapped = GuestPc::new(0x8000_3000);

        assert!(matches!(
            catalog.resolve_transfer(known, unmapped),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc { bank_start, bank_end },
            }) if at == ExecutionKey::new(known, unmapped)
                && bank_start == VA.get()
                && bank_end == VA.get() + 4
        ));
        assert!(matches!(
            catalog.resolve_entry(unknown, unmapped),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::UnknownBank,
            }) if at == ExecutionKey::new(unknown, unmapped)
        ));

        let misaligned = GuestPc::new(VA.get() + 2);
        assert_eq!(
            catalog.resolve_transfer(known, misaligned),
            Err(CpuFault::instruction_address_error(ExecutionKey::new(
                known, misaligned,
            )))
        );
        assert_eq!(
            catalog.resolve_entry(unknown, misaligned),
            Err(CpuFault::instruction_address_error(ExecutionKey::new(
                unknown, misaligned,
            )))
        );
    }

    #[test]
    fn executable_region_rewrite_retires_stale_bank_and_runner_atomically() {
        let first = BankId::new(0x101);
        let second = BankId::new(0x102);
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(VA, GuestPc::new(VA.get() + 4));
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(first, VA, vec![0x2402_0001]).unwrap(),
                    GeneratedBankRunner::new(first, first_runner),
                )
                .unwrap(),
            None
        );
        let first_key = region.resolve(VA).unwrap();
        assert_eq!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(second, VA, vec![0x2402_0002]).unwrap(),
                    GeneratedBankRunner::new(second, second_runner),
                )
                .unwrap(),
            Some(first)
        );
        assert_eq!(region.active_bank(), Some(second));
        assert!(matches!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        let second_key = region.resolve(VA).unwrap();
        assert_eq!(second_key.bank, second);
        program.run(
            second_key,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(ctx.r_u32(2), 2);
    }

    #[test]
    fn same_virtual_address_resolves_by_bank_identity() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        catalog.register(bank(2, &[0x2222])).unwrap();

        let first = ExecutionKey::new(BankId::new(1), VA);
        let second = ExecutionKey::new(BankId::new(2), VA);
        assert_eq!(catalog.resolve(first).unwrap().word, 0x1111);
        assert_eq!(catalog.resolve(second).unwrap().word, 0x2222);
    }

    #[test]
    fn sparse_bank_sorts_spans_and_never_resolves_a_bounding_hole() {
        let id = BankId::new(3);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x3333]).unwrap(),
                CodeSpan::new(id, VA, vec![0x1111, 0x2222]).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(bank.vram_start(), VA);
        assert_eq!(bank.vram_end(), GuestPc::new(VA.get() + 0x24));
        assert_eq!(bank.instruction_count(), 3);

        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap()
                .word,
            0x3333
        );
        assert!(matches!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10)))
                .unwrap_err()
                .kind,
            CpuFaultKind::UnmappedPc { .. }
        ));
    }

    #[test]
    fn sparse_bank_rejects_overlap_and_cross_bank_spans() {
        let id = BankId::new(4);
        let overlap = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1, 2]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 4), vec![3]).unwrap(),
            ],
        );
        assert_eq!(
            overlap,
            Err(BankError::OverlappingSpans {
                bank: id,
                left_end: GuestPc::new(VA.get() + 8),
                right_start: GuestPc::new(VA.get() + 4),
            })
        );

        let other = BankId::new(5);
        assert_eq!(
            CodeBank::from_spans(id, vec![CodeSpan::new(other, VA, vec![1]).unwrap()]),
            Err(BankError::SpanBankMismatch {
                bank: id,
                span_bank: other,
                start: VA,
            })
        );
    }

    #[test]
    fn classify_uses_sparse_admission_and_rejects_holes() {
        let id = BankId::new(6);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![0x2402_0001]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x0100_0008]).unwrap(),
            ],
        )
        .unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog.classify(ExecutionKey::new(id, VA)).unwrap(),
            BankWordKind::Straight
        );
        assert_eq!(
            catalog
                .classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap(),
            BankWordKind::ControlTransfer
        );
        assert!(matches!(
            catalog.classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10))),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
    }

    #[test]
    fn block_program_registration_is_atomic_and_bank_qualified() {
        let first = BankId::new(10);
        let second = BankId::new(11);
        let mut program = BlockProgram::new();
        assert_eq!(
            program.register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(second, first_runner),
            ),
            Err(ProgramError::RunnerBankMismatch {
                code_bank: first,
                runner_bank: second,
            })
        );
        assert!(program.code().bank(first).is_none());

        program
            .register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(first, first_runner),
            )
            .unwrap();
        program
            .register(
                bank(11, &[0x2222]),
                GeneratedBankRunner::new(second, second_runner),
            )
            .unwrap();
        assert_eq!(
            program.register(
                bank(10, &[0x3333]),
                GeneratedBankRunner::new(first, first_runner),
            ),
            Err(ProgramError::DuplicateBank { bank: first })
        );

        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let budget = InstructionBudget::new(2).unwrap();
        let first_key = ExecutionKey::new(first, VA);
        let second_key = ExecutionKey::new(second, VA);
        assert_eq!(
            program
                .run(first_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            program
                .run(second_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 2);
    }

    #[test]
    fn block_program_observes_direct_transferred_and_resolved_entries_in_order() {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        let first_artifact = ProgramArtifactIdentity::new([0x51; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x52; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(first_bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    observation_transfer_runner,
                    first_artifact,
                ),
            )
            .unwrap();
        program
            .register(
                CodeBank::new(second_bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    observation_transfer_runner,
                    second_artifact,
                ),
            )
            .unwrap();

        let immutable_before = program.evidence_snapshot();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program
            .code()
            .resolve(ExecutionKey::new(first_bank, VA))
            .is_ok());
        assert!(program.copy_execution_destinations().is_empty());

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, first_bank);
            assert_eq!(target_pc, VA);
            Ok(ExecutionKey::new(second_bank, target_pc))
        };
        let run = program
            .dispatch(
                ExecutionKey::new(first_bank, VA),
                InstructionBudget::new(6).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(second_bank, VA))
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, VA),
                    runner_artifact_identity: Some(first_artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4),),
                    runner_artifact_identity: Some(first_artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(second_bank, VA),
                    runner_artifact_identity: Some(second_artifact),
                    instructions: 1,
                },
            ]
        );
        assert_eq!(
            immutable_before,
            program.evidence_snapshot(),
            "historical execution must not enter future-affecting program evidence"
        );
    }

    #[test]
    fn block_program_records_host_resume_only_when_guest_execution_reenters() {
        let bank = BankId::new(0x503);
        let artifact = ProgramArtifactIdentity::new([0x53; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    observation_host_call_runner,
                    artifact,
                ),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |_source_bank: BankId, _target_pc: GuestPc| {
            unreachable!("host-call fixture must not resolve a guest transfer")
        };

        let first = program
            .dispatch(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        let resume = match first.exit {
            BlockExit::HostCall { resume, .. } => resume,
            exit => panic!("expected host call, got {exit:?}"),
        };
        assert_eq!(program.copy_execution_destinations().len(), 1);

        let second = program
            .dispatch(
                resume,
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(second.exit, BlockExit::Yield(resume));
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, VA),
                    runner_artifact_identity: Some(artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: resume,
                    runner_artifact_identity: Some(artifact),
                    instructions: 1,
                },
            ]
        );
    }

    #[test]
    fn image_change_preserves_prior_progress_without_recording_stale_entry() {
        let bank = BankId::new(0x504);
        let artifact = ProgramArtifactIdentity::new([0x54; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    observation_image_changed_runner,
                    artifact,
                ),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |_source_bank: BankId, _target_pc: GuestPc| {
            unreachable!("the image-change fixture uses only direct transfers")
        };
        let run = program
            .dispatch(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(6).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(run.instructions, 3);
        assert!(matches!(
            run.exit,
            BlockExit::ImageChanged {
                at: ExecutionKey { pc, .. },
                ..
            } if pc == GuestPc::new(VA.get() + 4)
        ));
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, VA),
                runner_artifact_identity: Some(artifact),
                instructions: 3,
            }]
        );
    }

    #[test]
    fn block_program_observation_lifetime_is_explicit_and_program_local() {
        let bank = BankId::new(0x504);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(bank, first_runner),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, VA),
                runner_artifact_identity: None,
                instructions: 1,
            }]
        );
        assert!(BlockProgram::new().copy_execution_destinations().is_empty());
        program.clear_execution_destinations();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program.code().bank(bank).is_some());

        program.set_execution_destination_history_limit(NonZeroUsize::new(2));
        for _ in 0..3 {
            program.run(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
            );
        }
        assert_eq!(program.copy_execution_destinations().len(), 2);
        program.set_execution_destination_history_enabled(false);
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert!(program.copy_execution_destinations().is_empty());
        program.set_execution_destination_history_enabled(true);
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(program.copy_execution_destinations().len(), 1);
    }

    fn mapped_observation_bank(bank: BankId) -> PhysicalCodeBank {
        PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0000_0040, vec![0x4022_4800]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![0x1000_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![0x2402_0002]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0030_0000, vec![0x2403_0003]).unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn mapped_fetch_failures_do_not_record_an_entered_destination() {
        let bank = BankId::new(0x505);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();

        let mut misaligned_ctx = RecompContext::new();
        let misaligned = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x8000_0042)),
            budget,
            &mut misaligned_ctx,
            &mut mem,
        );
        assert!(matches!(
            misaligned.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    ..
                },
                ..
            })
        ));

        let mut refill_ctx = RecompContext::new();
        let refill = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0060_0000)),
            budget,
            &mut refill_ctx,
            &mut mem,
        );
        assert!(matches!(
            refill.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbRefillLoad,
                    ..
                },
                ..
            })
        ));

        let mut unmapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut unmapped_ctx,
            0x0080_0000,
            0x0040_0000,
            0x0040_1000,
            true,
        );
        let unmapped = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0080_0000)),
            budget,
            &mut unmapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            unmapped.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPhysicalInstruction { .. },
                ..
            })
        ));

        let mut delay_ctx = RecompContext::new();
        map_instruction_pair(&mut delay_ctx, 0x0040_0000, 0x0010_0000, 0x0030_0000, false);
        let delay = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0040_0ffc)),
            budget,
            &mut delay_ctx,
            &mut mem,
        );
        assert!(matches!(
            delay.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    branch_delay: true,
                    ..
                },
                ..
            })
        ));
        assert!(program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_history_records_only_admitted_units_with_honest_lane_identity() {
        let bank = BankId::new(0x506);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let aot_artifact = ProgramArtifactIdentity::new([0x56; 32]);
        let direct_aot_entry = GuestPc::new(0x8010_0000);
        let aot = MappedAotBlock::new(
            program.physical_code(),
            &RecompContext::new(),
            bank,
            direct_aot_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        program.register_mapped_aot(aot).unwrap();

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();
        let mut ctx = RecompContext::new();
        let interpreted_entry = GuestPc::new(0x8000_0040);
        let interpreted = program.run(
            ExecutionKey::new(bank, interpreted_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert!(matches!(interpreted.exit, BlockExit::Fault(_)));
        program.run(
            ExecutionKey::new(bank, direct_aot_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, interpreted_entry),
                    runner_artifact_identity: None,
                    instructions: interpreted.instructions,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, direct_aot_entry),
                    runner_artifact_identity: Some(aot_artifact),
                    instructions: 1,
                },
            ]
        );

        let mut stale_program = BlockProgram::new();
        stale_program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let stale_entry = GuestPc::new(0x0080_0000);
        let mut original_ctx = RecompContext::new();
        map_instruction_pair(
            &mut original_ctx,
            stale_entry.get(),
            0x0010_0000,
            0x0030_0000,
            true,
        );
        let stale = MappedAotBlock::new(
            stale_program.physical_code(),
            &original_ctx,
            bank,
            stale_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        stale_program.register_mapped_aot(stale).unwrap();
        let mut remapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut remapped_ctx,
            stale_entry.get(),
            0x0020_0000,
            0x0030_0000,
            true,
        );
        let stale_run = stale_program.run(
            ExecutionKey::new(bank, stale_entry),
            budget,
            &mut remapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            stale_run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(stale_program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_wraparound_delay_fetch_is_precise_and_records_only_after_admission() {
        let bank = BankId::new(0x507);
        let branch_word = 0x1000_0001;
        let delay_word = 0x2442_0005;
        let physical = PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![branch_word]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![delay_word]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();
        let entry = GuestPc::new(0xffff_fffc);
        let budget = InstructionBudget::new(2).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        let mut invalid_ctx = RecompContext::new();
        for tlb in &mut invalid_ctx.tlb_entries {
            tlb.entry_hi = 0x0040_0000;
        }
        invalid_ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0xffff_e000,
            entry_lo0: instruction_entry_lo(0x0010_0000, true),
            entry_lo1: instruction_entry_lo(0x0010_0000, true),
        };
        invalid_ctx.tlb_entries[1] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0,
            entry_lo0: instruction_entry_lo(0x0020_0000, false),
            entry_lo1: instruction_entry_lo(0x0020_1000, false),
        };
        let invalid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut invalid_ctx,
            &mut mem,
        );
        assert!(matches!(
            invalid.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey {
                    pc: GuestPc(0),
                    ..
                },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    epc,
                    branch_delay: true,
                    bad_vaddr: Some(0),
                    ..
                },
            }) if epc == entry
        ));
        assert!(program.copy_execution_destinations().is_empty());

        let mut valid_ctx = invalid_ctx;
        valid_ctx.tlb_entries[1].entry_lo0 = instruction_entry_lo(0x0020_0000, true);
        let valid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut valid_ctx,
            &mut mem,
        );
        assert_eq!(valid.instructions, 2);
        assert_eq!(
            valid.exit,
            BlockExit::ResolveTransfer {
                source_bank: bank,
                target_pc: GuestPc::new(4),
            }
        );
        assert_eq!(valid_ctx.r_u32(2), 5);
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, entry),
                runner_artifact_identity: None,
                instructions: 2,
            }]
        );
    }

    #[test]
    fn block_program_evidence_is_sorted_and_runner_pointer_independent() {
        let first = BankId::new(0x21);
        let second = BankId::new(0x22);
        let artifact = ProgramArtifactIdentity::new([0xA5; 32]);
        let mut forward = BlockProgram::new();
        forward
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, first_runner, artifact),
            )
            .unwrap();
        forward
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, second_runner, artifact),
            )
            .unwrap();

        let mut reverse_with_different_runners = BlockProgram::new();
        reverse_with_different_runners
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, first_runner, artifact),
            )
            .unwrap();
        reverse_with_different_runners
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, second_runner, artifact),
            )
            .unwrap();

        let snapshot = forward.evidence_snapshot();
        assert_eq!(snapshot, reverse_with_different_runners.evidence_snapshot());
        assert_eq!(
            snapshot.identity.source,
            ProgramIdentitySource::CanonicalBlockProgramSha256
        );
        assert_eq!(
            snapshot
                .banks
                .iter()
                .map(|bank| bank.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
    }

    #[test]
    fn block_program_identity_binds_bank_span_and_instruction_families() {
        fn snapshot(id: BankId, start: GuestPc, words: Vec<u32>) -> BlockProgramEvidenceSnapshot {
            let mut program = BlockProgram::new();
            program
                .register(
                    CodeBank::new(id, start, words).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        id,
                        first_runner,
                        ProgramArtifactIdentity::new([0xC3; 32]),
                    ),
                )
                .unwrap();
            program.evidence_snapshot()
        }

        let baseline = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2222]);
        let changed_bank = snapshot(BankId::new(0x32), VA, vec![0x1111, 0x2222]);
        let changed_span = snapshot(
            BankId::new(0x31),
            GuestPc::new(VA.get() + 4),
            vec![0x1111, 0x2222],
        );
        let changed_word = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2223]);

        for changed in [&changed_bank, &changed_span, &changed_word] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }

        let mut changed_runner_artifact = BlockProgram::new();
        changed_runner_artifact
            .register(
                CodeBank::new(BankId::new(0x31), VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    BankId::new(0x31),
                    first_runner,
                    ProgramArtifactIdentity::new([0x3C; 32]),
                ),
            )
            .unwrap();
        let changed_runner_artifact = changed_runner_artifact.evidence_snapshot();
        assert_ne!(baseline, changed_runner_artifact);
        assert_ne!(
            baseline.identity.identity,
            changed_runner_artifact.identity.identity
        );
    }

    #[test]
    fn generated_adapter_identity_binds_adapter_runner_and_bank() {
        let baseline = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            [0x22; 32],
            BankId::new(0x33),
            GeneratedAdapterRole::DirectGenerated,
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x10; 32],
                [0x22; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x23; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x22; 32],
                BankId::new(0x34),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x22; 32],
                [0x11; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x22; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::EntryContextGate,
            )
        );
    }

    fn mapped_evidence_snapshot(
        bank: BankId,
        spans: &[(u32, u32)],
        mappings: &[(GuestPc, u32, u32, ProgramArtifactIdentity)],
    ) -> BlockProgramEvidenceSnapshot {
        let physical = PhysicalCodeBank::from_spans(
            bank,
            spans
                .iter()
                .map(|&(physical_start, word)| {
                    crate::fetch::PhysicalCodeSpan::new(bank, physical_start, vec![word]).unwrap()
                })
                .collect(),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();

        let mut ctx = RecompContext::new();
        for (index, &(entry, physical_address, word, artifact)) in mappings.iter().enumerate() {
            assert_eq!(entry.get() & 0x1fff, 0);
            assert_eq!(physical_address & 0xfff, 0);
            ctx.tlb_entries[index] = crate::runtime::TlbEntryRaw {
                page_mask: 0,
                entry_hi: u64::from(entry.get() & 0xffff_e000),
                entry_lo0: ((physical_address >> 6) & 0x03ff_ffc0) | 0x7,
                entry_lo1: 0,
            };
            let block = MappedAotBlock::new(
                program.physical_code(),
                &ctx,
                bank,
                entry,
                &[word],
                GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, artifact),
            )
            .unwrap();
            program.register_mapped_aot(block).unwrap();
        }
        program.evidence_snapshot()
    }

    #[test]
    fn mapped_block_program_evidence_is_canonical_across_registration_order() {
        let bank = BankId::new(0x51);
        let first_entry = GuestPc::new(0x0040_0000);
        let second_entry = GuestPc::new(0x0040_2000);
        let first_word = 0x2402_0001;
        let second_word = 0x2403_0002;
        let first_artifact = ProgramArtifactIdentity::new([0x11; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x22; 32]);
        let forward = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, first_word), (0x0020_0000, second_word)],
            &[
                (first_entry, 0x0010_0000, first_word, first_artifact),
                (second_entry, 0x0020_0000, second_word, second_artifact),
            ],
        );
        let reverse = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, second_word), (0x0010_0000, first_word)],
            &[
                (second_entry, 0x0020_0000, second_word, second_artifact),
                (first_entry, 0x0010_0000, first_word, first_artifact),
            ],
        );

        assert_eq!(forward, reverse);
        assert_eq!(forward.physical_banks.len(), 1);
        assert_eq!(forward.mapped_aot.len(), 2);
    }

    #[test]
    fn mapped_block_program_identity_binds_physical_and_aot_identity_families() {
        let bank = BankId::new(0x61);
        let entry = GuestPc::new(0x0040_0000);
        let word = 0x2402_0001;
        let artifact = ProgramArtifactIdentity::new([0x33; 32]);
        let baseline = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_bank = mapped_evidence_snapshot(
            BankId::new(0x62),
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_physical_address = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, word)],
            &[(entry, 0x0020_0000, word, artifact)],
        );
        let changed_entry = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(GuestPc::new(0x0040_2000), 0x0010_0000, word, artifact)],
        );
        let changed_word = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word + 1)],
            &[(entry, 0x0010_0000, word + 1, artifact)],
        );
        let changed_artifact = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(
                entry,
                0x0010_0000,
                word,
                ProgramArtifactIdentity::new([0x44; 32]),
            )],
        );

        for changed in [
            &changed_bank,
            &changed_physical_address,
            &changed_entry,
            &changed_word,
            &changed_artifact,
        ] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }
        assert_eq!(baseline.mapped_aot[0].entry.pc, entry);
        assert_eq!(
            baseline.mapped_aot[0].instructions,
            vec![InstructionWordIdentity::new(bank, 0x0010_0000)]
        );
        assert_eq!(baseline.mapped_aot[0].expected_words, vec![word]);
    }

    fn cross_catalog_mapped_program(compiled_word: u32) -> BlockProgram {
        let bank = BankId::new(0x63);
        let entry = GuestPc::new(0x8010_0000);
        let mut compilation_catalog = PhysicalCodeCatalog::new();
        compilation_catalog
            .register(PhysicalCodeBank::new(bank, 0x0010_0000, vec![compiled_word]).unwrap())
            .unwrap();
        let block = MappedAotBlock::new(
            &compilation_catalog,
            &RecompContext::new(),
            bank,
            entry,
            &[compiled_word],
            GeneratedBankRunner::new_with_artifact_identity(
                bank,
                first_runner,
                ProgramArtifactIdentity::new([0x63; 32]),
            ),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register_physical_code(
                PhysicalCodeBank::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
            )
            .unwrap();
        program.register_mapped_aot(block).unwrap();
        program
    }

    #[test]
    fn mapped_aot_evidence_binds_future_preflight_expected_words() {
        let valid = cross_catalog_mapped_program(0x2402_0001);
        let stale = cross_catalog_mapped_program(0x2402_0002);
        let valid_snapshot = valid.evidence_snapshot();
        let stale_snapshot = stale.evidence_snapshot();
        assert_eq!(valid_snapshot.physical_banks, stale_snapshot.physical_banks);
        assert_eq!(
            valid_snapshot.mapped_aot[0].instructions,
            stale_snapshot.mapped_aot[0].instructions
        );
        assert_ne!(
            valid_snapshot.mapped_aot[0].expected_words,
            stale_snapshot.mapped_aot[0].expected_words
        );
        assert_ne!(
            valid_snapshot.identity.identity,
            stale_snapshot.identity.identity
        );

        let entry = ExecutionKey::new(BankId::new(0x63), GuestPc::new(0x8010_0000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut valid_ctx = RecompContext::new();
        let mut stale_ctx = RecompContext::new();
        let mut valid_storage = [];
        let mut stale_storage = [];
        assert!(!matches!(
            valid
                .run(
                    entry,
                    budget,
                    &mut valid_ctx,
                    &mut Rdram::new(&mut valid_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(matches!(
            stale
                .run(
                    entry,
                    budget,
                    &mut stale_ctx,
                    &mut Rdram::new(&mut stale_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
    }

    #[test]
    #[should_panic(expected = "stable artifact identity for generated runner")]
    fn block_program_evidence_rejects_unidentified_runner_artifact() {
        let id = BankId::new(0x41);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(id, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(id, first_runner),
            )
            .unwrap();
        let _ = program.evidence_snapshot();
    }

    #[test]
    fn block_program_rejects_holes_before_invoking_runner() {
        let id = BankId::new(12);
        let sparse = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 8), vec![2]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register(sparse, GeneratedBankRunner::new(id, first_runner))
            .unwrap();
        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let hole = ExecutionKey::new(id, GuestPc::new(VA.get() + 4));
        let run = program.run(hole, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
        assert!(matches!(
            run,
            BlockRun {
                exit: BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::UnmappedPc { .. }
                }),
                instructions: 0,
            } if at == hole
        ));
        assert_eq!(
            ctx.r_u32(2),
            0,
            "runner must not execute for a catalog hole"
        );
        assert!(program.copy_execution_destinations().is_empty());

        let unknown = ExecutionKey::new(BankId::new(0xDEAD), VA);
        assert!(matches!(
            program
                .run(
                    unknown,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        assert!(program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn transfers_distinguish_proven_and_runtime_resolved_destinations() {
        let destination = ExecutionKey::new(BankId::new(9), GuestPc::new(0x8000_2000));
        assert_eq!(
            BlockExit::Transfer(destination),
            BlockExit::Transfer(destination)
        );

        let indirect = BlockExit::ResolveTransfer {
            source_bank: BankId::new(1),
            target_pc: GuestPc::new(0x8000_2000),
        };
        assert!(matches!(
            indirect,
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc
            } if source_bank == BankId::new(1) && target_pc == GuestPc::new(0x8000_2000)
        ));
    }

    #[test]
    fn instruction_budget_cannot_split_a_branch_delay_pair() {
        assert_eq!(InstructionBudget::new(0), None);
        let one = InstructionBudget::new(1).unwrap();
        assert_eq!(one.get(), 1);
        assert!(!one.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        let two = InstructionBudget::new(2).unwrap();
        assert_eq!(two.get(), 2);
        assert!(two.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        assert!(!two.can_fit(1, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        assert!(!InstructionBudget::new(u32::MAX)
            .unwrap()
            .can_fit(u32::MAX, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
    }

    #[test]
    fn malformed_destinations_fault_with_bank_and_pc() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(7, &[0])).unwrap();

        let unaligned = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 2));
        let fault = catalog.resolve(unaligned).unwrap_err();
        assert_eq!(fault, CpuFault::instruction_address_error(unaligned));
        assert!(fault.to_string().contains("bank:0000000000000007"));
        assert!(fault.to_string().contains("0x80001002"));

        let unmapped = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 4));
        assert!(matches!(
            catalog.resolve(unmapped).unwrap_err().kind,
            CpuFaultKind::UnmappedPc { .. }
        ));

        let unknown = ExecutionKey::new(BankId::new(8), VA);
        assert_eq!(
            catalog.resolve(unknown).unwrap_err().kind,
            CpuFaultKind::UnknownBank
        );
    }

    #[test]
    fn bank_identity_cannot_be_reused_for_new_bytes() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        assert_eq!(
            catalog.register(bank(1, &[0x2222])),
            Err(BankError::DuplicateId {
                bank: BankId::new(1)
            })
        );
    }

    #[test]
    fn dispatcher_follows_direct_and_resolved_bank_qualified_transfers() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let second = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1010));
        let third = ExecutionKey::new(BankId::new(2), GuestPc::new(0x8000_1010));
        let mut runner = |entry: ExecutionKey, _budget: InstructionBudget| match entry {
            key if key == first => BlockRun::new(BlockExit::Transfer(second), 1),
            key if key == second => BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: second.bank,
                    target_pc: second.pc,
                },
                2,
            ),
            key if key == third => BlockRun::new(BlockExit::Yield(third), 1),
            _ => unreachable!("test runner received an unexpected key"),
        };
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, second.bank);
            assert_eq!(target_pc, second.pc);
            Ok(third)
        };

        assert_eq!(
            dispatch_until_boundary(
                first,
                InstructionBudget::new(6).unwrap(),
                &mut runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::Yield(third),
                instructions: 4,
                blocks: 3,
            }
        );
    }

    #[test]
    fn dispatcher_reports_an_indivisible_unit_in_the_final_one_instruction_slice() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let next = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1004));
        let mut calls = 0;
        let mut runner = |entry, budget: InstructionBudget| {
            calls += 1;
            if entry == next && budget.get() == 1 {
                BlockRun::new(BlockExit::Checkpoint(next), 0)
            } else {
                BlockRun::new(BlockExit::Transfer(next), 1)
            }
        };
        let mut resolver = |_source_bank, _target_pc| unreachable!();

        let final_budget = InstructionBudget::new(1).unwrap();
        assert_eq!(
            dispatch_until_boundary(
                first,
                InstructionBudget::new(2).unwrap(),
                &mut runner,
                &mut resolver,
            ),
            Err(DispatchError::IndivisibleUnitExceedsBudget {
                at: next,
                budget: final_budget,
                required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
            })
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn dispatcher_rejects_non_progress_and_budget_violations() {
        let entry = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut resolver = |_source_bank, _target_pc| unreachable!();
        let mut stalled = |_entry, _budget| BlockRun::new(BlockExit::Transfer(entry), 0);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut stalled, &mut resolver),
            Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: BlockExit::Transfer(entry),
            })
        );

        let checkpoint = BlockExit::Checkpoint(entry);
        let mut stalled_checkpoint = |_entry, _budget| BlockRun::new(checkpoint, 0);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut stalled_checkpoint, &mut resolver),
            Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: checkpoint,
            })
        );

        let mut excessive = |_entry, _budget| BlockRun::new(BlockExit::Yield(entry), 3);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut excessive, &mut resolver),
            Err(DispatchError::RunnerExceededBudget {
                at: entry,
                budget,
                actual: 3,
            })
        );
    }

    #[test]
    fn both_dispatchers_reject_zero_progress_executable_write_exits() {
        let bank_id = BankId::new(0x71);
        let budget = InstructionBudget::new(2).unwrap();
        let resume = ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 12));
        let entries_and_exits = [
            (
                ExecutionKey::new(bank_id, VA),
                BlockExit::ExecutableWrite {
                    source_bank: bank_id,
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 4)),
                BlockExit::ExecutableWriteResolveCall {
                    source_bank: bank_id,
                    target_pc: GuestPc::new(0x8000_2000),
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(
                    ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                )),
            ),
        ];

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank_id, VA, vec![0; 3]).unwrap(),
                GeneratedBankRunner::new(bank_id, zero_progress_executable_write_runner),
            )
            .unwrap();
        let mut ctx = RecompContext::new();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        for (entry, exit) in entries_and_exits {
            let mut runner = move |_entry, _budget| BlockRun::new(exit, 0);
            let mut resolver = |_source_bank, _target_pc| unreachable!();
            assert_eq!(
                dispatch_until_boundary(entry, budget, &mut runner, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
            assert_eq!(
                program.dispatch(entry, budget, &mut ctx, &mut mem, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
        }
    }

    #[test]
    fn executable_write_boundary_preserves_cross_bank_source_lineage() {
        fn changed(_: crate::runtime::GuestWriteEvent) -> crate::runtime::GuestWriteBoundary {
            crate::runtime::GuestWriteBoundary::ExecutableChanged
        }

        let source = BankId::new(0xA);
        let target = ExecutionKey::new(BankId::new(0xC), GuestPc::new(0x8000_4000));
        crate::runtime::set_guest_write_boundary_observer(Some(changed));
        crate::runtime::notify_cpu_instruction_store(0x20, 4);
        assert_eq!(
            finalize_executable_write_exit(source, BlockExit::Transfer(target)),
            BlockExit::ExecutableWrite {
                source_bank: source,
                resume: target,
            }
        );
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn executable_write_special_continuations_escape_dispatch_unresolved() {
        let source = BankId::new(0xA);
        let target = GuestPc::new(0x8000_5000);
        let resume = ExecutionKey::new(source, GuestPc::new(0x8000_1008));
        let call = BlockExit::ExecutableWriteResolveCall {
            source_bank: source,
            target_pc: target,
            resume,
        };
        let mut call_runner = move |_entry, _budget| BlockRun::new(call, 2);
        let mut resolver = |_source_bank, _target_pc| -> Result<ExecutionKey, CpuFault> {
            panic!("executable-write continuation resolved before owner rebuild")
        };
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut call_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: call,
                instructions: 2,
                blocks: 1,
            }
        );

        let fault = CpuFault::instruction_address_error(ExecutionKey::new(
            source,
            GuestPc::new(0x8000_2002),
        ));
        let mut fault_runner =
            move |_entry, _budget| BlockRun::new(BlockExit::ExecutableWriteFault(fault), 3);
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut fault_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::ExecutableWriteFault(fault),
                instructions: 3,
                blocks: 1,
            }
        );
    }

    fn source_attestation_fixture(
        binding: CargoGeneratedRunnerSourceBindingV1,
    ) -> Result<CatalogBlockProgramV1, CatalogBlockProgramErrorV1> {
        let artifact = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            [0x33; 32],
            binding.bank,
            GeneratedAdapterRole::DirectGenerated,
        );
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(binding.bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    binding.bank,
                    first_runner,
                    artifact,
                ),
            )
            .unwrap();
        CatalogBlockProgramV1::new_with_cargo_generated_runner_source_attestation_v2(
            program,
            ExecutionKey::new(binding.bank, VA),
            InstructionBudget::new(2).unwrap(),
            CargoGeneratedProgramSourceAttestationV2 {
                root_adapter_source_sha256: [0x11; 32],
                shard_cargo_source_tree_sha256: [0x22; 32],
                expected_emitter_source_sha256: [0x44; 32],
                externally_measured_emitter_source_sha256: [0x44; 32],
                expected_runtime_source_sha256: generated_runner_runtime_source_receipt_v1()
                    .source_sha256(),
                runtime_source_receipt: generated_runner_runtime_source_receipt_v1(),
                runners: &[binding],
            },
        )
    }

    fn valid_source_binding() -> CargoGeneratedRunnerSourceBindingV1 {
        CargoGeneratedRunnerSourceBindingV1 {
            bank: BankId::new(0xA701),
            generated_runner_source_sha256: [0x33; 32],
            code_words_sha256: Sha256::digest(0u32.to_be_bytes()).into(),
            vram_start: VA,
            vram_end: GuestPc::new(VA.get() + 4),
            composite_subrunner_count: 32,
            adapter_role: GeneratedAdapterRole::DirectGenerated,
        }
    }

    #[test]
    fn generated_runner_source_attestation_binds_composite_source_program_and_role() {
        let catalog = source_attestation_fixture(valid_source_binding()).unwrap();
        let attestation = catalog
            .generated_runner_source_attestation()
            .expect("source-attested constructor retains its projection");
        assert_eq!(
            attestation.schema(),
            GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2
        );
        assert!(attestation.cargo_source_fields_validated());
        assert_eq!(
            attestation.build_receipt(),
            static_execution_build_receipt()
        );

        for malformed in [
            CargoGeneratedRunnerSourceBindingV1 {
                generated_runner_source_sha256: [0x44; 32],
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                adapter_role: GeneratedAdapterRole::EntryContextGate,
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                vram_end: GuestPc::new(VA.get() + 8),
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                code_words_sha256: [0x55; 32],
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                composite_subrunner_count: 0,
                ..valid_source_binding()
            },
        ] {
            assert!(source_attestation_fixture(malformed).is_err());
        }
    }

    #[test]
    fn generic_generated_runner_identity_never_claims_source_attestation() {
        let binding = valid_source_binding();
        let artifact = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            binding.generated_runner_source_sha256,
            binding.bank,
            binding.adapter_role,
        );
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(binding.bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    binding.bank,
                    first_runner,
                    artifact,
                ),
            )
            .unwrap();
        let catalog = CatalogBlockProgramV1::new(
            program,
            ExecutionKey::new(binding.bank, VA),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap();
        assert!(catalog.generated_runner_source_attestation().is_none());
    }
}
