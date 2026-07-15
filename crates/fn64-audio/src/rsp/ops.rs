//! The VU op dispatch table + operand-shape descriptors — a STUB the
//! ops-phase agents fill in. This module names every CP2 compute op the
//! RSPRecomp codegen emits, records its operand shape (matching
//! `rsp_recomp.cpp`'s `vector_operands` map), and provides the function-shape
//! skeleton each op body plugs into. The bodies themselves (the actual DSP
//! math for all 47 ops) are implemented in the ops phase against the
//! `VuState`/`Accumulator`/`Flags` API in [`super::vu`].
//!
//! ## What "the generated call" looks like (the contract these ops satisfy)
//!
//! Per `rsp_recomp.cpp` (lines 33-40 of RSP-VU-ISA.md, and the codegen at
//! lines 316-371), each vector instruction becomes a call on an `rsp` object:
//! ```text
//! rsp.VMULF<e>(rsp.vpu.r[vd], rsp.vpu.r[vs], rsp.vpu.r[vt]);  // Vd,Vs,Vt
//! rsp.VMOV<e>(rsp.vpu.r[vd], de, rsp.vpu.r[vt]);              // Vd,De,Vt
//! rsp.VSAR<e>(rsp.vpu.r[vd], rsp.vpu.r[vs]);                  // Vd,Vs
//! rsp.VRNDN<e>(rsp.vpu.r[vd], vs /*index*/, rsp.vpu.r[vt]);   // Vd,VsIndex,Vt
//! rsp.VMACQ(rsp.vpu.r[vd]);                                   // Vd only
//! rsp.VNOP();                                                 // none
//! ```
//! `e` is a compile-time 0..15 element modifier; `de` is `de & 7`. Every op
//! that consumes `Vt` applies `e` through [`super::vu::element_select`]; the
//! scalar ops (VMOV/VRCP*/VRSQ*) use `e` to pick one source lane and `de` for
//! the destination lane.
//!
//! ## The op-body shape the ops phase implements
//!
//! An op is a function of `&mut VuState` plus its operand register references
//! and the const parameters (`e`, `de`, or the `vs` index). The dispatcher
//! resolves an opcode + fields into the right call. Because the ops write
//! their result into `vd` (a `&mut Vec8`) and mutate `state.acc`/`state.flags`
//! in place, they return `()`. This module currently provides:
//! - [`VuOp`] — the enumeration of all emitted compute ops,
//! - [`OperandShape`] — the 3-operand shape descriptor (mirrors
//!   `rsp_recomp.cpp`'s `RspOperand`),
//! - [`operand_shape`] — the op → shape lookup (the codegen's
//!   `vector_operands` map),
//! - [`dispatch`] — a skeleton dispatcher that routes to per-op stubs.
//!
//! The per-op bodies are `todo!()`-free stubs today (they trap via
//! [`OpStatus::Unimplemented`]); the ops phase replaces each stub with real
//! math without changing this module's public shape.

use super::vu::{Vec8, VuState};

/// Every CP2 compute op the RSPRecomp codegen emits (the `Vd,Vs,Vt` /
/// `Vd,De,Vt` / `Vd,VsIndex,Vt` / `Vd,Vs` / `Vd` / none groups in
/// `rsp_recomp.cpp`'s `vector_operands`). 47 ops: the multiply families, the
/// add/sub-with-carry family, VABS, the logical ops, the compares + VMRG, the
/// clip ops, VSAR, VMOV, the VRND pair, and the reciprocal/inverse-sqrt
/// family, plus VNOP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VuOp {
    // --- Multiply: set accumulator (§6.1) ---
    Vmulf,
    Vmulu,
    Vmulq,
    Vmudh,
    Vmudm,
    Vmudn,
    Vmudl,
    // --- Multiply: accumulate (§6.2) ---
    Vmacf,
    Vmacu,
    Vmacq,
    Vmadh,
    Vmadm,
    Vmadn,
    Vmadl,
    // --- Add / subtract with carry (§6.3) ---
    Vadd,
    Vaddc,
    Vsub,
    Vsubc,
    // --- VABS (§6.4) ---
    Vabs,
    // --- Logical (§6.5) ---
    Vand,
    Vnand,
    Vor,
    Vnor,
    Vxor,
    Vnxor,
    // --- Compares + merge (§6.6) ---
    Vlt,
    Veq,
    Vne,
    Vge,
    Vmrg,
    // --- Clip (§6.7) ---
    Vch,
    Vcl,
    Vcr,
    // --- Accumulator read (§6.9) ---
    Vsar,
    // --- Move (§6.10) ---
    Vmov,
    // --- Round (§6.11) ---
    Vrndn,
    Vrndp,
    // --- Reciprocal / inverse-sqrt (§6.12) ---
    Vrcp,
    Vrcpl,
    Vrcph,
    Vrsq,
    Vrsql,
    Vrsqh,
    // --- No-op (§6.5) ---
    Vnop,
}

/// The 3-operand shape of a VU op, mirroring `rsp_recomp.cpp`'s `RspOperand`
/// tuple in `vector_operands`. Names which of `Vd`/`Vs`/`Vt`/`De`/`VsIndex`
/// each of the three call operands is, so the dispatcher passes the right
/// thing. `element_ignored` marks the two ops (`Vmacq`, `Vnop`) the codegen
/// calls without the `<e>` template argument (line 178-180, 367-368).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperandShape {
    /// The three operand roles, in call order. `None` slots are absent.
    pub operands: [OperandRole; 3],
    /// True for `Vmacq`/`Vnop`, which ignore the element field.
    pub element_ignored: bool,
}

/// One operand role in a generated VU call (a subset of `rsp_recomp.cpp`'s
/// `RspOperand` relevant to the CP2 compute ops — the load/store and scalar
/// MIPS roles live elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandRole {
    /// Absent operand slot.
    None,
    /// `rsp.vpu.r[vd]` — the destination vector register.
    Vd,
    /// `rsp.vpu.r[vs]` — a source vector register.
    Vs,
    /// `rsp.vpu.r[vt]` — the element-selected source vector register.
    Vt,
    /// `de & 7` — the destination-element index (VMOV/VRCP*/VRSQ*).
    De,
    /// A bare `vs` index repurposed as a 0/1 immediate (VRNDN/VRNDP).
    VsIndex,
}

/// The op → operand-shape lookup, i.e. the CP2-compute subset of
/// `rsp_recomp.cpp`'s `vector_operands` map (lines 62-114).
pub fn operand_shape(op: VuOp) -> OperandShape {
    use OperandRole::{De, None, Vd, Vs, VsIndex, Vt};
    // Vd, Vs, Vt group
    let vd_vs_vt = OperandShape {
        operands: [Vd, Vs, Vt],
        element_ignored: false,
    };
    // Vd, De, Vt group
    let vd_de_vt = OperandShape {
        operands: [Vd, De, Vt],
        element_ignored: false,
    };
    match op {
        // Vd, Vs, Vt
        VuOp::Vabs
        | VuOp::Vadd
        | VuOp::Vaddc
        | VuOp::Vand
        | VuOp::Vch
        | VuOp::Vcl
        | VuOp::Vcr
        | VuOp::Veq
        | VuOp::Vge
        | VuOp::Vlt
        | VuOp::Vmacf
        | VuOp::Vmacu
        | VuOp::Vmadh
        | VuOp::Vmadl
        | VuOp::Vmadm
        | VuOp::Vmadn
        | VuOp::Vmrg
        | VuOp::Vmudh
        | VuOp::Vmudl
        | VuOp::Vmudm
        | VuOp::Vmudn
        | VuOp::Vne
        | VuOp::Vnor
        | VuOp::Vnxor
        | VuOp::Vor
        | VuOp::Vsub
        | VuOp::Vsubc
        | VuOp::Vmulf
        | VuOp::Vmulu
        | VuOp::Vmulq
        | VuOp::Vnand
        | VuOp::Vxor => vd_vs_vt,
        // Vd, Vs (Vt = None)
        VuOp::Vsar => OperandShape {
            operands: [Vd, Vs, None],
            element_ignored: false,
        },
        // Vd only; e ignored
        VuOp::Vmacq => OperandShape {
            operands: [Vd, None, None],
            element_ignored: true,
        },
        // Vd, VsIndex, Vt
        VuOp::Vrndn | VuOp::Vrndp => OperandShape {
            operands: [Vd, VsIndex, Vt],
            element_ignored: false,
        },
        // Vd, De, Vt
        VuOp::Vmov | VuOp::Vrcp | VuOp::Vrcpl | VuOp::Vrcph | VuOp::Vrsq | VuOp::Vrsql
        | VuOp::Vrsqh => vd_de_vt,
        // Nop
        VuOp::Vnop => OperandShape {
            operands: [None, None, None],
            element_ignored: true,
        },
    }
}

/// The resolved operands + const fields for a single VU op invocation. The
/// dispatcher builds one of these from the decoded instruction and hands it to
/// the op body. Register operands are passed by index into `state.regs.r`
/// (the op reads/writes `state.regs.r[vd]` etc.), which keeps the borrow
/// checker happy when an op touches `vd`, `vs`, `vt`, `acc`, and `flags` at
/// once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OpInvocation {
    /// Destination vector register index (`Vd`), if the op has one.
    pub vd: usize,
    /// Source vector register index (`Vs`), if the op has one.
    pub vs: usize,
    /// Element-selected source vector register index (`Vt`), if present.
    pub vt: usize,
    /// The element modifier `e` (0..15). Ignored for `Vmacq`/`Vnop`.
    pub e: usize,
    /// The destination-element index (`de & 7`) for VMOV/VRCP*/VRSQ*.
    pub de: usize,
    /// The `vs`-as-index 0/1 immediate for VRNDN/VRNDP.
    pub vs_index: usize,
}

/// Result of attempting to execute an op through the dispatcher stub. The ops
/// phase replaces the per-op `Unimplemented` arms with `Executed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpStatus {
    /// The op ran and mutated `state` in place.
    Executed,
    /// The op body is not yet implemented (foundation phase). Names which op
    /// so a caller that reaches an unimplemented op fails loudly rather than
    /// silently miscomputing.
    Unimplemented(VuOp),
}

/// Dispatch skeleton: route a decoded op to its body. Today every arm returns
/// `OpStatus::Unimplemented(op)` — the ops phase fills the bodies in, calling
/// into the [`super::vu`] API (element_select, clamp_*, the accumulator and
/// flag helpers). The signature is the stable seam: an op body takes
/// `&mut VuState` and the resolved `OpInvocation` and returns `OpStatus`.
///
/// Keeping this exhaustive `match` here means adding/here-implementing an op
/// is a localized edit and the compiler enforces every `VuOp` is handled.
pub fn dispatch(state: &mut VuState, op: VuOp, inv: OpInvocation) -> OpStatus {
    use super::vu_ops::logic::{self, LogicKind};
    match op {
        // --- Logical family (§6.5): VAND/VNAND/VOR/VNOR/VXOR/VNXOR + VNOP ---
        VuOp::Vand => {
            logic::exec_logic(state, LogicKind::And, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vnand => {
            logic::exec_logic(state, LogicKind::Nand, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vor => {
            logic::exec_logic(state, LogicKind::Or, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vnor => {
            logic::exec_logic(state, LogicKind::Nor, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vxor => {
            logic::exec_logic(state, LogicKind::Xor, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vnxor => {
            logic::exec_logic(state, LogicKind::Nxor, inv.vd, inv.vs, inv.vt, inv.e);
            OpStatus::Executed
        }
        VuOp::Vnop => {
            logic::exec_vnop(state);
            OpStatus::Executed
        }
        // --- Multiply-accumulate family (§6.2) + VSAR (§6.9) ---
        VuOp::Vmacf
        | VuOp::Vmacq
        | VuOp::Vmadh
        | VuOp::Vmadm
        | VuOp::Vmadn
        | VuOp::Vmadl
        | VuOp::Vsar => super::vu_ops::mac::dispatch_mac(state, op, &inv)
            .expect("mac family op routed to dispatch_mac"),
        // --- Select family (§6.6–§6.7): compares/merge/clip ---
        VuOp::Vlt
        | VuOp::Veq
        | VuOp::Vne
        | VuOp::Vge
        | VuOp::Vmrg
        | VuOp::Vch
        | VuOp::Vcl
        | VuOp::Vcr => super::vu_ops::select::try_dispatch(state, op, &inv)
            .expect("select family op routed to select::try_dispatch"),
        // --- "mul-hi" multiply family (set accumulator, §6.1) ---
        VuOp::Vmulf => {
            super::vu_ops::mul_hi::vmulf(state, &inv);
            OpStatus::Executed
        }
        VuOp::Vmulq => {
            super::vu_ops::mul_hi::vmulq(state, &inv);
            OpStatus::Executed
        }
        VuOp::Vmudh => {
            super::vu_ops::mul_hi::vmudh(state, &inv);
            OpStatus::Executed
        }
        VuOp::Vmudm => {
            super::vu_ops::mul_hi::vmudm(state, &inv);
            OpStatus::Executed
        }
        VuOp::Vmudn => {
            super::vu_ops::mul_hi::vmudn(state, &inv);
            OpStatus::Executed
        }
        VuOp::Vmudl => {
            super::vu_ops::mul_hi::vmudl(state, &inv);
            OpStatus::Executed
        }
        // --- Add/subtract family (§6.3) + VABS (§6.4) ---
        VuOp::Vadd | VuOp::Vaddc | VuOp::Vsub | VuOp::Vsubc | VuOp::Vabs => {
            super::vu_ops::addsub::dispatch_addsub(state, op, &inv)
                .expect("addsub family op routed to dispatch_addsub")
        }
        // Other families are implemented by their own op-group modules; until
        // wired here they trap loudly rather than silently miscomputing. The
        // `_ = &inv` keeps the operand fields live for the not-yet-wired arms.
        _ => {
            let _ = &inv;
            OpStatus::Unimplemented(op)
        }
    }
}

/// Convenience for the ops phase: apply the element modifier to an op's `Vt`
/// source, reading it out of the register file. Thin wrapper over
/// [`super::vu::element_select`] that resolves the register index, so op
/// bodies don't re-hardcode the `state.regs.r[inv.vt]` access.
pub fn selected_vt(state: &VuState, inv: &OpInvocation) -> Vec8 {
    super::vu::element_select(&state.regs.r[inv.vt], inv.e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operand_shapes_match_codegen_groups() {
        // Vd,Vs,Vt group sample
        assert_eq!(
            operand_shape(VuOp::Vmulf).operands,
            [OperandRole::Vd, OperandRole::Vs, OperandRole::Vt]
        );
        // Vd,De,Vt group sample
        assert_eq!(
            operand_shape(VuOp::Vrcp).operands,
            [OperandRole::Vd, OperandRole::De, OperandRole::Vt]
        );
        // Vd,VsIndex,Vt
        assert_eq!(
            operand_shape(VuOp::Vrndn).operands,
            [OperandRole::Vd, OperandRole::VsIndex, OperandRole::Vt]
        );
        // Vd,Vs (Vt None)
        assert_eq!(
            operand_shape(VuOp::Vsar).operands,
            [OperandRole::Vd, OperandRole::Vs, OperandRole::None]
        );
        // Vd only, element ignored
        let macq = operand_shape(VuOp::Vmacq);
        assert_eq!(
            macq.operands,
            [OperandRole::Vd, OperandRole::None, OperandRole::None]
        );
        assert!(macq.element_ignored);
        // Nop, element ignored
        assert!(operand_shape(VuOp::Vnop).element_ignored);
    }

    #[test]
    fn dispatch_stub_reports_unimplemented_by_op() {
        let mut state = VuState::new();
        let inv = OpInvocation::default();
        assert_eq!(
            dispatch(&mut state, VuOp::Vmulf, inv),
            OpStatus::Unimplemented(VuOp::Vmulf)
        );
    }

    #[test]
    fn selected_vt_applies_element_modifier() {
        let mut state = VuState::new();
        state.regs.r[5] = [0, 1, 2, 3, 4, 5, 6, 7];
        let inv = OpInvocation {
            vt: 5,
            e: 8, // whole-broadcast lane 0
            ..Default::default()
        };
        assert_eq!(selected_vt(&state, &inv), [0; 8]);
    }
}
