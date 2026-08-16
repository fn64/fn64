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

use fn64_cpu_runtime::decoder::{decode, Instruction, Reg};
use fn64_cpu_runtime::execution::{BankId, BankWordKind};
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

/// One compile-sized slice of a contiguous immutable bank.
///
/// `words` contains every owned entry word followed by at most one lookahead
/// word. The lookahead is never admitted as an entry in this runner; it exists
/// only so a control transfer in the final owned word can execute its
/// architectural delay slot. The adjacent shard owns that word as an ordinary
/// direct entry, preserving the distinction between "executed as a delay
/// instruction" and "fetched as a control instruction".
pub struct DenseBankShardInput<'a> {
    pub name: &'a str,
    pub bank: BankId,
    pub image_vram_start: u32,
    pub image_vram_end: u32,
    /// The complete native artifact interval. Transfers inside it retain this
    /// shard's `BankId`; transfers elsewhere in the logical image must return
    /// through the active-generation resolver because another artifact owns
    /// that PC.
    pub artifact_vram_start: u32,
    pub artifact_vram_end: u32,
    pub shard_vram_start: u32,
    pub words: &'a [u32],
    pub delay_lookahead: Option<u32>,
    /// Fail before executing a word whose live RDRAM bytes no longer match
    /// this immutable artifact. Production whole-image shards enable this;
    /// compile-only generic emitter probes may leave code out of guest memory.
    pub verify_live_words: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenseEmitError {
    EmptyShard,
    UnalignedGeometry,
    ShardOutsideImage,
    AddressOverflow,
    MissingArchitecturalDelayWord { pc: u32 },
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
    let _ = writeln!(out, "// Emitted by fn64-cpu-runtime (typed Rust, no unsafe).");
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
        "    fn64_cpu_runtime::notify_function_entry(fn64_cpu_runtime::TranslatedFunctionIdentity::new({base:#010X}, {:?}));",
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
    let image_end = bank
        .vram
        .checked_add(
            u32::try_from(bank.words.len())
                .expect("bank instruction count exceeds u32")
                .checked_mul(4)
                .expect("bank byte length exceeds u32"),
        )
        .expect("bank virtual interval exceeds u32");
    emit_dense_bank_runner_inner(
        bank.name, bank.bank, bank.vram, image_end, bank.vram, image_end, bank.vram, bank.words,
        None, host_calls, true, false,
    )
    .unwrap_or_else(|error| match error {
        DenseEmitError::MissingArchitecturalDelayWord { pc } => panic!(
            "bank {} ends with control transfer at {pc:#010X} and omits its delay slot",
            bank.bank
        ),
        _ => panic!("invalid dense bank {}: {error:?}", bank.bank),
    })
}

/// Emit one compile-sized dense shard without a registration helper.
/// Every owned aligned word is a valid direct entry. A single non-owned
/// lookahead word may supply the delay instruction for the final owned word.
pub fn emit_dense_bank_shard_runner_function_with_host_calls(
    shard: &DenseBankShardInput<'_>,
    host_calls: &[u32],
) -> Result<String, DenseEmitError> {
    emit_dense_bank_runner_inner(
        shard.name,
        shard.bank,
        shard.image_vram_start,
        shard.image_vram_end,
        shard.artifact_vram_start,
        shard.artifact_vram_end,
        shard.shard_vram_start,
        shard.words,
        shard.delay_lookahead,
        host_calls,
        false,
        shard.verify_live_words,
    )
}

pub fn emit_dense_bank_shard_runner_function(
    shard: &DenseBankShardInput<'_>,
) -> Result<String, DenseEmitError> {
    emit_dense_bank_shard_runner_function_with_host_calls(shard, &[])
}

fn emit_dense_bank_runner_inner(
    name: &str,
    bank: BankId,
    image_start: u32,
    image_end: u32,
    artifact_start: u32,
    artifact_end: u32,
    base: u32,
    words: &[u32],
    delay_lookahead: Option<u32>,
    host_calls: &[u32],
    emit_registration: bool,
    verify_live_words: bool,
) -> Result<String, DenseEmitError> {
    if words.is_empty() {
        return Err(DenseEmitError::EmptyShard);
    }
    if !base.is_multiple_of(4)
        || !image_start.is_multiple_of(4)
        || !image_end.is_multiple_of(4)
        || !artifact_start.is_multiple_of(4)
        || !artifact_end.is_multiple_of(4)
        || image_start >= image_end
        || artifact_start >= artifact_end
    {
        return Err(DenseEmitError::UnalignedGeometry);
    }
    let byte_len = u32::try_from(words.len())
        .map_err(|_| DenseEmitError::AddressOverflow)?
        .checked_mul(4)
        .ok_or(DenseEmitError::AddressOverflow)?;
    let bank_end = base
        .checked_add(byte_len)
        .ok_or(DenseEmitError::AddressOverflow)?;
    if artifact_start < image_start
        || artifact_end > image_end
        || base < artifact_start
        || bank_end > artifact_end
    {
        return Err(DenseEmitError::ShardOutsideImage);
    }
    let instrs: Vec<Instruction> = words.iter().copied().map(decode).collect();
    let ranges = [(artifact_start, artifact_end)];
    let domain = ExecutionDomain {
        ranges: &ranges,
        runtime_predicate: None,
        host_calls,
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Bank-qualified MIPS runner `{}`: {} @ {base:#010X} ({} instructions).",
        name,
        bank,
        words.len()
    );
    let _ = writeln!(out, "#[inline(never)]");
    let _ = writeln!(out, "#[allow(unused_variables, unused_mut, unused_labels)]");
    let _ = writeln!(
        out,
        "pub fn {}(entry: ExecutionKey, budget: InstructionBudget, ctx: &mut RecompContext, mem: &mut Rdram) -> BlockRun {{",
        name
    );
    let _ = writeln!(out, "    let mut executed = 0u32;");
    let _ = writeln!(out, "    use fn64_cpu_runtime::{{generated_support::{{address_error, finish_data_access_error, ArchitecturalFaultSite as FaultSite}}, DataAccessKind}};");
    let _ = writeln!(out, "    macro_rules! finish {{");
    let _ = writeln!(
        out,
        "        ($exit:expr) => {{ return BlockRun::new(fn64_cpu_runtime::finalize_executable_write_exit(BankId::new({:#018X}), $exit), executed) }};",
        bank.get()
    );
    let _ = writeln!(out, "    }}");
    if verify_live_words {
        let _ = writeln!(out, "    macro_rules! verify_live_word {{");
        let _ = writeln!(
            out,
            "        ($bank:expr, $mem:expr, $pc:expr, $expected:expr, $fault_at:expr) => {{ if let Err(miss) = fn64_cpu_runtime::verify_precompiled_instruction_word($bank, GuestPc::new($pc), $expected, $mem) {{ finish!(BlockExit::ImageChanged {{ at: ExecutionKey::new($bank, GuestPc::new($fault_at)), miss }}); }} }};"
        );
        let _ = writeln!(out, "    }}");
    }
    let _ = writeln!(
        out,
        "    let expected_bank = BankId::new({:#018X});",
        bank.get()
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
    if verify_live_words {
        let _ = write!(out, "    const EXPECTED_WORDS: &[u32] = &[");
        for word in words {
            let _ = write!(out, "{word:#010X},");
        }
        let _ = writeln!(out, "];");
    }
    let _ = writeln!(out, "    let mut pc = entry.pc.get();");
    let _ = writeln!(out, "    'run: loop {{");
    if verify_live_words {
        let _ = writeln!(
            out,
            "        if pc >= {base:#010X} && pc < {bank_end:#010X} {{"
        );
        let _ = writeln!(
            out,
            "            let expected_word = EXPECTED_WORDS[((pc - {base:#010X}) / 4) as usize];"
        );
        let _ = writeln!(
            out,
            "            verify_live_word!(expected_bank, mem, pc, expected_word, pc);"
        );
        let _ = writeln!(out, "        }}");
    }
    let _ = writeln!(out, "        match pc {{");

    for (index, instr) in instrs.iter().copied().enumerate() {
        let vram = base + index as u32 * 4;
        let _ = writeln!(out, "        {vram:#010X} => {{");
        let _ = writeln!(out, "            // {vram:#010X}: {instr:?}");
        emit_bank_cop1_guard(&mut out, instr, vram, vram, false, bank, false);
        if instr.has_delay_slot() {
            let delay_word = words
                .get(index + 1)
                .copied()
                .or(delay_lookahead)
                .ok_or(DenseEmitError::MissingArchitecturalDelayWord { pc: vram })?;
            let delay = decode(delay_word);
            if verify_live_words && instr.is_branch_likely() {
                let condition = branch_condition(&instr).expect("likely branch has condition");
                let _ = writeln!(out, "            if {condition} {{");
                let _ = writeln!(
                    out,
                    "                verify_live_word!(expected_bank, mem, {:#010X}, {delay_word:#010X}, {vram:#010X});",
                    vram + 4,
                );
                let _ = writeln!(out, "            }}");
            } else if verify_live_words {
                let _ = writeln!(
                    out,
                    "            verify_live_word!(expected_bank, mem, {:#010X}, {delay_word:#010X}, {vram:#010X});",
                    vram + 4,
                );
            }
            let _ = writeln!(
                out,
                "            if !budget.can_fit(executed, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS) {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new(pc))));"
            );
            let _ = writeln!(out, "            }}");
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank, false);
            let _ = writeln!(out, "            executed += 2;");
            emit_bank_control_transfer(&mut out, instr, vram, Some(delay), vram + 4, bank, &domain);
        } else {
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank, false);
            if !emit_bank_eret(&mut out, instr, bank)
                && !emit_bank_overflow(&mut out, instr, vram, vram, false, bank, false)
                && !emit_bank_fpu_trap(&mut out, instr, vram, vram, false, bank, false)
                && !emit_bank_exception(&mut out, instr, vram, vram, false, bank, false)
                && !emit_bank_address_exception(&mut out, instr, vram, vram, false, bank, false)
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
            let may_continue_locally = next < bank_end;
            let _ = writeln!(
                out,
                "            if let Some(exit) = fn64_cpu_runtime::post_straight_instruction_exit(expected_bank, GuestPc::new({next:#010X}), executed, budget, {may_continue_locally}) {{ finish!(exit); }}"
            );
            if may_continue_locally {
                let _ = writeln!(out, "            pc = {next:#010X}; continue 'run;");
            } else if domain.contains(next) {
                emit_proven_or_resolved_transfer(&mut out, bank, next, &domain, 12);
            } else {
                emit_resolve_transfer(&mut out, bank, next, 12);
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
    let _ = writeln!(out, "        }}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}");
    if emit_registration {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "pub fn register_{}(program: &mut BlockProgram, code: CodeBank) -> Result<(), ProgramError> {{",
            name
        );
        let _ = writeln!(
            out,
            "    program.register(code, GeneratedBankRunner::new(BankId::new({:#018X}), {}))",
            bank.get(),
            name
        );
        let _ = writeln!(out, "}}");
    }
    Ok(out)
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
    let _ = writeln!(out, "    use fn64_cpu_runtime::{{generated_support::{{address_error, finish_data_access_error, ArchitecturalFaultSite as FaultSite}}, DataAccessKind}};");
    let _ = writeln!(out, "    macro_rules! finish {{");
    let _ = writeln!(
        out,
        "        ($exit:expr) => {{ return BlockRun::new(fn64_cpu_runtime::finalize_executable_write_exit(BankId::new({:#018X}), $exit), executed) }};",
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
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank.bank, false);
            emit_data_control_word(&mut out, vram);
        } else if instr.has_delay_slot() && !delay_slots.contains(&vram) {
            let delay_vram = delay_vram.expect("sparse bank delay-slot address exceeds u32");
            let _ = writeln!(
                out,
                "            if !budget.can_fit(executed, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS) {{"
            );
            let _ = writeln!(
                out,
                "                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new(pc))));"
            );
            let _ = writeln!(out, "            }}");
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank.bank, false);
            let _ = writeln!(out, "            executed += 2;");
            emit_bank_control_transfer(
                &mut out, instr, vram, delay, delay_vram, bank.bank, &domain,
            );
        } else if instr.has_delay_slot() {
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank.bank, false);
            emit_data_control_word(&mut out, vram);
        } else {
            emit_bank_cop0_guard(&mut out, instr, vram, vram, false, bank.bank, false);
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
            let may_continue_locally = domain.contains(next);
            let _ = writeln!(
                out,
                "            if let Some(exit) = fn64_cpu_runtime::post_straight_instruction_exit(expected_bank, GuestPc::new({next:#010X}), executed, budget, {may_continue_locally}) {{ finish!(exit); }}"
            );
            if may_continue_locally {
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
                emit_bank_cop0_guard(out, delay, delay_vram, vram, true, bank, true);
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
            emit_runtime_transfer(out, bank, domain, vram, rs, None, 12);
        }
        Jalr { rd, rs } => {
            let _ = writeln!(out, "            let target = ctx.r_u32({rs});");
            let _ = writeln!(
                out,
                "            ctx.set_r32({rd}, {fallthrough:#010X}u32 as i32);"
            );
            emit_delay(out);
            emit_runtime_transfer(out, bank, domain, vram, rs, Some(fallthrough), 12);
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
    source_pc: u32,
    source_register: u8,
    resume: Option<u32>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    let link_pc = resume.map_or_else(|| "None".to_string(), |pc| format!("Some({pc:#010X})"));
    let _ = writeln!(
        out,
        "{pad}ctx.record_indirect_transfer({:#018X}, {source_pc:#010X}, {source_register}, target, {link_pc});",
        bank.get()
    );
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
    if let Some(resume) = resume {
        let _ = writeln!(out, "{pad}finish!(BlockExit::ResolveCall {{ source_bank: BankId::new({:#018X}), target_pc: GuestPc::new(target), resume: ExecutionKey::new(BankId::new({:#018X}), GuestPc::new({resume:#010X})) }});", bank.get(), bank.get());
    } else {
        let condition = domain.runtime_condition("target");
        let _ = writeln!(out, "{pad}if {condition} {{");
        let _ = writeln!(out, "{pad}    finish!(BlockExit::Transfer(ExecutionKey::new(BankId::new({:#018X}), GuestPc::new(target))));", bank.get());
        let _ = writeln!(out, "{pad}}}");
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
// Straight-line instruction emission lives in a child module; the glob
// pair below keeps every existing unqualified call site on both sides
// working without touching them.
mod ops;
use ops::*;

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
