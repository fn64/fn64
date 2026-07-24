//! The typed Rust emitter: a decoded MIPS function -> one Rust `fn` body that
//! operates on the typed [`crate::runtime::RecompContext`] + [`crate::runtime::Rdram`].
//!
//! # Structure (mirrors N64Recomp, emits Rust not C)
//!
//! N64Recomp emits one C function per MIPS function, with `goto L_XXXX` labels
//! at every branch target and the delay-slot instruction duplicated into the
//! taken path (see `recompilation.cpp::process_instruction`). C `goto` has no
//! safe Rust equivalent, so we emit the same control-flow graph as a
//! **labelled dispatch loop**:
//!
//! ```text
//! let mut pc = <entry>;
//! 'run: loop { match pc {
//!     0x…00 => { <ops> ; pc = 0x…04; }          // straight-line fall-through
//!     0x…10 => { <ops> ;                          // a branch site:
//!         <delay-slot ops>                         //   delay slot runs first
//!         if <cond> { pc = 0x…40; } else { pc = 0x…18; }
//!     }
//!     0x…44 => { <ops> ; return; }               // jr $ra
//!     _ => unreachable!(),
//! } }
//! ```
//!
//! Every basic-block boundary (a branch target, or the instruction after a
//! branch) starts a new `match` arm keyed by the instruction's vram. This is
//! the delay-slot rule made explicit: the branch arm executes the delay slot,
//! then assigns `pc`, so the delay slot always runs exactly once regardless of
//! whether the branch is taken. Branch-likely ops put the delay slot only in
//! the taken assignment.
//!
//! The emitted code contains **no `unsafe`, no pointer casts** — only typed
//! `RecompContext`/`Rdram` method calls. That is the property `-rs` exists
//! to guarantee.

use crate::decoder::{decode, Instruction, Reg};
use crate::execution::BankId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// One MIPS function to recompile: its name, its start vram, and its words
/// (big-endian instruction words already read into `u32`s).
pub struct FuncInput<'a> {
    pub name: &'a str,
    pub vram: u32,
    pub words: &'a [u32],
}

/// One immutable executable interval for the arbitrary-PC block lane.
pub struct BankInput<'a> {
    pub name: &'a str,
    pub bank: BankId,
    pub vram: u32,
    pub words: &'a [u32],
}

/// Decoder-level classification for a complete aligned bank scan. This is
/// deliberately weaker than executable ownership: ROM data can decode as a
/// valid instruction, and an `Unknown` word may be unreachable data.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankWordKind {
    Straight,
    ControlTransfer,
    Unknown,
}

/// Compact, bank-local classification table for every aligned word. The
/// table is intentionally independent of function boundaries and can be
/// queried by an arbitrary guest PC without a generated match expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankWordCatalog {
    base: u32,
    words: Vec<BankWordKind>,
}

/// Run-length encoded view of a bank classification.  The decoder still
/// classifies every word, but the dispatcher stores only transitions between
/// classes rather than one host-language arm per PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BankWordRun {
    pub first_word: u32,
    pub len: u32,
    pub kind: BankWordKind,
}

impl BankWordCatalog {
    /// Build the compact transition list used by table-backed dispatch.
    pub fn runs(&self) -> Vec<BankWordRun> {
        let mut runs: Vec<BankWordRun> = Vec::new();
        for (index, &kind) in self.words.iter().enumerate() {
            let index = index as u32;
            if let Some(last) = runs.last_mut() {
                if last.kind == kind && last.first_word + last.len == index {
                    last.len += 1;
                    continue;
                }
            }
            runs.push(BankWordRun {
                first_word: index,
                len: 1,
                kind,
            });
        }
        runs
    }

    /// Resolve a PC through the compact run list.  `None` means unaligned or
    /// outside the admitted bank; it never turns a bounding-range hole into
    /// executable code.
    pub fn kind_at_compact(&self, pc: u32) -> Option<BankWordKind> {
        let offset = pc.checked_sub(self.base)?;
        if !offset.is_multiple_of(4) {
            return None;
        }
        let word = offset / 4;
        self.runs()
            .into_iter()
            .find(|run| word >= run.first_word && word < run.first_word + run.len)
            .map(|run| run.kind)
    }
}

impl BankWordCatalog {
    pub fn new(base: u32, words: &[u32]) -> Self {
        assert!(base.is_multiple_of(4), "catalog base must be aligned");
        Self {
            base,
            words: classify_bank_words(words),
        }
    }

    pub const fn base(&self) -> u32 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn kind_at(&self, pc: u32) -> Option<BankWordKind> {
        let offset = pc.checked_sub(self.base)?;
        if !offset.is_multiple_of(4) {
            return None;
        }
        self.words.get((offset / 4) as usize).copied()
    }
}

/// Classify every aligned word without generating one host-language arm per
/// address. The resulting compact catalog is the input to the universal
/// table-backed dispatcher and keeps code/data/decoder gaps explicit.
pub fn classify_bank_words(words: &[u32]) -> Vec<BankWordKind> {
    words
        .iter()
        .copied()
        .map(decode)
        .map(|instruction| {
            if matches!(instruction, Instruction::Unknown { .. }) {
                BankWordKind::Unknown
            } else if instruction.has_delay_slot() {
                BankWordKind::ControlTransfer
            } else {
                BankWordKind::Straight
            }
        })
        .collect()
}

/// One disjoint proven-code span in a sparse executable bank.
pub struct BankBlockInput<'a> {
    pub vram: u32,
    pub words: &'a [u32],
}

/// Function-boundary-independent bank input containing only admitted code
/// spans. Addresses in holes are never decoded or emitted.
pub struct SparseBankInput<'a> {
    pub name: &'a str,
    pub bank: BankId,
    pub blocks: &'a [BankBlockInput<'a>],
}

/// How an inter-function control transfer (a `JAL`/`J` whose target lands
/// *outside* the current function) is emitted.
///
/// This is the ELF/symbol-table front-end's decision, mirroring N64Recomp's
/// `resolve_jal` (`recompilation.cpp`): a `JAL 0xNNN` whose target vram is a
/// known function symbol becomes a **direct Rust call** to that named `fn`,
/// which is what makes the output a whole-*program* recompile with real
/// cross-function calls rather than a bag of per-function `lookup()` stubs.
///
/// - [`CallTarget::Direct`] — the target is a uniquely-known function; emit a
///   direct `name(ctx, mem)` call (N64Recomp's `JalResolutionResult::Match`).
/// - [`CallTarget::Indirect`] — the target is unknown, ambiguous, or the call
///   is register-indirect; emit a runtime `lookup(addr)(ctx, mem)` dispatch
///   (N64Recomp's `Ambiguous`/`NoMatch`/`LOOKUP_FUNC`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallTarget {
    /// A direct call to the named recompiled function.
    Direct(String),
    /// A runtime-dispatched call (address resolved at run time).
    Indirect,
}

/// Resolves a jump/call target vram to how it should be emitted. Implemented by
/// the symbol-table front-end ([`crate::module::SymbolTable`]); the default
/// [`NullResolver`] resolves nothing (everything indirect), which reproduces
/// the per-function foundation behaviour exactly.
pub trait CallResolver {
    /// Resolve an absolute (already-computed) call target vram. `Some(name)`
    /// yields a direct call; `None` (the default) yields an indirect lookup.
    fn resolve(&self, target_vram: u32) -> CallTarget {
        let _ = target_vram;
        CallTarget::Indirect
    }
}

/// The trivial resolver: never resolves a name, so every `JAL`/`J` target is an
/// indirect `lookup()`. This is what [`emit_function`] uses, preserving the
/// per-function foundation output byte-for-byte.
pub struct NullResolver;
impl CallResolver for NullResolver {}

/// Emit a complete Rust module: the `fn <name>` plus a doc header. The emitted
/// function has signature `fn <name>(ctx: &mut RecompContext, mem: &mut Rdram)`.
///
/// Indirect calls (`jal`/`jalr`/`jr $t` tail calls) are emitted as a call
/// through a supplied dispatch closure; for this foundation slice we emit them
/// as a `lookup(target)(ctx, mem)` call so the shape is proven, matching
/// N64Recomp's `LOOKUP_FUNC`. Straight-line leaf functions (the oracle target)
/// need none of that.
///
/// This is the per-function entry point: every `JAL`/`J` inter-function target
/// is emitted as an indirect `lookup()`. To resolve targets to direct named
/// calls (the whole-program front-end), use [`emit_function_resolved`].
pub fn emit_function(func: &FuncInput) -> String {
    emit_function_resolved(func, &NullResolver)
}

/// Emit one function, resolving inter-function `JAL`/`J` targets through
/// `resolver`. A [`CallTarget::Direct`] target becomes a direct
/// `name(ctx, mem)` call (or, for `J`, a direct tail call `name(ctx, mem);
/// return;`); a [`CallTarget::Indirect`] target keeps the `lookup(addr)`
/// runtime dispatch. This is the codegen half of the symbol-table front-end.
pub fn emit_function_resolved(func: &FuncInput, resolver: &dyn CallResolver) -> String {
    let base = func.vram;
    let words = func.words;

    // 1. Decode every word.
    let instrs: Vec<Instruction> = words.iter().map(|&w| decode(w)).collect();

    // 2. Find all basic-block leaders: the entry, every branch/jump target
    //    that lands inside this function, and every instruction that follows a
    //    branch (fall-through target). This is the CFG-leader set that decides
    //    which vrams get their own `match` arm.
    let mut leaders: BTreeSet<u32> = BTreeSet::new();
    leaders.insert(base);
    let func_end = base + (words.len() as u32) * 4;
    for (i, instr) in instrs.iter().enumerate() {
        let vram = base + (i as u32) * 4;
        if let Some(target) = branch_target(instr, vram) {
            if target >= base && target < func_end {
                leaders.insert(target);
            }
        }
        if instr.has_delay_slot() {
            // The instruction two words down (after the delay slot) is a
            // fall-through leader.
            let after = vram + 8;
            if after >= base && after < func_end {
                leaders.insert(after);
            }
        }
    }

    // A register-indirect `jr` may be a compiler-generated local jump table.
    // Its targets are data, not immediates in the instruction stream, so the
    // static pass above cannot discover them. The dispatcher below accepts any
    // in-function target; give every aligned instruction address an arm when
    // such a transfer exists so that promise is true. OoT's
    // AudioSeq_SequenceChannelProcessScript jumps to 0x800C0898, which is a
    // straight-line instruction and therefore was not otherwise a leader.
    if instrs
        .iter()
        .any(|instr| matches!(instr, Instruction::Jr { rs } if *rs != 31))
    {
        leaders.extend((0..words.len()).map(|i| base + (i as u32) * 4));
    }

    // 3. Emit.
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Recompiled from MIPS function `{}` @ {:#010X} ({} instructions).",
        func.name,
        base,
        words.len()
    );
    let _ = writeln!(out, "// Emitted by fn64-recomp-rs (typed Rust, no unsafe).");
    // A leaf function may not touch memory (or, degenerately, registers); the
    // fixed ABI signature keeps both params, so allow the unused-var lint per
    // function rather than second-guessing which params a body references.
    let _ = writeln!(out, "#[allow(unused_variables)]");
    let _ = writeln!(
        out,
        "pub fn {}(ctx: &mut RecompContext, mem: &mut Rdram) {{",
        func.name
    );
    let _ = writeln!(
        out,
        "    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new({base:#010X}, {:?}));",
        func.name
    );
    let _ = writeln!(out, "    let mut pc: u32 = {:#010X};", base);
    let _ = writeln!(out, "    'run: loop {{ match pc {{");

    let mut i = 0;
    while i < instrs.len() {
        let vram = base + (i as u32) * 4;
        if leaders.contains(&vram) {
            let _ = writeln!(out, "        {:#010X} => {{", vram);
        }

        let instr = instrs[i];
        // Emit the original instruction as a comment (N64Recomp does this too).
        let _ = writeln!(out, "            // {:#010X}: {:?}", vram, instr);

        if instr.has_delay_slot() {
            // The next word is the delay slot. Emit the control transfer,
            // which consumes the delay slot.
            let delay = instrs.get(i + 1).copied();
            let delay_vram = vram + 4;
            emit_control_transfer(
                &mut out, instr, vram, delay, delay_vram, base, func_end, resolver,
            );
            let _ = writeln!(out, "        }}");

            // A branch may target another branch's delay-slot address. The
            // delay instruction is duplicated in N64Recomp's emitted C at the
            // target label (the ordinary execution path also executes it as
            // the parent's delay slot). Give that address its own arm too;
            // the parent's post-delay address is always a leader.
            if leaders.contains(&delay_vram) {
                let _ = writeln!(out, "        {delay_vram:#010X} => {{");
                if let Some(delay_instr) = delay {
                    let _ = writeln!(out, "            // {delay_vram:#010X}: {delay_instr:?}");
                    emit_straight(&mut out, delay_instr, delay_vram, &MemFault::Panic);
                }
                let _ = writeln!(out, "            pc = {:#010X};", delay_vram + 4);
                let _ = writeln!(out, "        }}");
            }

            // Skip the delay-slot word; its normal execution was handled by
            // the transfer, and any independently reachable copy is above.
            i += 2;
            continue;
        } else {
            emit_straight(&mut out, instr, vram, &MemFault::Panic);
        }

        // Fall-through: if the NEXT instruction starts a new block, close this
        // arm with an explicit `pc =` assignment to it. Otherwise let the arm
        // continue straight-lining into the next instruction.
        let next_vram = vram + 4;
        let next_is_leader = leaders.contains(&next_vram) && next_vram < func_end;
        if next_is_leader {
            let _ = writeln!(out, "            pc = {:#010X};", next_vram);
            let _ = writeln!(out, "        }}");
        } else if next_vram >= func_end {
            // This straight-line instruction is the LAST word of the function
            // (e.g. a padding `nop` sitting after a `jr $ra` return, which is
            // common alignment tail). Its arm was opened but has no successor
            // to fall through to, so close it explicitly — otherwise the block
            // dangles into the `_ =>` catch-all and the emitted Rust has an
            // unbalanced brace. Assign `pc` past the function so the loop's
            // `_ =>` arm is the (unreachable) terminator.
            let _ = writeln!(out, "            pc = {:#010X};", next_vram);
            let _ = writeln!(out, "        }}");
        }
        i += 1;
    }

    let _ = writeln!(
        out,
        "        _ => unreachable!(\"jumped to unmapped vram {{:#X}}\", pc),"
    );
    let _ = writeln!(out, "    }} }}");
    let _ = writeln!(out, "}}");
    out
}

/// Emit a bank-qualified runner that accepts every aligned instruction as an
/// entry and executes until the next control transfer.
///
/// Unlike [`emit_function`], this mode does not need or trust function
/// boundaries. Every instruction has a dispatch arm. Straight-line execution
/// stays in the local loop; a branch/jump executes its delay slot and returns a
/// typed [`crate::BlockExit`] to the outer dispatcher. Targets inside this
/// immutable interval are already proven bank-qualified; computed or outside
/// targets remain [`crate::BlockExit::ResolveTransfer`] until the active
/// mapping layer resolves them.
pub fn emit_bank_runner(bank: &BankInput<'_>) -> String {
    emit_bank_runner_with_host_calls(bank, &[])
}

/// Emit a contiguous bank while treating the supplied statically-known call
/// destinations as host ABI boundaries. A matching JAL executes its delay
/// slot, records `$ra`, and returns [`crate::BlockExit::HostCall`] with the
/// exact bank-qualified resume PC instead of asking the executable mapping to
/// pretend host code is a guest bank.
pub fn emit_bank_runner_with_host_calls(bank: &BankInput<'_>, host_calls: &[u32]) -> String {
    let base = bank.vram;
    let byte_len = u32::try_from(bank.words.len())
        .expect("bank instruction count exceeds u32")
        .checked_mul(4)
        .expect("bank byte length exceeds u32");
    let bank_end = base
        .checked_add(byte_len)
        .expect("bank virtual interval exceeds u32");
    let instrs: Vec<Instruction> = bank.words.iter().copied().map(decode).collect();
    let ranges = [(base, bank_end)];
    let domain = ExecutionDomain {
        ranges: &ranges,
        runtime_predicate: None,
        host_calls,
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Bank-qualified MIPS runner `{}`: {} @ {base:#010X} ({} instructions).",
        bank.name,
        bank.bank,
        bank.words.len()
    );
    let _ = writeln!(out, "#[allow(unused_variables, unused_mut, unused_labels)]");
    let _ = writeln!(
        out,
        "pub fn {}(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram) -> BlockRun {{",
        bank.name
    );
    let _ = writeln!(out, "    let mut executed = 0u32;");
    let _ = writeln!(out, "    macro_rules! finish {{");
    let _ = writeln!(
        out,
        "        ($exit:expr) => {{ return BlockRun::new(fn64_recomp_rs::finalize_executable_write_exit(BankId::new({:#018X}), $exit), executed) }};",
        bank.bank.get()
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(
        out,
        "    let expected_bank = BankId::new({:#018X});",
        bank.bank.get()
    );
    let _ = writeln!(out, "    if entry.bank != expected_bank {{");
    let _ = writeln!(
        out,
        "        finish!(BlockExit::Fault(CpuFault {{ at: entry, kind: CpuFaultKind::UnknownBank }}));"
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    if !entry.pc.is_instruction_aligned() {{");
    let _ = writeln!(out, "        executed += 1;");
    let _ = writeln!(
        out,
        "        finish!(BlockExit::Fault(CpuFault::instruction_address_error(entry)));"
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    let mut pc = entry.pc.get();");
    let _ = writeln!(out, "    'run: loop {{ match pc {{");

    for (index, instr) in instrs.iter().copied().enumerate() {
        let vram = base + index as u32 * 4;
        let _ = writeln!(out, "        {vram:#010X} => {{");
        let _ = writeln!(out, "            // {vram:#010X}: {instr:?}");
        emit_bank_cop1_guard(&mut out, instr, vram, vram, false, bank.bank, false);
        if instr.has_delay_slot() {
            let delay = Some(*instrs.get(index + 1).unwrap_or_else(|| {
                panic!(
                    "bank {} ends with control transfer at {vram:#010X} and omits its delay slot",
                    bank.bank
                )
            }));
            let _ = writeln!(
                out,
                "            if executed != 0 && executed + 2 > budget.get() {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new(pc))));"
            );
            let _ = writeln!(out, "            }}");
            let _ = writeln!(out, "            executed += 2;");
            emit_bank_control_transfer(&mut out, instr, vram, delay, vram + 4, bank.bank, &domain);
        } else {
            if !emit_bank_eret(&mut out, instr, bank.bank)
                && !emit_bank_overflow(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_fpu_trap(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_exception(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_address_exception(
                    &mut out, instr, vram, vram, false, bank.bank, false,
                )
            {
                emit_straight(
                    &mut out,
                    instr,
                    vram,
                    &MemFault::Fault {
                        pc: vram,
                        epc: vram,
                        branch_delay: false,
                        retired: "executed",
                    },
                );
            }
            let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
            let _ = writeln!(out, "            executed += 1;");
            let next = vram + 4;
            let _ = writeln!(
                out,
                "            if fn64_recomp_rs::take_executable_write_boundary() {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::ExecutableWrite {{ source_bank: expected_bank, resume: ExecutionKey::new(expected_bank, GuestPc::new({next:#010X})) }});"
            );
            let _ = writeln!(out, "            }}");
            if domain.contains(next) {
                let _ = writeln!(out, "            if executed >= budget.get() {{");
                let _ = writeln!(
                    out,
                    "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new({next:#010X}))));"
                );
                let _ = writeln!(out, "            }}");
                let _ = writeln!(out, "            pc = {next:#010X}; continue 'run;");
            } else {
                emit_resolve_transfer(&mut out, bank.bank, next, 12);
            }
        }
        let _ = writeln!(out, "        }}");
    }

    let _ = writeln!(out, "        _ => finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(
        out,
        "            at: ExecutionKey::new(expected_bank, GuestPc::new(pc)),"
    );
    let _ = writeln!(
        out,
        "            kind: CpuFaultKind::UnmappedPc {{ bank_start: {base:#010X}, bank_end: {bank_end:#010X} }},"
    );
    let _ = writeln!(out, "        }})),");
    let _ = writeln!(out, "    }} }}");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "pub fn register_{}(program: &mut BlockProgram, code: CodeBank) -> Result<(), ProgramError> {{",
        bank.name
    );
    let _ = writeln!(
        out,
        "    program.register(code, GeneratedBankRunner::new(BankId::new({:#018X}), {}))",
        bank.bank.get(),
        bank.name
    );
    let _ = writeln!(out, "}}");
    out
}

/// Emit a bank-qualified arbitrary-PC runner from disjoint proven-code spans.
///
/// Only supplied words receive dispatch arms. A transfer into an interval
/// between spans is unresolved even when that address lies between the bank's
/// lowest and highest admitted addresses. This is the code/data boundary the
/// contiguous [`emit_bank_runner`] input cannot express.
pub fn emit_sparse_bank_runner(bank: &SparseBankInput<'_>) -> String {
    emit_sparse_bank_runner_with_host_calls(bank, &[])
}

/// Emit only the sparse runner function, omitting the compatibility
/// registration helper. Whole-program generators use this form so they can
/// require an artifact identity when constructing every runner.
pub fn emit_sparse_bank_runner_function(bank: &SparseBankInput<'_>) -> String {
    emit_sparse_bank_runner_function_with_host_calls(bank, &[])
}

/// Sparse-bank counterpart of [`emit_bank_runner_with_host_calls`].
pub fn emit_sparse_bank_runner_with_host_calls(
    bank: &SparseBankInput<'_>,
    host_calls: &[u32],
) -> String {
    emit_sparse_bank_runner_inner(bank, host_calls, true)
}

/// [`emit_sparse_bank_runner_function`] with an explicit static host-call
/// inventory.
pub fn emit_sparse_bank_runner_function_with_host_calls(
    bank: &SparseBankInput<'_>,
    host_calls: &[u32],
) -> String {
    emit_sparse_bank_runner_inner(bank, host_calls, false)
}

fn emit_sparse_bank_runner_inner(
    bank: &SparseBankInput<'_>,
    host_calls: &[u32],
    emit_registration: bool,
) -> String {
    assert!(
        !bank.blocks.is_empty(),
        "sparse bank {} contains no proven code spans",
        bank.bank
    );

    let mut blocks: Vec<&BankBlockInput<'_>> = bank.blocks.iter().collect();
    blocks.sort_by_key(|block| block.vram);
    let mut ranges = Vec::with_capacity(blocks.len());
    let mut instrs = BTreeMap::new();
    let mut delay_slots = BTreeSet::new();
    let mut previous_end = None;
    for block in blocks {
        assert!(
            block.vram.is_multiple_of(4),
            "sparse bank {} has unaligned span at {:#010X}",
            bank.bank,
            block.vram
        );
        assert!(
            !block.words.is_empty(),
            "sparse bank {} has empty span at {:#010X}",
            bank.bank,
            block.vram
        );
        let byte_len = u32::try_from(block.words.len())
            .expect("sparse bank instruction count exceeds u32")
            .checked_mul(4)
            .expect("sparse bank byte length exceeds u32");
        let end = block
            .vram
            .checked_add(byte_len)
            .expect("sparse bank virtual span exceeds u32");
        let mut index = 0usize;
        while index < block.words.len() {
            if decode(block.words[index]).has_delay_slot() && index + 1 < block.words.len() {
                delay_slots.insert(block.vram + (index as u32 + 1) * 4);
                index += 2;
            } else {
                index += 1;
            }
        }
        if let Some(previous_end) = previous_end {
            assert!(
                block.vram >= previous_end,
                "sparse bank {} has overlapping spans at {previous_end:#010X} and {:#010X}",
                bank.bank,
                block.vram
            );
        }
        for (index, word) in block.words.iter().copied().enumerate() {
            let pc = block.vram + index as u32 * 4;
            assert!(
                instrs.insert(pc, decode(word)).is_none(),
                "sparse bank {} contains duplicate instruction at {pc:#010X}",
                bank.bank
            );
        }
        if let Some((_, previous_end)) = ranges.last_mut() {
            if *previous_end == block.vram {
                *previous_end = end;
            } else {
                ranges.push((block.vram, end));
            }
        } else {
            ranges.push((block.vram, end));
        }
        previous_end = Some(end);
    }

    let domain = ExecutionDomain {
        ranges: &ranges,
        runtime_predicate: Some("admitted"),
        host_calls,
    };
    let (bank_start, bank_end) = domain.bounds();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Sparse bank-qualified MIPS runner `{}`: {} ({} spans, {} instructions).",
        bank.name,
        bank.bank,
        ranges.len(),
        instrs.len()
    );
    let _ = writeln!(out, "#[allow(unused_variables, unused_mut, unused_labels)]");
    let _ = writeln!(
        out,
        "pub fn {}(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram) -> BlockRun {{",
        bank.name
    );
    let _ = writeln!(out, "    let mut executed = 0u32;");
    let _ = writeln!(out, "    macro_rules! finish {{");
    let _ = writeln!(
        out,
        "        ($exit:expr) => {{ return BlockRun::new(fn64_recomp_rs::finalize_executable_write_exit(BankId::new({:#018X}), $exit), executed) }};",
        bank.bank.get()
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(
        out,
        "    let expected_bank = BankId::new({:#018X});",
        bank.bank.get()
    );
    let _ = writeln!(out, "    if entry.bank != expected_bank {{");
    let _ = writeln!(
        out,
        "        finish!(BlockExit::Fault(CpuFault {{ at: entry, kind: CpuFaultKind::UnknownBank }}));"
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    if !entry.pc.is_instruction_aligned() {{");
    let _ = writeln!(out, "        executed += 1;");
    let _ = writeln!(
        out,
        "        finish!(BlockExit::Fault(CpuFault::instruction_address_error(entry)));"
    );
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    let mut pc = entry.pc.get();");
    let admitted_patterns = ranges
        .iter()
        .map(|(start, end)| {
            let inclusive_end = end - 1;
            format!("{start:#010X}..={inclusive_end:#010X}")
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let _ = writeln!(
        out,
        "    let admitted = |target: u32| matches!(target, {admitted_patterns});"
    );
    let _ = writeln!(out, "    'run: loop {{ match pc {{");

    for (&vram, &instr) in &instrs {
        let _ = writeln!(out, "        {vram:#010X} => {{");
        let _ = writeln!(out, "            // {vram:#010X}: {instr:?}");
        emit_bank_cop1_guard(&mut out, instr, vram, vram, false, bank.bank, false);
        let delay_vram = vram.checked_add(4);
        let delay = delay_vram.and_then(|address| instrs.get(&address).copied());
        if instr.has_delay_slot() && !delay_slots.contains(&vram) && delay.is_none() {
            emit_data_control_word(&mut out, vram);
        } else if instr.has_delay_slot() && !delay_slots.contains(&vram) {
            let delay_vram = delay_vram.expect("sparse bank delay-slot address exceeds u32");
            let _ = writeln!(
                out,
                "            if executed != 0 && executed + 2 > budget.get() {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new(pc))));"
            );
            let _ = writeln!(out, "            }}");
            let _ = writeln!(out, "            executed += 2;");
            emit_bank_control_transfer(
                &mut out, instr, vram, delay, delay_vram, bank.bank, &domain,
            );
        } else if instr.has_delay_slot() {
            emit_data_control_word(&mut out, vram);
        } else {
            if !emit_bank_eret(&mut out, instr, bank.bank)
                && !emit_bank_overflow(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_fpu_trap(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_exception(&mut out, instr, vram, vram, false, bank.bank, false)
                && !emit_bank_address_exception(
                    &mut out, instr, vram, vram, false, bank.bank, false,
                )
            {
                emit_straight(
                    &mut out,
                    instr,
                    vram,
                    &MemFault::Fault {
                        pc: vram,
                        epc: vram,
                        branch_delay: false,
                        retired: "executed",
                    },
                );
            }
            let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
            let _ = writeln!(out, "            executed += 1;");
            let next = vram
                .checked_add(4)
                .expect("sparse bank fallthrough address exceeds u32");
            let _ = writeln!(
                out,
                "            if fn64_recomp_rs::take_executable_write_boundary() {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::ExecutableWrite {{ source_bank: expected_bank, resume: ExecutionKey::new(expected_bank, GuestPc::new({next:#010X})) }});"
            );
            let _ = writeln!(out, "            }}");
            if domain.contains(next) {
                let _ = writeln!(out, "            if executed >= budget.get() {{");
                let _ = writeln!(
                    out,
                    "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new({next:#010X}))));"
                );
                let _ = writeln!(out, "            }}");
                let _ = writeln!(out, "            pc = {next:#010X}; continue 'run;");
            } else {
                emit_resolve_transfer(&mut out, bank.bank, next, 12);
            }
        }
        let _ = writeln!(out, "        }}");
    }

    let _ = writeln!(out, "        _ => finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(
        out,
        "            at: ExecutionKey::new(expected_bank, GuestPc::new(pc)),"
    );
    let _ = writeln!(
        out,
        "            kind: CpuFaultKind::UnmappedPc {{ bank_start: {bank_start:#010X}, bank_end: {bank_end:#010X} }},"
    );
    let _ = writeln!(out, "        }})),");
    let _ = writeln!(out, "    }} }}");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    if emit_registration {
        let _ = writeln!(
            out,
            "pub fn register_{}(program: &mut BlockProgram, code: CodeBank) -> Result<(), ProgramError> {{",
            bank.name
        );
        let _ = writeln!(
            out,
            "    program.register(code, GeneratedBankRunner::new(BankId::new({:#018X}), {}))",
            bank.bank.get(),
            bank.name
        );
        let _ = writeln!(out, "}}");
    }
    out
}

struct ExecutionDomain<'a> {
    ranges: &'a [(u32, u32)],
    runtime_predicate: Option<&'a str>,
    host_calls: &'a [u32],
}

impl ExecutionDomain<'_> {
    fn contains(&self, target: u32) -> bool {
        self.ranges
            .iter()
            .any(|&(start, end)| target >= start && target < end)
    }

    fn bounds(&self) -> (u32, u32) {
        (
            self.ranges.first().expect("nonempty execution domain").0,
            self.ranges.last().expect("nonempty execution domain").1,
        )
    }

    fn runtime_condition(&self, target: &str) -> String {
        if let Some(predicate) = self.runtime_predicate {
            return format!("{predicate}({target})");
        }
        self.ranges
            .iter()
            .map(|(start, end)| format!("({target} >= {start:#010X} && {target} < {end:#010X})"))
            .collect::<Vec<_>>()
            .join(" || ")
    }
}

fn emit_proven_or_resolved_transfer(
    out: &mut String,
    bank: BankId,
    target: u32,
    domain: &ExecutionDomain<'_>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    if domain.contains(target) {
        let _ = writeln!(
            out,
            "{pad}finish!(BlockExit::Transfer(ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({target:#010X}))));",
            bank.get()
        );
    } else {
        emit_resolve_transfer(out, bank, target, indent);
    }
}

fn emit_call_transfer(
    out: &mut String,
    bank: BankId,
    target: u32,
    resume: u32,
    domain: &ExecutionDomain<'_>,
    indent: usize,
) {
    if domain.host_calls.contains(&target) {
        let pad = " ".repeat(indent);
        let _ = writeln!(
            out,
            "{pad}finish!(BlockExit::HostCall {{ vram: GuestPc::new({target:#010X}), resume: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({resume:#010X})) }});",
            bank.get()
        );
    } else if domain.contains(target) {
        emit_proven_or_resolved_transfer(out, bank, target, domain, indent);
    } else {
        emit_resolve_call(out, bank, target, resume, indent);
    }
}

fn emit_resolve_call(out: &mut String, bank: BankId, target: u32, resume: u32, indent: usize) {
    let pad = " ".repeat(indent);
    let _ = writeln!(
        out,
        "{pad}finish!(BlockExit::ResolveCall {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new({target:#010X}), resume: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({resume:#010X})) }});",
        bank.get(),
        bank.get()
    );
}

fn emit_resolve_transfer(out: &mut String, bank: BankId, target: u32, indent: usize) {
    let pad = " ".repeat(indent);
    let _ = writeln!(
        out,
        "{pad}finish!(BlockExit::ResolveTransfer {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new({target:#010X}) }});",
        bank.get()
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_bank_control_transfer(
    out: &mut String,
    instr: Instruction,
    vram: u32,
    delay: Option<Instruction>,
    delay_vram: u32,
    bank: BankId,
    domain: &ExecutionDomain<'_>,
) {
    use Instruction::*;
    let target = branch_target(&instr, vram);
    let fallthrough = delay_vram + 4;
    let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
    let emit_delay = |out: &mut String| {
        if let Some(delay) = delay {
            let _ = writeln!(out, "            // delay: {delay_vram:#010X}: {delay:?}");
            if delay.has_delay_slot() {
                emit_data_control_word(out, delay_vram);
            } else {
                emit_bank_cop1_guard(out, delay, delay_vram, vram, true, bank, true);
                if !emit_bank_overflow(out, delay, delay_vram, vram, true, bank, true)
                    && !emit_bank_fpu_trap(out, delay, delay_vram, vram, true, bank, true)
                    && !emit_bank_exception(out, delay, delay_vram, vram, true, bank, true)
                    && !emit_bank_address_exception(out, delay, delay_vram, vram, true, bank, true)
                {
                    emit_straight(
                        out,
                        delay,
                        delay_vram,
                        &MemFault::Fault {
                            pc: delay_vram,
                            epc: vram,
                            branch_delay: true,
                            retired: "(executed - 2)",
                        },
                    );
                }
                let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
            }
        }
    };

    let self_pause = target == Some(vram)
        && (matches!(instr, J { .. }) || matches!(instr, Beq { rs: 0, rt: 0, .. }));
    if self_pause {
        emit_delay(out);
        let _ = writeln!(
            out,
            "            finish!(BlockExit::Yield(ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({vram:#010X}))));",
            bank.get()
        );
        return;
    }

    match instr {
        Jr { rs } => {
            // The target is read before the delay slot. A delay instruction
            // that writes `rs` must not redirect the already-issued jump.
            let _ = writeln!(out, "            let target = ctx.r_u32({rs});");
            emit_delay(out);
            emit_runtime_transfer(out, bank, domain, None, 12);
        }
        Jalr { rd, rs } => {
            let _ = writeln!(out, "            let target = ctx.r_u32({rs});");
            let _ = writeln!(
                out,
                "            ctx.set_r32({rd}, {fallthrough:#010X}u32 as i32);"
            );
            emit_delay(out);
            emit_runtime_transfer(out, bank, domain, Some(fallthrough), 12);
        }
        J { .. } => {
            emit_delay(out);
            emit_proven_or_resolved_transfer(out, bank, target.expect("J has target"), domain, 12);
        }
        Jal { .. } => {
            let _ = writeln!(
                out,
                "            ctx.set_r32(31, {fallthrough:#010X}u32 as i32);"
            );
            emit_delay(out);
            emit_call_transfer(
                out,
                bank,
                target.expect("JAL has target"),
                fallthrough,
                domain,
                12,
            );
        }
        Bltzal { .. } | Bgezal { .. } | Bltzall { .. } | Bgezall { .. } => {
            let condition = branch_condition(&instr).expect("link branch has condition");
            let target = target.expect("link branch has target");
            let _ = writeln!(out, "            let take = {condition};");
            let _ = writeln!(
                out,
                "            ctx.set_r32(31, {fallthrough:#010X}u32 as i32);"
            );
            if instr.is_branch_likely() {
                let _ = writeln!(out, "            if take {{");
                emit_delay(out);
                emit_call_transfer(out, bank, target, fallthrough, domain, 16);
                let _ = writeln!(out, "            }}");
                let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
                emit_proven_or_resolved_transfer(out, bank, fallthrough, domain, 12);
            } else {
                if domain.host_calls.contains(&target) {
                    let _ = writeln!(out, "            if take {{");
                    emit_delay(out);
                    emit_call_transfer(out, bank, target, fallthrough, domain, 16);
                    let _ = writeln!(out, "            }}");
                    emit_delay(out);
                    emit_proven_or_resolved_transfer(out, bank, fallthrough, domain, 12);
                } else {
                    emit_delay(out);
                    emit_conditional_transfer(out, bank, target, fallthrough, domain);
                }
            }
        }
        _ if instr.is_branch_likely() => {
            let condition = branch_condition(&instr).expect("likely branch has condition");
            let target = target.expect("likely branch has target");
            let _ = writeln!(out, "            if {condition} {{");
            emit_delay(out);
            emit_proven_or_resolved_transfer(out, bank, target, domain, 16);
            let _ = writeln!(out, "            }}");
            let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
            emit_proven_or_resolved_transfer(out, bank, fallthrough, domain, 12);
        }
        _ => {
            let condition = branch_condition(&instr).expect("branch has condition");
            let target = target.expect("branch has target");
            let _ = writeln!(out, "            let take = {condition};");
            emit_delay(out);
            emit_conditional_transfer(out, bank, target, fallthrough, domain);
        }
    }
}

fn emit_runtime_transfer(
    out: &mut String,
    bank: BankId,
    domain: &ExecutionDomain<'_>,
    resume: Option<u32>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    let _ = writeln!(out, "{pad}if ctx.is_thread_return(target) {{");
    let _ = writeln!(out, "{pad}    finish!(BlockExit::ThreadReturn);");
    let _ = writeln!(out, "{pad}}}");
    let _ = writeln!(out, "{pad}if target & 3 != 0 {{");
    let _ = writeln!(
        out,
        "{pad}    let at = ExecutionKey::new(BankId::new({:#018X}), GuestPc::new(target));",
        bank.get()
    );
    let _ = writeln!(out, "{pad}    if executed >= budget.get() {{");
    let _ = writeln!(out, "{pad}        finish!(BlockExit::Checkpoint(at));");
    let _ = writeln!(out, "{pad}    }}");
    let _ = writeln!(out, "{pad}    executed += 1;");
    let _ = writeln!(
        out,
        "{pad}    finish!(BlockExit::Fault(CpuFault::instruction_address_error(at)));"
    );
    let _ = writeln!(out, "{pad}}}");
    let condition = domain.runtime_condition("target");
    let _ = writeln!(out, "{pad}if {condition} {{");
    let _ = writeln!(out, "{pad}    finish!(BlockExit::Transfer(ExecutionKey::new(BankId::new({:#018X}), GuestPc::new(target))));", bank.get());
    let _ = writeln!(out, "{pad}}}");
    if let Some(resume) = resume {
        let _ = writeln!(out, "{pad}finish!(BlockExit::ResolveCall {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new(target), resume: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({resume:#010X})) }});", bank.get(), bank.get());
    } else {
        let _ = writeln!(out, "{pad}finish!(BlockExit::ResolveTransfer {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new(target) }});", bank.get());
    }
}

fn emit_conditional_transfer(
    out: &mut String,
    bank: BankId,
    target: u32,
    fallthrough: u32,
    domain: &ExecutionDomain<'_>,
) {
    let target_expr = transfer_expression(bank, target, domain);
    let fallthrough_expr = transfer_expression(bank, fallthrough, domain);
    let _ = writeln!(
        out,
        "            finish!(if take {{ {target_expr} }} else {{ {fallthrough_expr} }});"
    );
}

fn transfer_expression(bank: BankId, target: u32, domain: &ExecutionDomain<'_>) -> String {
    if domain.contains(target) {
        format!(
            "BlockExit::Transfer(ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({target:#010X})))",
            bank.get()
        )
    } else {
        format!(
            "BlockExit::ResolveTransfer {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new({target:#010X}) }}",
            bank.get()
        )
    }
}

/// The (in-function) target vram of a branch/jump instruction, if it has a
/// statically known one. `jr`/`jalr` (register-indirect) return `None`.
fn branch_target(instr: &Instruction, vram: u32) -> Option<u32> {
    use Instruction::*;
    let rel = |off: i16| vram.wrapping_add(4).wrapping_add((off as i32 as u32) << 2);
    match *instr {
        Beq { off, .. } | Bne { off, .. } | Beql { off, .. } | Bnel { off, .. } => Some(rel(off)),
        Blez { off, .. } | Bgtz { off, .. } | Blezl { off, .. } | Bgtzl { off, .. } => {
            Some(rel(off))
        }
        Bltz { off, .. } | Bgez { off, .. } | Bltzl { off, .. } | Bgezl { off, .. } => {
            Some(rel(off))
        }
        Bltzal { off, .. } | Bgezal { off, .. } | Bltzall { off, .. } | Bgezall { off, .. } => {
            Some(rel(off))
        }
        Bc1t { off } | Bc1f { off } | Bc1tl { off } | Bc1fl { off } => Some(rel(off)),
        Bc0t { off } | Bc0f { off } | Bc0tl { off } | Bc0fl { off } => Some(rel(off)),
        // Absolute jumps: target = (delay_slot_pc & 0xF0000000) | (target << 2).
        J { target } | Jal { target } => Some((vram.wrapping_add(4) & 0xF000_0000) | (target << 2)),
        _ => None,
    }
}

/// Rust condition expression for a conditional branch.
fn branch_condition(instr: &Instruction) -> Option<String> {
    use Instruction::*;
    Some(match *instr {
        Beq { rs, rt, .. } | Beql { rs, rt, .. } => format!("{} == {}", r(rs), r(rt)),
        Bne { rs, rt, .. } | Bnel { rs, rt, .. } => format!("{} != {}", r(rs), r(rt)),
        Blez { rs, .. } | Blezl { rs, .. } => format!("{} <= 0", rs64(rs)),
        Bgtz { rs, .. } | Bgtzl { rs, .. } => format!("{} > 0", rs64(rs)),
        Bltz { rs, .. } | Bltzl { rs, .. } | Bltzal { rs, .. } | Bltzall { rs, .. } => {
            format!("{} < 0", rs64(rs))
        }
        Bgez { rs, .. } | Bgezl { rs, .. } | Bgezal { rs, .. } | Bgezall { rs, .. } => {
            format!("{} >= 0", rs64(rs))
        }
        Bc1t { .. } | Bc1tl { .. } => "ctx.fpu_cond".to_string(),
        Bc1f { .. } | Bc1fl { .. } => "!ctx.fpu_cond".to_string(),
        Bc0t { .. } | Bc0tl { .. } => "ctx.cop0_cond".to_string(),
        Bc0f { .. } | Bc0fl { .. } => "!ctx.cop0_cond".to_string(),
        _ => return None,
    })
}

/// Register read expression as typed Rust. `$zero` folds to a literal `0`.
fn r(idx: Reg) -> String {
    if idx == 0 {
        "0i64 as u64".to_string()
    } else {
        format!("ctx.r({})", idx)
    }
}

/// Register read as `i32` (low word, signed).
fn rs32(idx: Reg) -> String {
    if idx == 0 {
        "0i32".to_string()
    } else {
        format!("ctx.r_s32({})", idx)
    }
}

/// Register read as `u32` (low word, unsigned).
fn ru32(idx: Reg) -> String {
    if idx == 0 {
        "0u32".to_string()
    } else {
        format!("ctx.r_u32({})", idx)
    }
}

/// Register read as `i64` (full register, signed) — the `SIGNED(reg)`/`ToS64`
/// operand for SLT/SLTI and the single-operand branches, which MIPS III
/// evaluates on the whole 64-bit register.
fn rs64(idx: Reg) -> String {
    if idx == 0 {
        "0i64".to_string()
    } else {
        format!("ctx.r_s64({})", idx)
    }
}

/// Register read as `u64` (full register, unsigned) — the `ToU64` operand for
/// SLTU/SLTIU.
fn ru64(idx: Reg) -> String {
    if idx == 0 {
        "0u64".to_string()
    } else {
        format!("ctx.r_u64({})", idx)
    }
}

fn emit_fpu_i32(out: &mut String, fd: Reg, fs: Reg, single: bool, mode: Option<u8>) {
    let value = if single {
        format!("ctx.f_s({}) as f64", fs)
    } else {
        format!("ctx.f_d({})", fs)
    };
    let mode = mode.map_or_else(|| "None".to_string(), |m| format!("Some({})", m));
    let _ = writeln!(
        out,
        "            {{ let v = {}; let r = ctx.fpu_to_i32(v, {}); ctx.set_f_bits({}, r as u32); }}",
        value, mode, fd
    );
}

fn emit_fpu_i64(out: &mut String, fd: Reg, fs: Reg, single: bool, mode: Option<u8>) {
    let value = if single {
        format!("ctx.f_s({}) as f64", fs)
    } else {
        format!("ctx.f_d({})", fs)
    };
    let mode = mode.map_or_else(|| "None".to_string(), |m| format!("Some({})", m));
    let _ = writeln!(
        out,
        "            {{ let v = {}; let r = ctx.fpu_to_i64(v, {}); ctx.set_d_bits({}, r as u64); }}",
        value, mode, fd
    );
}

/// Emit the Status.CU1 check before any COP1-visible effect. A branch checks
/// before its delay slot; a COP1 delay instruction checks after the branch has
/// retired and therefore reports the branch EPC with Cause.BD set.
fn emit_bank_cop1_guard(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) {
    if !instr.requires_cop1() {
        return;
    }
    let _ = writeln!(out, "            if ctx.cop0_status & (1 << 29) == 0 {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::CoprocessorUnusable,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: Some(1),");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
}

fn emit_bank_overflow(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (result, write) = match instr {
        Addi { rt, rs, imm } => (
            format!("({}).checked_add({})", rs32(rs), imm as i32),
            format!("ctx.set_r32({rt}, value);"),
        ),
        Add { rd, rs, rt } => (
            format!("({}).checked_add({})", rs32(rs), rs32(rt)),
            format!("ctx.set_r32({rd}, value);"),
        ),
        Sub { rd, rs, rt } => (
            format!("({}).checked_sub({})", rs32(rs), rs32(rt)),
            format!("ctx.set_r32({rd}, value);"),
        ),
        Daddi { rt, rs, imm } => (
            format!("({}).checked_add({}i64)", rs64(rs), imm as i64),
            format!("ctx.set_r({rt}, value as u64);"),
        ),
        Dadd { rd, rs, rt } => (
            format!("({}).checked_add({})", rs64(rs), rs64(rt)),
            format!("ctx.set_r({rd}, value as u64);"),
        ),
        Dsub { rd, rs, rt } => (
            format!("({}).checked_sub({})", rs64(rs), rs64(rt)),
            format!("ctx.set_r({rd}, value as u64);"),
        ),
        _ => return false,
    };
    let _ = writeln!(out, "            if let Some(value) = {result} {{");
    let _ = writeln!(out, "                {write}");
    let _ = writeln!(out, "            }} else {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::IntegerOverflow,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: None,");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
    true
}

/// Emit the alignment checks that architecturally precede aligned memory
/// operations in the arbitrary-PC lane. The effective address is sampled
/// before any destination, memory, or LLbit state can change.
fn emit_bank_address_exception(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (base, off, alignment, exception) = match instr {
        Lh { base, off, .. } | Lhu { base, off, .. } => (base, off, 2u32, "AddressErrorLoad"),
        Lw { base, off, .. }
        | Lwu { base, off, .. }
        | Ll { base, off, .. }
        | Lwc1 { base, off, .. } => (base, off, 4, "AddressErrorLoad"),
        Ld { base, off, .. } | Lld { base, off, .. } | Ldc1 { base, off, .. } => {
            (base, off, 8, "AddressErrorLoad")
        }
        Sh { base, off, .. } => (base, off, 2, "AddressErrorStore"),
        Sw { base, off, .. } | Sc { base, off, .. } | Swc1 { base, off, .. } => {
            (base, off, 4, "AddressErrorStore")
        }
        Sd { base, off, .. } | Scd { base, off, .. } | Sdc1 { base, off, .. } => {
            (base, off, 8, "AddressErrorStore")
        }
        _ => return false,
    };
    let _ = writeln!(
        out,
        "            let effective_address = Rdram::eff_addr({}, {});",
        r(base),
        off
    );
    let _ = writeln!(
        out,
        "            if effective_address & {:#010X} != 0 {{",
        alignment - 1
    );
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::{exception},"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(
        out,
        "                        bad_vaddr: Some(effective_address),"
    );
    let _ = writeln!(out, "                        coprocessor: None,");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
    emit_straight(
        out,
        instr,
        fault_vram,
        &MemFault::Fault {
            pc: fault_vram,
            epc,
            branch_delay,
            retired: if branch_delay {
                "(executed - 2)"
            } else {
                "executed"
            },
        },
    );
    true
}

/// ERET is a privileged transfer without a delay slot. The arbitrary-PC lane
/// can express it directly as a resolved transfer after applying CP0/LLbit
/// state; whole-function output retains its host-boundary trap because it has
/// no typed transfer return.
fn emit_bank_eret(out: &mut String, instr: Instruction, bank: BankId) -> bool {
    if !matches!(instr, Instruction::Eret) {
        return false;
    }
    let _ = writeln!(out, "            executed += 1;");
    let _ = writeln!(out, "            let target = ctx.exception_return_pc();");
    let _ = writeln!(out, "            ctx.advance_cop0_random(1);");
    let _ = writeln!(out, "            finish!(BlockExit::ResolveTransfer {{");
    let _ = writeln!(
        out,
        "                source_bank: BankId::new({:#018X}),",
        bank.get()
    );
    let _ = writeln!(out, "                target_pc: GuestPc::new(target),");
    let _ = writeln!(out, "            }});");
    true
}

/// Emit a synchronous architectural exception for an arbitrary-PC runner.
/// Whole-function output retains its historical loud panic until it also has
/// an exception-return ABI; the block lane can preserve exact bank/PC/EPC/BD
/// context in its existing typed `BlockExit::Fault` boundary today.
fn emit_bank_exception(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let (condition, exception, code) = match instr {
        Syscall { code } => (None, "Syscall", code),
        Break { code } => (None, "Breakpoint", code),
        Tge { rs, rt, code } => (
            Some(format!("{} >= {}", rs64(rs), rs64(rt))),
            "Trap",
            code as u32,
        ),
        Tgeu { rs, rt, code } => (
            Some(format!("{} >= {}", ru64(rs), ru64(rt))),
            "Trap",
            code as u32,
        ),
        Tlt { rs, rt, code } => (
            Some(format!("{} < {}", rs64(rs), rs64(rt))),
            "Trap",
            code as u32,
        ),
        Tltu { rs, rt, code } => (
            Some(format!("{} < {}", ru64(rs), ru64(rt))),
            "Trap",
            code as u32,
        ),
        Teq { rs, rt, code } => (Some(format!("{} == {}", r(rs), r(rt))), "Trap", code as u32),
        Tne { rs, rt, code } => (Some(format!("{} != {}", r(rs), r(rt))), "Trap", code as u32),
        Tgei { rs, imm } => (
            Some(format!("{} >= {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        Tgeiu { rs, imm } => (
            Some(format!("{} >= {}u64", ru64(rs), imm as i64 as u64)),
            "Trap",
            0,
        ),
        Tlti { rs, imm } => (Some(format!("{} < {}i64", rs64(rs), imm as i64)), "Trap", 0),
        Tltiu { rs, imm } => (
            Some(format!("{} < {}u64", ru64(rs), imm as i64 as u64)),
            "Trap",
            0,
        ),
        Teqi { rs, imm } => (
            Some(format!("{} == {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        Tnei { rs, imm } => (
            Some(format!("{} != {}i64", rs64(rs), imm as i64)),
            "Trap",
            0,
        ),
        _ => return false,
    };
    if let Some(condition) = &condition {
        let _ = writeln!(out, "            if {condition} {{");
    }
    let pad = if condition.is_some() { "    " } else { "" };
    if !already_counted {
        let _ = writeln!(out, "            {pad}executed += 1;");
    }
    let _ = writeln!(out, "            {pad}finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "            {pad}    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "            {pad}    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "            {pad}        exception: CpuException::{exception},"
    );
    let _ = writeln!(
        out,
        "            {pad}        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(
        out,
        "            {pad}        branch_delay: {branch_delay},"
    );
    let _ = writeln!(
        out,
        "            {pad}        instruction_code: {code:#010X},"
    );
    let _ = writeln!(out, "            {pad}        bad_vaddr: None,");
    let _ = writeln!(out, "            {pad}        coprocessor: None,");
    let _ = writeln!(out, "            {pad}    }},");
    let _ = writeln!(out, "            {pad}}}));");
    if condition.is_some() {
        let _ = writeln!(out, "            }}");
    }
    true
}

fn emit_trap(out: &mut String, condition: &str, mnemonic: &str, code: u16) {
    let _ = writeln!(
        out,
        "            if {} {{ panic!(\"MIPS {} trap (code {:#X})\"); }}",
        condition, mnemonic, code
    );
}

fn emit_data_control_word(out: &mut String, vram: u32) {
    let _ = writeln!(
        out,
        "            panic!(\"control transfer at {vram:#010X} has no admitted delay slot or is architecturally UNPREDICTABLE in a delay slot\");"
    );
}

/// Selects the historical panicking memory boundary for whole-function output
/// or the typed out-of-range boundary required by arbitrary-PC runners.
#[derive(Clone, Copy)]
enum MemFault {
    Panic,
    Fault {
        pc: u32,
        epc: u32,
        branch_delay: bool,
        retired: &'static str,
    },
}

impl MemFault {
    fn finish(self) -> String {
        match self {
            Self::Panic => unreachable!("panicking memory accesses do not emit a typed fault"),
            Self::Fault {
                pc,
                epc,
                branch_delay,
                retired,
            } => format!(
                "let __architectural = __fa.is_architectural_exception(); let __kind = __fa.into_cpu_fault_kind(GuestPc::new({epc:#010X}), {branch_delay}); if __architectural {{ if !{branch_delay} {{ executed += 1; }} return BlockRun::new(BlockExit::Fault(CpuFault {{ at: ExecutionKey::new(expected_bank, GuestPc::new({pc:#010X})), kind: __kind }}), executed); }} return BlockRun::new(BlockExit::Fault(CpuFault {{ at: ExecutionKey::new(expected_bank, GuestPc::new({pc:#010X})), kind: __kind }}), {retired});"
            ),
        }
    }

    fn store(self, out: &mut String, unchecked: &str, checked: &str) {
        match self {
            Self::Panic => {
                let _ = writeln!(out, "            {unchecked}");
            }
            Self::Fault { .. } => {
                let _ = writeln!(
                    out,
                    "            if let Err(__fa) = {checked} {{ {} }}",
                    self.finish()
                );
            }
        }
    }

    fn load(
        self,
        out: &mut String,
        unchecked: &str,
        checked: &str,
        consume: impl FnOnce(&str) -> String,
    ) {
        match self {
            Self::Panic => {
                let _ = writeln!(out, "            {}", consume(unchecked));
            }
            Self::Fault { .. } => {
                let _ = writeln!(
                    out,
                    "            let __mv = match {checked} {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; {}",
                    self.finish(),
                    consume("__mv")
                );
            }
        }
    }
}

fn emit_ll(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = mem.load_w(addr); ctx.set_r32({rt}, value); ctx.set_ll_reservation(addr, 4); }}");
        }
        MemFault::Fault { .. } => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = match mem.try_load_w_translated(ctx, addr) {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; ctx.set_r32({rt}, value); ctx.set_ll_reservation(addr, 4); }}", mem_fault.finish());
        }
    }
}

fn emit_lld(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = mem.load_d(addr); ctx.set_r({rt}, value); ctx.set_ll_reservation(addr, 8); }}");
        }
        MemFault::Fault { .. } => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = match mem.try_load_d_translated(ctx, addr) {{ Ok(value) => value, Err(__fa) => {{ {} }} }}; ctx.set_r({rt}, value); ctx.set_ll_reservation(addr, 8); }}", mem_fault.finish());
        }
    }
}

fn emit_sc(out: &mut String, mem_fault: MemFault, rt: Reg, base: Reg, off: i16, double: bool) {
    let addr = format!("Rdram::eff_addr({}, {})", r(base), off);
    let (value, width, store, checked_store) = if double {
        (ru64(rt), 8, "store_d", "try_store_d_translated")
    } else {
        (ru32(rt), 4, "store_w", "try_store_w_translated")
    };
    match mem_fault {
        MemFault::Panic => {
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = {value}; if ctx.take_ll_reservation(addr, {width}) {{ mem.{store}(addr, value); ctx.set_r({rt}, 1); }} else {{ ctx.set_r({rt}, 0); }} }}");
        }
        MemFault::Fault { .. } => {
            let finish = mem_fault.finish();
            let _ = writeln!(out, "            {{ let addr = {addr}; let value = {value}; if let Err(__fa) = Rdram::check_store_translation(ctx, addr) {{ {finish} }} if ctx.take_ll_reservation(addr, {width}) {{ if let Err(__fa) = mem.{checked_store}(ctx, addr, value) {{ {finish} }} ctx.set_r({rt}, 1); }} else {{ ctx.set_r({rt}, 0); }} }}");
        }
    }
}

/// Wrap a `ctx.fpu_*(...)` arithmetic call (which returns `true` on an enabled
/// FP exception) for the whole-function / straight-line lane, which has no
/// exception-return ABI: a trap panics loudly, mirroring the
/// `.expect("MIPS ADD integer overflow")` shape the integer arithmetic uses.
/// The bank lane never reaches here for these ops — it short-circuits with
/// [`emit_bank_fpu_trap`] to produce a typed `BlockExit::Fault` instead.
fn emit_fpu_arith_call(call: &str) -> String {
    format!("if {call} {{ fn64_recomp_rs::trap_unsupported(\"enabled COP1 exception\"); }}")
}

/// If `instr` is a COP1 arithmetic op that can raise an enabled FP exception,
/// emit the bank-lane trap check and return `true` (short-circuiting
/// `emit_straight`, exactly as [`emit_bank_overflow`] does for integer
/// arithmetic). The emitted `ctx.fpu_*` call returns `true` when an enabled
/// exception fired — the FCSR Cause field is written but the destination
/// register and sticky Flags are not — and that turns into a typed ExcCode-15
/// `BlockExit::Fault` carrying the exact EPC/BD.
fn emit_bank_fpu_trap(
    out: &mut String,
    instr: Instruction,
    fault_vram: u32,
    epc: u32,
    branch_delay: bool,
    bank: BankId,
    already_counted: bool,
) -> bool {
    use Instruction::*;
    let call = match instr {
        AddS { fd, fs, ft } => format!("ctx.fpu_add_s({fd}, {fs}, {ft})"),
        SubS { fd, fs, ft } => format!("ctx.fpu_sub_s({fd}, {fs}, {ft})"),
        MulS { fd, fs, ft } => format!("ctx.fpu_mul_s({fd}, {fs}, {ft})"),
        DivS { fd, fs, ft } => format!("ctx.fpu_div_s({fd}, {fs}, {ft})"),
        AbsS { fd, fs } => format!("ctx.fpu_abs_s({fd}, {fs})"),
        NegS { fd, fs } => format!("ctx.fpu_neg_s({fd}, {fs})"),
        SqrtS { fd, fs } => format!("ctx.fpu_sqrt_s({fd}, {fs})"),
        AddD { fd, fs, ft } => format!("ctx.fpu_add_d({fd}, {fs}, {ft})"),
        SubD { fd, fs, ft } => format!("ctx.fpu_sub_d({fd}, {fs}, {ft})"),
        MulD { fd, fs, ft } => format!("ctx.fpu_mul_d({fd}, {fs}, {ft})"),
        DivD { fd, fs, ft } => format!("ctx.fpu_div_d({fd}, {fs}, {ft})"),
        AbsD { fd, fs } => format!("ctx.fpu_abs_d({fd}, {fs})"),
        NegD { fd, fs } => format!("ctx.fpu_neg_d({fd}, {fs})"),
        SqrtD { fd, fs } => format!("ctx.fpu_sqrt_d({fd}, {fs})"),
        _ => return false,
    };
    let _ = writeln!(out, "            if {call} {{");
    if !already_counted {
        let _ = writeln!(out, "                executed += 1;");
    }
    let _ = writeln!(out, "                finish!(BlockExit::Fault(CpuFault {{");
    let _ = writeln!(out, "                    at: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({fault_vram:#010X})),", bank.get());
    let _ = writeln!(out, "                    kind: CpuFaultKind::Exception {{");
    let _ = writeln!(
        out,
        "                        exception: CpuException::FloatingPoint,"
    );
    let _ = writeln!(
        out,
        "                        epc: GuestPc::new({epc:#010X}),"
    );
    let _ = writeln!(out, "                        branch_delay: {branch_delay},");
    let _ = writeln!(out, "                        instruction_code: 0,");
    let _ = writeln!(out, "                        bad_vaddr: None,");
    let _ = writeln!(out, "                        coprocessor: None,");
    let _ = writeln!(out, "                    }},");
    let _ = writeln!(out, "                }}));");
    let _ = writeln!(out, "            }}");
    true
}

/// Emit a straight-line (non-control-transfer) instruction as typed Rust.
fn emit_straight(out: &mut String, instr: Instruction, _vram: u32, mem_fault: &MemFault) {
    use Instruction::*;
    let line = |out: &mut String, s: String| {
        let _ = writeln!(out, "            {}", s);
    };
    let unsupported = |out: &mut String, context: String| {
        line(
            out,
            format!("fn64_recomp_rs::trap_unsupported({context:?});"),
        );
    };
    match instr {
        Nop => line(out, "// nop".to_string()),

        // --- ALU immediate (results are 32-bit, sign-extended into GPR) ---
        Addi { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_add({}).expect(\"MIPS ADDI integer overflow\"));",
                rt,
                rs32(rs),
                imm as i32
            ),
        ),
        Addiu { rt, rs, imm } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_add({}));", rt, rs32(rs), imm as i32),
        ),
        // SLTI/SLTIU compare the full 64-bit register (ToS64/ToU64) against
        // the sign-extended immediate. `imm as i64` sign-extends; for SLTIU
        // the same sign-extended value is reinterpreted as u64.
        Slti { rt, rs, imm } => line(
            out,
            format!("ctx.set_r({}, if {} < {}i64 {{ 1 }} else {{ 0 }});", rt, rs64(rs), imm as i64),
        ),
        Sltiu { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r({}, if {} < {}u64 {{ 1 }} else {{ 0 }});",
                rt,
                ru64(rs),
                imm as i64 as u64
            ),
        ),
        Andi { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} & {:#X});", rt, r(rs), imm as u64))
        }
        Ori { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} | {:#X});", rt, r(rs), imm as u64))
        }
        Xori { rt, rs, imm } => {
            line(out, format!("ctx.set_r({}, {} ^ {:#X});", rt, r(rs), imm as u64))
        }
        Lui { rt, imm } => {
            // Emit the constant as a `u32` literal cast to `i32`: a high LUI
            // (e.g. 0x800F0000) has bit 31 set, so a bare `…i32` literal would
            // overflow the `i32` range (a rustc `overflowing_literals` error).
            line(out, format!("ctx.set_r32({}, {:#X}u32 as i32);", rt, ((imm as u32) << 16)))
        }

        // --- ALU register ---
        Add { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_add({}).expect(\"MIPS ADD integer overflow\"));",
                rd,
                rs32(rs),
                rs32(rt)
            ),
        ),
        Addu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_add({}));", rd, rs32(rs), rs32(rt)),
        ),
        Sub { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r32({}, ({}).checked_sub({}).expect(\"MIPS SUB integer overflow\"));",
                rd,
                rs32(rs),
                rs32(rt)
            ),
        ),
        Subu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r32({}, ({}).wrapping_sub({}));", rd, rs32(rs), rs32(rt)),
        ),
        And { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} & {});", rd, r(rs), r(rt))),
        Or { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} | {});", rd, r(rs), r(rt))),
        Xor { rd, rs, rt } => line(out, format!("ctx.set_r({}, {} ^ {});", rd, r(rs), r(rt))),
        Nor { rd, rs, rt } => line(out, format!("ctx.set_r({}, !({} | {}));", rd, r(rs), r(rt))),
        Slt { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, if {} < {} {{ 1 }} else {{ 0 }});", rd, rs64(rs), rs64(rt)),
        ),
        Sltu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, if {} < {} {{ 1 }} else {{ 0 }});", rd, ru64(rs), ru64(rt)),
        ),

        // --- Shifts (32-bit, sign-extended) ---
        Sll { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, (({}) << {}) as i32);", rd, ru32(rt), sa))
        }
        Srl { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, (({}) >> {}) as i32);", rd, ru32(rt), sa))
        }
        Sra { rd, rt, sa } => {
            line(out, format!("ctx.set_r32({}, {} >> {});", rd, rs32(rt), sa))
        }
        Sllv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, (({}) << ({} & 31)) as i32);", rd, ru32(rt), ru32(rs)),
        ),
        Srlv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, (({}) >> ({} & 31)) as i32);", rd, ru32(rt), ru32(rs)),
        ),
        Srav { rd, rt, rs } => line(
            out,
            format!("ctx.set_r32({}, {} >> ({} & 31));", rd, rs32(rt), ru32(rs)),
        ),

        // --- Mult/Div (write HI/LO). MIPS keeps 32x32 -> 64 in {hi,lo}. ---
        Mult { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as i64) * ({} as i64); ctx.lo = (p as i32) as i64 as u64; ctx.hi = ((p >> 32) as i32) as i64 as u64; }}",
                rs32(rs),
                rs32(rt)
            ),
        ),
        Multu { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as u64) * ({} as u64); ctx.lo = (p as i32) as i64 as u64; ctx.hi = ((p >> 32) as i32) as i64 as u64; }}",
                ru32(rs),
                ru32(rt)
            ),
        ),
        Div { rs, rt } => line(
            out,
            format!("ctx.div_s32({}, {});", rs32(rs), rs32(rt)),
        ),
        Divu { rs, rt } => line(
            out,
            format!("ctx.div_u32({}, {});", ru32(rs), ru32(rt)),
        ),
        Mfhi { rd } => line(out, format!("ctx.set_r({}, ctx.hi);", rd)),
        Mflo { rd } => line(out, format!("ctx.set_r({}, ctx.lo);", rd)),
        Mthi { rs } => line(out, format!("ctx.hi = {};", r(rs))),
        Mtlo { rs } => line(out, format!("ctx.lo = {};", r(rs))),

        // --- Loads ---
        Lw { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),
        Lwu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u32 as u64);"),
        ),
        Ll { rt, base, off } => emit_ll(out, *mem_fault, rt, base, off),
        Lh { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_h(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_h_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value} as i32);"),
        ),
        Lhu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_hu(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_hu_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u64);"),
        ),
        Lb { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_b(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_b_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value} as i32);"),
        ),
        Lbu { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_bu(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_bu_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value} as u64);"),
        ),
        Lwl { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_wl(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_wl_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),
        Lwr { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_wr(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_wr_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r32({rt}, {value});"),
        ),

        // --- Stores ---
        Sw { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_w(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_w_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Sh { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_h(Rdram::eff_addr({}, {}), {} as u16);", r(base), off, ru32(rt)),
            &format!("mem.try_store_h_translated(ctx, Rdram::eff_addr({}, {}), {} as u16)", r(base), off, ru32(rt)),
        ),
        Sb { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_b(Rdram::eff_addr({}, {}), {} as u8);", r(base), off, ru32(rt)),
            &format!("mem.try_store_b_translated(ctx, Rdram::eff_addr({}, {}), {} as u8)", r(base), off, ru32(rt)),
        ),
        Swl { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_wl(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_wl_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Swr { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_wr(Rdram::eff_addr({}, {}), {});", r(base), off, ru32(rt)),
            &format!("mem.try_store_wr_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru32(rt)),
        ),
        Sc { rt, base, off } => emit_sc(out, *mem_fault, rt, base, off, false),

        // --- 64-bit doubleword ALU immediate ---
        // DADDI/DADDIU: full 64-bit add of rs and the sign-extended immediate;
        // the trapping form uses checked arithmetic.
        Daddi { rt, rs, imm } => line(
            out,
            format!(
                "ctx.set_r({}, ({} as i64).checked_add({}i64).expect(\"MIPS DADDI integer overflow\") as u64);",
                rt,
                ru64(rs),
                imm as i64
            ),
        ),
        Daddiu { rt, rs, imm } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}i64 as u64));", rt, ru64(rs), imm as i64),
        ),

        // --- 64-bit doubleword ALU register ---
        Dadd { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r({}, ({}).checked_add({}).expect(\"MIPS DADD integer overflow\") as u64);",
                rd,
                rs64(rs),
                rs64(rt)
            ),
        ),
        Daddu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_add({}));", rd, ru64(rs), ru64(rt)),
        ),
        Dsub { rd, rs, rt } => line(
            out,
            format!(
                "ctx.set_r({}, ({}).checked_sub({}).expect(\"MIPS DSUB integer overflow\") as u64);",
                rd,
                rs64(rs),
                rs64(rt)
            ),
        ),
        Dsubu { rd, rs, rt } => line(
            out,
            format!("ctx.set_r({}, ({}).wrapping_sub({}));", rd, ru64(rs), ru64(rt)),
        ),

        // --- 64-bit doubleword shifts (results stay full 64-bit) ---
        // DSLL/DSRL by sa (0..31); logical shifts operate on ToU64, arithmetic
        // (DSRA) on ToS64 so bit 63 fills.
        Dsll { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) << {});", rd, ru64(rt), sa))
        }
        Dsrl { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) >> {});", rd, ru64(rt), sa))
        }
        Dsra { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, (({}) >> {}) as u64);", rd, rs64(rt), sa))
        }
        // The *32 forms shift by sa + 32 (32..63).
        Dsll32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) << {});", rd, ru64(rt), sa as u32 + 32))
        }
        Dsrl32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, ({}) >> {});", rd, ru64(rt), sa as u32 + 32))
        }
        Dsra32 { rd, rt, sa } => {
            line(out, format!("ctx.set_r({}, (({}) >> {}) as u64);", rd, rs64(rt), sa as u32 + 32))
        }
        // Variable doubleword shifts: shift count is the low 6 bits of rs (0..63).
        Dsllv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, ({}) << ({} & 63));", rd, ru64(rt), ru64(rs)),
        ),
        Dsrlv { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, ({}) >> ({} & 63));", rd, ru64(rt), ru64(rs)),
        ),
        Dsrav { rd, rt, rs } => line(
            out,
            format!("ctx.set_r({}, (({}) >> ({} & 63)) as u64);", rd, rs64(rt), ru64(rs)),
        ),

        // --- 64-bit doubleword mult/div (write HI/LO as full 64-bit) ---
        // DMULT/DMULTU: 64x64 -> 128-bit product; LO = low 64, HI = high 64.
        // Rust's i128/u128 give the full product safely (no unsafe, no
        // pointer tricks) — the typed analogue of N64Recomp's __int128 DMULT.
        Dmult { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as i128) * ({} as i128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }}",
                rs64(rs),
                rs64(rt)
            ),
        ),
        Dmultu { rs, rt } => line(
            out,
            format!(
                "{{ let p = ({} as u128) * ({} as u128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }}",
                ru64(rs),
                ru64(rt)
            ),
        ),
        // DDIV: signed 64-bit, including INT64_MIN / -1. The runtime helper
        // traps loudly on the manual-uncertain zero-divisor case.
        Ddiv { rs, rt } => line(
            out,
            format!("ctx.div_s64({}, {});", rs64(rs), rs64(rt)),
        ),
        Ddivu { rs, rt } => line(
            out,
            format!("ctx.div_u64({}, {});", ru64(rs), ru64(rt)),
        ),

        // --- Doubleword loads ---
        Ld { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_d(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_d_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),
        Lld { rt, base, off } => emit_lld(out, *mem_fault, rt, base, off),
        Ldl { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_dl(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_dl_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),
        Ldr { rt, base, off } => mem_fault.load(
            out,
            &format!("mem.load_dr(ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_dr_translated(ctx, ctx.r({rt}), Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_r({rt}, {value});"),
        ),

        // --- Doubleword stores ---
        Sd { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_d(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_d_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Sdl { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_dl(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_dl_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Sdr { rt, base, off } => mem_fault.store(
            out,
            &format!("mem.store_dr(Rdram::eff_addr({}, {}), {});", r(base), off, ru64(rt)),
            &format!("mem.try_store_dr_translated(ctx, Rdram::eff_addr({}, {}), {})", r(base), off, ru64(rt)),
        ),
        Scd { rt, base, off } => emit_sc(out, *mem_fault, rt, base, off, true),

        // ================================================================
        // COP1 / FPU.
        //
        // All FPU register reads/writes go through typed `RecompContext`
        // accessors (`f_s`/`set_f_s` single, `f_d`/`set_f_d` double,
        // `f_bits`/`d_bits` raw) that resolve the FR=0 even/odd pairing
        // internally — the emitter never open-codes the `f_odd[(N-1)*2]`
        // pointer arithmetic the C oracle uses. Semantics are clean-roomed
        // from the MIPS III / VR4300 reference (and cross-checked against the
        // recomp.h CVT_/TRUNC_ macro definitions, which are the ISA facts).
        // ================================================================

        // --- GPR <-> FPR moves ---
        // MFC1: GPR = sign-extend(FPR single low32). Mirrors `(int32_t)f.u32l`.
        Mfc1 { rt, fs } => {
            line(out, format!("ctx.set_r32({}, ctx.f_bits({}) as i32);", rt, fs))
        }
        // MTC1: FPR single low32 = GPR low32 (raw bits).
        Mtc1 { rt, fs } => {
            line(out, format!("ctx.set_f_bits({}, {});", fs, ru32(rt)))
        }
        // DMFC1: GPR = FPR full 64 bits.
        Dmfc1 { rt, fs } => line(out, format!("ctx.set_r({}, ctx.d_bits({}));", rt, fs)),
        // DMTC1: FPR 64 bits = GPR.
        Dmtc1 { rt, fs } => line(out, format!("ctx.set_d_bits({}, {});", fs, ru64(rt))),
        // CFC1/CTC1: typed FCR0/FCR31 access. OoT reads and rewrites FCR31
        // around conversion sequences, including non-nearest RM values.
        Cfc1 { rt, fs } => line(
            out,
            format!("{{ let v = ctx.read_fcr({}); ctx.set_r32({}, v as i32); }}", fs, rt),
        ),
        Ctc1 { rt, fs } => line(out, format!("ctx.write_fcr({}, {});", fs, ru32(rt))),

        // --- COP1 loads/stores ---
        Lwc1 { ft, base, off } => mem_fault.load(
            out,
            &format!("mem.load_w(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_w_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_f_bits({ft}, {value} as u32);"),
        ),
        Swc1 { ft, base, off } => mem_fault.store(
            out,
            &format!("mem.store_w(Rdram::eff_addr({}, {}), ctx.f_bits({ft}));", r(base), off),
            &format!("mem.try_store_w_translated(ctx, Rdram::eff_addr({}, {}), ctx.f_bits({ft}))", r(base), off),
        ),
        Ldc1 { ft, base, off } => mem_fault.load(
            out,
            &format!("mem.load_d(Rdram::eff_addr({}, {}))", r(base), off),
            &format!("mem.try_load_d_translated(ctx, Rdram::eff_addr({}, {}))", r(base), off),
            |value| format!("ctx.set_d_bits({ft}, {value});"),
        ),
        Sdc1 { ft, base, off } => mem_fault.store(
            out,
            &format!("mem.store_d(Rdram::eff_addr({}, {}), ctx.d_bits({ft}));", r(base), off),
            &format!("mem.try_store_d_translated(ctx, Rdram::eff_addr({}, {}), ctx.d_bits({ft}))", r(base), off),
        ),

        // --- Single-precision arithmetic ---
        // Routed through the IEEE soft-float shim so the op honors FCSR.RM and
        // sets the FCSR Cause/Flag bits (`crate::fpu` via the `ctx.fpu_*`
        // helpers). The raw-host `+`/`*`/`.sqrt()` path (round-to-nearest,
        // no flags) is retired.
        // The `fpu_*` shim helpers return `true` when an ENABLED FP exception
        // trapped (destination left unwritten). The whole-function / straight-
        // line lane has no exception-return ABI yet, so it panics loudly on a
        // trap, mirroring the `.expect("MIPS ADD integer overflow")` shape the
        // integer-arithmetic arms use. The bank lane instead short-circuits this
        // via `emit_bank_fpu_trap`, which turns the same `true` into a typed
        // `BlockExit::Fault(CpuException::FloatingPoint)` (ExcCode 15).
        AddS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_add_s({fd}, {fs}, {ft})"))),
        SubS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sub_s({fd}, {fs}, {ft})"))),
        MulS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_mul_s({fd}, {fs}, {ft})"))),
        DivS { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_div_s({fd}, {fs}, {ft})"))),
        AbsS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_abs_s({fd}, {fs})"))),
        NegS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_neg_s({fd}, {fs})"))),
        SqrtS { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sqrt_s({fd}, {fs})"))),
        // MOV.S is a bit-exact copy (not an arithmetic op): move the raw word.
        MovS { fd, fs } => line(out, format!("ctx.set_f_bits({}, ctx.f_bits({}));", fd, fs)),
        // Conditional moves: pure register copies, never trap (no `if`-guard).
        MovcfS { fd, fs, tf } => line(out, format!("ctx.fpu_movcf_s({fd}, {fs}, {tf});")),
        MovzS { fd, fs, rt } => line(out, format!("ctx.fpu_movz_s({fd}, {fs}, {rt});")),
        MovnS { fd, fs, rt } => line(out, format!("ctx.fpu_movn_s({fd}, {fs}, {rt});")),

        // --- Double-precision arithmetic (routed through the shim). ---
        AddD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_add_d({fd}, {fs}, {ft})"))),
        SubD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sub_d({fd}, {fs}, {ft})"))),
        MulD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_mul_d({fd}, {fs}, {ft})"))),
        DivD { fd, fs, ft } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_div_d({fd}, {fs}, {ft})"))),
        AbsD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_abs_d({fd}, {fs})"))),
        NegD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_neg_d({fd}, {fs})"))),
        SqrtD { fd, fs } => line(out, emit_fpu_arith_call(&format!("ctx.fpu_sqrt_d({fd}, {fs})"))),
        MovD { fd, fs } => line(out, format!("ctx.set_d_bits({}, ctx.d_bits({}));", fd, fs)),
        MovcfD { fd, fs, tf } => line(out, format!("ctx.fpu_movcf_d({fd}, {fs}, {tf});")),
        MovzD { fd, fs, rt } => line(out, format!("ctx.fpu_movz_d({fd}, {fs}, {rt});")),
        MovnD { fd, fs, rt } => line(out, format!("ctx.fpu_movn_d({fd}, {fs}, {rt});")),

        // --- Conversions. Float->float use lossless/rounding `as` casts; the
        //     int destinations write the RAW 32/64 bits of the result into the
        //     FPR (an int-in-FPR is stored as its two's-complement bit pattern,
        //     exactly as the C writes `f.u32l = (int32_t)...`). The int source
        //     of CVT.S.W/CVT.D.W reads the FPR single word AS an i32.

        // int32 (fs single word, read as i32) -> float/double
        CvtSW { fd, fs } => {
            line(out, format!("ctx.set_f_s({}, (ctx.f_bits({}) as i32) as f32);", fd, fs))
        }
        CvtDW { fd, fs } => {
            line(out, format!("ctx.set_f_d({}, (ctx.f_bits({}) as i32) as f64);", fd, fs))
        }
        // int64 (fs 64 bits, read as i64) -> float/double
        CvtSL { fd, fs } => {
            line(out, format!("ctx.set_f_s({}, (ctx.d_bits({}) as i64) as f32);", fd, fs))
        }
        CvtDL { fd, fs } => {
            line(out, format!("ctx.set_f_d({}, (ctx.d_bits({}) as i64) as f64);", fd, fs))
        }
        // float <-> double
        CvtDS { fd, fs } => line(out, format!("ctx.set_f_d({}, ctx.f_s({}) as f64);", fd, fs)),
        CvtSD { fd, fs } => line(out, format!("ctx.set_f_s({}, ctx.f_d({}) as f32);", fd, fs)),

        // float/double -> int32 (round to nearest, ties to even = FCSR default).
        // Written as raw bits of the i32 into the FPR single word.
        CvtWS { fd, fs } => emit_fpu_i32(out, fd, fs, true, None),
        CvtWD { fd, fs } => emit_fpu_i32(out, fd, fs, false, None),
        // float/double -> int64 (round to nearest).
        CvtLS { fd, fs } => emit_fpu_i64(out, fd, fs, true, None),
        CvtLD { fd, fs } => emit_fpu_i64(out, fd, fs, false, None),

        // TRUNC.* -> round toward zero. Rust `f32 as i32` is exactly the C
        // `(int32_t)val` truncation (both saturate/clamp per IEEE-to-int, and
        // OoT's inputs are in range), matching the recomp.h TRUNC_W_S macro.
        TruncWS { fd, fs } => emit_fpu_i32(out, fd, fs, true, Some(1)),
        TruncWD { fd, fs } => emit_fpu_i32(out, fd, fs, false, Some(1)),
        TruncLS { fd, fs } => emit_fpu_i64(out, fd, fs, true, Some(1)),
        TruncLD { fd, fs } => emit_fpu_i64(out, fd, fs, false, Some(1)),
        RoundWS { fd, fs } => emit_fpu_i32(out, fd, fs, true, Some(0)),
        RoundWD { fd, fs } => emit_fpu_i32(out, fd, fs, false, Some(0)),
        RoundLS { fd, fs } => emit_fpu_i64(out, fd, fs, true, Some(0)),
        RoundLD { fd, fs } => emit_fpu_i64(out, fd, fs, false, Some(0)),
        CeilWS { fd, fs } => emit_fpu_i32(out, fd, fs, true, Some(2)),
        CeilWD { fd, fs } => emit_fpu_i32(out, fd, fs, false, Some(2)),
        CeilLS { fd, fs } => emit_fpu_i64(out, fd, fs, true, Some(2)),
        CeilLD { fd, fs } => emit_fpu_i64(out, fd, fs, false, Some(2)),
        FloorWS { fd, fs } => emit_fpu_i32(out, fd, fs, true, Some(3)),
        FloorWD { fd, fs } => emit_fpu_i32(out, fd, fs, false, Some(3)),
        FloorLS { fd, fs } => emit_fpu_i64(out, fd, fs, true, Some(3)),
        FloorLD { fd, fs } => emit_fpu_i64(out, fd, fs, false, Some(3)),
        // (FLOOR/CEIL/ROUND.W.{S,D} are handled by the unified emit_fpu_i32
        // arms above with the mode arg Some(3)/Some(2)/Some(0); the duplicate
        // inline arms from main's driver branch were removed as unreachable on
        // merge -- the emit_fpu_i32 helper and the merged decoder are the
        // superset, and fpu_oracle.rs verifies the emitted behavior matches.)

        // --- FP compares: set the condition flag (FCSR bit 23). ---
        CEqS { fs, ft } => {
            line(out, format!("ctx.fpu_compare_s({}, {}, 2);", fs, ft))
        }
        CLtS { fs, ft } => {
            line(out, format!("ctx.fpu_compare_s({}, {}, 12);", fs, ft))
        }
        CLeS { fs, ft } => {
            line(out, format!("ctx.fpu_compare_s({}, {}, 14);", fs, ft))
        }
        CEqD { fs, ft } => {
            line(out, format!("ctx.fpu_compare_d({}, {}, 2);", fs, ft))
        }
        CLtD { fs, ft } => {
            line(out, format!("ctx.fpu_compare_d({}, {}, 12);", fs, ft))
        }
        CLeD { fs, ft } => {
            line(out, format!("ctx.fpu_compare_d({}, {}, 14);", fs, ft))
        }
        CCondS { fs, ft, cond } => {
            line(out, format!("ctx.fpu_compare_s({}, {}, {});", fs, ft, cond))
        }
        CCondD { fs, ft, cond } => {
            line(out, format!("ctx.fpu_compare_d({}, {}, {});", fs, ft, cond))
        }

        // --- COP0 system control ---
        //
        // The typed context owns the modeled COP0 state. Unsupported
        // registers remain loud; the block lane separately expresses ERET as
        // a typed arbitrary-PC transfer.
        Mfc0 { rt, cop0d } => match cop0d {
            9 => line(out, format!("ctx.set_r32({}, ctx.cop0_count as i32);", rt)),
            11 => line(out, format!("ctx.set_r32({}, ctx.cop0_compare as i32);", rt)),
            1 if matches!(mem_fault, MemFault::Fault { .. }) => line(
                out,
                format!("ctx.set_r32({}, ctx.read_cop0(1) as i32);", rt),
            ),
            0 | 2 | 3 | 4 | 5 | 6 | 8 | 10 | 12 | 13 | 14 | 18 | 19 | 20 | 30 => line(
                out,
                format!("ctx.set_r32({}, ctx.read_cop0({}) as i32);", rt, cop0d),
            ),
            other => unsupported(
                out,
                format!("unsupported mfc0 from COP0 register {other}"),
            ),
        },
        Mtc0 { rt, cop0d } => match cop0d {
            0 | 2 | 3 | 4 | 5 | 6 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30 => line(
                out,
                format!("ctx.write_cop0({}, {});", cop0d, ru32(rt)),
            ),
            other => unsupported(
                out,
                format!("unsupported mtc0 to COP0 register {other}"),
            ),
        },
        Dmfc0 { rt, cop0d }
            if matches!(mem_fault, MemFault::Fault { .. }) && matches!(cop0d, 8 | 10 | 20) =>
        {
            line(
                out,
                format!("ctx.set_r({}, ctx.read_cop0_64({}));", rt, cop0d),
            )
        }
        Dmtc0 { rt, cop0d }
            if matches!(mem_fault, MemFault::Fault { .. }) && matches!(cop0d, 10 | 20) =>
        {
            line(
                out,
                format!("ctx.write_cop0_64({}, {});", cop0d, r(rt)),
            )
        }
        Dmfc0 { cop0d, .. } => unsupported(
            out,
            format!("unsupported dmfc0 from COP0 register {cop0d}"),
        ),
        Dmtc0 { cop0d, .. } => unsupported(
            out,
            format!("unsupported dmtc0 to COP0 register {cop0d}"),
        ),
        Eret => unsupported(
            out,
            "eret executed in recompiled code: exception return is host/libultra territory"
                .to_owned(),
        ),
        Tlbwi => line(out, "ctx.tlbwi_record();".to_string()),
        Tlbwr => match mem_fault {
            MemFault::Panic => unsupported(
                out,
                "tlbwr in whole-function code requires an instruction clock".to_owned(),
            ),
            MemFault::Fault { .. } => line(out, "ctx.tlbwr_record();".to_string()),
        },
        Tlbp => line(out, "ctx.tlbp_probe();".to_string()),
        Tlbr => line(out, "ctx.tlbr_read();".to_string()),

        // --- Cache / sync: no-ops on a coherent host rdram ---
        Cache { op, .. } => {
            line(out, format!("// cache op {:#04X}: no-op (host rdram is coherent)", op))
        }
        Sync => line(out, "// sync: no-op (single-threaded recompiled context)".to_string()),

        // --- COP2: unused coprocessor, loud trap ---
        Mfc2 { .. } | Mtc2 { .. } | Cfc2 { .. } | Ctc2 { .. } | Dmfc2 { .. }
        | Dmtc2 { .. } | Cop2Op { .. } | Lwc2 { .. } | Ldc2 { .. } | Swc2 { .. }
        | Sdc2 { .. } => unsupported(
            out,
            "COP2 access in recompiled code: COP2 is unused on the N64 and not modeled".to_owned(),
        ),

        // --- Traps ---
        Syscall { code } => line(
            out,
            format!("panic!(\"syscall (code {:#X}) executed in recompiled code\");", code),
        ),
        Break { code } => {
            line(out, format!("panic!(\"break (code {:#X}) executed in recompiled code\");", code))
        }
        Tge { rs, rt, code } => emit_trap(out, &format!("{} >= {}", rs64(rs), rs64(rt)), "tge", code),
        Tgeu { rs, rt, code } => emit_trap(out, &format!("{} >= {}", ru64(rs), ru64(rt)), "tgeu", code),
        Tlt { rs, rt, code } => emit_trap(out, &format!("{} < {}", rs64(rs), rs64(rt)), "tlt", code),
        Tltu { rs, rt, code } => emit_trap(out, &format!("{} < {}", ru64(rs), ru64(rt)), "tltu", code),
        Teq { rs, rt, code } => emit_trap(out, &format!("{} == {}", r(rs), r(rt)), "teq", code),
        Tne { rs, rt, code } => emit_trap(out, &format!("{} != {}", r(rs), r(rt)), "tne", code),
        Tgei { rs, imm } => emit_trap(out, &format!("{} >= {}i64", rs64(rs), imm as i64), "tgei", 0),
        Tgeiu { rs, imm } => emit_trap(out, &format!("{} >= {}u64", ru64(rs), imm as i64 as u64), "tgeiu", 0),
        Tlti { rs, imm } => emit_trap(out, &format!("{} < {}i64", rs64(rs), imm as i64), "tlti", 0),
        Tltiu { rs, imm } => emit_trap(out, &format!("{} < {}u64", ru64(rs), imm as i64 as u64), "tltiu", 0),
        Teqi { rs, imm } => emit_trap(out, &format!("{} == {}i64", rs64(rs), imm as i64), "teqi", 0),
        Tnei { rs, imm } => emit_trap(out, &format!("{} != {}i64", rs64(rs), imm as i64), "tnei", 0),

        // Control transfers are never emitted here.
        other => line(out, format!("compile_error!(\"non-straight op reached emit_straight: {:?}\");", other)),
    }
}

/// Emit a control transfer (branch/jump) plus its delay slot. The delay slot
/// runs first (unconditionally for normal branches; only when taken for
/// branch-likely). Then `pc` is assigned to the successor block.
#[allow(clippy::too_many_arguments)]
fn emit_control_transfer(
    out: &mut String,
    instr: Instruction,
    vram: u32,
    delay: Option<Instruction>,
    delay_vram: u32,
    base: u32,
    func_end: u32,
    resolver: &dyn CallResolver,
) {
    use Instruction::*;
    let target = branch_target(&instr, vram);
    let fallthrough = delay_vram + 4;
    // Whether an absolute target lands inside THIS function (so it is a local
    // block, resolved via `pc = ...`), or outside it (an inter-function call
    // the resolver decides how to emit).
    let in_func = |t: u32| t >= base && t < func_end;

    let emit_delay = |out: &mut String| {
        if let Some(d) = delay {
            let _ = writeln!(out, "            // delay: {:#010X}: {:?}", delay_vram, d);
            emit_straight(out, d, delay_vram, &MemFault::Panic);
        }
    };

    // N64Recomp's MIT `recompilation.cpp:489-496` treats an unconditional
    // `j`/pseudo-`b` to its own address as the cooperative idle-loop boundary
    // `pause_self`, rather than burning the host CPU forever. Preserve the
    // delay slot and branch back after each resume so the typed coroutine path
    // has the same repeated-yield semantics.
    let self_pause = target == Some(vram)
        && (matches!(instr, J { .. }) || matches!(instr, Beq { rs: 0, rt: 0, .. }));
    if self_pause {
        let _ = writeln!(out, "            pause_self();");
        emit_delay(out);
        let _ = writeln!(out, "            pc = {vram:#010X}; continue 'run;");
        return;
    }

    match instr {
        Jr { rs } => {
            // `jr $ra` is a return; any other register is an indirect tail call.
            if rs == 31 {
                emit_delay(out);
                let _ = writeln!(out, "            return;");
            } else {
                // N64Recomp lowers a recognized local jump table to `goto`
                // labels inside this function (`recompilation.cpp:462-483`).
                // Preserve the general machine-level form: a computed target
                // in this function resumes our local pc dispatcher; only an
                // address outside the body is an indirect function tail-call.
                let _ = writeln!(out, "            let _target = ctx.r_u32({});", rs);
                // The jump target is captured before the delay slot. This
                // ordering matters when the delay instruction writes `rs`.
                emit_delay(out);
                let _ = writeln!(
                    out,
                    "            if _target >= {base:#010X} && _target < {func_end:#010X} {{ pc = _target; continue 'run; }}"
                );
                let _ = writeln!(out, "            lookup(_target)(ctx, mem); return;");
            }
        }
        J { .. } => {
            emit_delay(out);
            let t = target.unwrap();
            if in_func(t) {
                // Local jump: transfer control to the block at `t`.
                let _ = writeln!(out, "            pc = {:#010X}; continue 'run;", t);
            } else {
                // Inter-function `J` is a tail call: invoke the target, then
                // return (the target's `jr $ra` returns to OUR caller).
                match resolver.resolve(t) {
                    CallTarget::Direct(name) => {
                        let _ = writeln!(
                            out,
                            "            call_host_or_recompiled({:#010X}, {}, ctx, mem); return;",
                            t, name
                        );
                    }
                    CallTarget::Indirect => {
                        let _ = writeln!(out, "            lookup({:#010X})(ctx, mem); return;", t);
                    }
                }
            }
        }
        Jal { .. } => {
            // Link: $ra = address after the delay slot. Emit the address as a
            // `u32` literal + `as i32` so a high (bit-31-set) return address
            // like 0x80002008 is not an out-of-range `i32` literal.
            let _ = writeln!(
                out,
                "            ctx.set_r32(31, {:#010X}u32 as i32);",
                fallthrough
            );
            emit_delay(out);
            let t = target.unwrap();
            match resolver.resolve(t) {
                CallTarget::Direct(name) => {
                    let _ = writeln!(
                        out,
                        "            call_host_or_recompiled({:#010X}, {}, ctx, mem);",
                        t, name
                    );
                }
                CallTarget::Indirect => {
                    let _ = writeln!(out, "            lookup({:#010X})(ctx, mem);", t);
                }
            }
            let _ = writeln!(
                out,
                "            pc = {:#010X}; continue 'run;",
                fallthrough
            );
        }
        Jalr { rd, rs } => {
            // JALR reads the target before writing the link; this matters when
            // rd == rs. Register zero remains zero (rd=0 discards the link).
            let _ = writeln!(out, "            let _target = ctx.r_u32({});", rs);
            let _ = writeln!(
                out,
                "            ctx.set_r32({}, {:#010X}u32 as i32);",
                rd, fallthrough
            );
            emit_delay(out);
            let _ = writeln!(out, "            lookup(_target)(ctx, mem);");
            let _ = writeln!(
                out,
                "            pc = {:#010X}; continue 'run;",
                fallthrough
            );
        }
        Bltzal { .. } | Bgezal { .. } => {
            // Conditional branch-and-link.
            let c = branch_condition(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            let _take = {};", c);
            let _ = writeln!(
                out,
                "            ctx.set_r32(31, {:#010X}u32 as i32);",
                fallthrough
            );
            emit_delay(out);
            let _ = writeln!(
                out,
                "            pc = if _take {{ {:#010X} }} else {{ {:#010X} }}; continue 'run;",
                t, fallthrough
            );
        }
        Bltzall { .. } | Bgezall { .. } => {
            let c = branch_condition(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(
                out,
                "            ctx.set_r32(31, {:#010X}u32 as i32);",
                fallthrough
            );
            let _ = writeln!(out, "            if {} {{", c);
            emit_delay(out);
            let _ = writeln!(out, "                pc = {:#010X};", t);
            let _ = writeln!(out, "            }} else {{");
            let _ = writeln!(out, "                pc = {:#010X};", fallthrough);
            let _ = writeln!(out, "            }} continue 'run;");
        }
        _ if instr.is_branch_likely() => {
            // Branch-likely: delay slot is executed ONLY when the branch is
            // taken. Evaluate condition, then run delay slot inside the taken
            // arm.
            let c = branch_condition(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            if {} {{", c);
            emit_delay(out);
            let _ = writeln!(out, "                pc = {:#010X};", t);
            let _ = writeln!(out, "            }} else {{");
            let _ = writeln!(out, "                pc = {:#010X};", fallthrough);
            let _ = writeln!(out, "            }} continue 'run;");
        }
        _ => {
            // Normal conditional branch: delay slot runs unconditionally.
            let c = branch_condition(&instr).unwrap();
            let t = target.unwrap();
            let _ = writeln!(out, "            let _take = {};", c);
            emit_delay(out);
            let _ = writeln!(
                out,
                "            pc = if _take {{ {:#010X} }} else {{ {:#010X} }}; continue 'run;",
                t, fallthrough
            );
        }
    }
}
