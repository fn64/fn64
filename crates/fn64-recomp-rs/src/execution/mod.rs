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
use crate::fetch::{
    admit_mapped_unit, run_admitted_mapped_unit, run_admitted_mapped_unit_with_memory_port,
};
use crate::fetch::{
    MappedAotBlock, MappedAotEvidenceSnapshot, PhysicalCodeBank, PhysicalCodeBankEvidenceSnapshot,
    PhysicalCodeCatalog, PhysicalCodeError,
};
use crate::generation::{BackedPrecompiledGenerationCatalogV1, GenerationCatalogError};
use crate::runtime::{HostFunctionCatalogV1, Rdram, RecompContext};
#[cfg(feature = "dev-interpreter")]
use crate::semantic::{MemoryPort, NoMmio};
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
    /// Byte offset from `va_start` where live memory first differs from the
    /// compiled image, when the caller can determine it.
    ///
    /// Two digests prove the image is not the compiled one but say nothing
    /// about WHY, and the two explanations need opposite fixes: a low offset
    /// inside a data field means the game wrote to mutable state the digest
    /// covers, while a divergence spread from offset zero means this is a
    /// different overlay entirely. On WM2000 that distinction is what blocks
    /// the route past its overlay entry, and no diagnostic recorded it.
    ///
    /// `None` when the comparison is digest-only (the caller never held both
    /// byte images), which keeps this additive for existing callers.
    pub first_diff_offset: Option<u32>,
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
        )?;
        if let Some(offset) = self.first_diff_offset {
            write!(
                formatter,
                "; first differing byte at +{offset:#x} (va {:#010X})",
                self.va_start.get().saturating_add(offset),
            )?;
        }
        Ok(())
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
            // Digest-only comparison: this seam holds the live bytes and the
            // expected DIGEST, never the expected bytes, so there is nothing
            // to diff against.
            first_diff_offset: None,
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
        // One word, and it differs, so the first differing byte is within it.
        first_diff_offset: Some(
            expected_word
                .to_be_bytes()
                .iter()
                .zip(actual_word.to_be_bytes().iter())
                .position(|(expected, actual)| expected != actual)
                .expect("words differ, so some byte differs") as u32,
        ),
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

mod catalog_v1;
mod program;
pub use catalog_v1::*;
pub use program::*;

#[cfg(test)]
mod tests;
