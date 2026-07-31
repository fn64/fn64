//! The whole-extent consistency proof: one question that answers both "is this
//! code" and "where does it load".
//!
//! ROM-gated; an unset var is a loud skip, never a silent pass.

use fn64_discover::delta_vote::{prove_region, DeltaVoteConfig};

fn rom(var: &str) -> Option<Vec<u8>> {
    let path = std::env::var_os(var)?;
    Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{var}: {e}")))
}

/// The eight spans a prologue-presence heuristic wrongly called code. The proof
/// must reject every one -- that is what lets it replace the heuristic as the
/// gate rather than merely supplement it.
#[test]
fn the_proof_rejects_known_false_positives() {
    let Some(bytes) = rom("FN64_DISCOVER_OOT_ROM") else {
        eprintln!("skip: FN64_DISCOVER_OOT_ROM unset");
        return;
    };
    let config = DeltaVoteConfig::default();
    let false_positives = [
        0x0a65000u32,
        0x0bf60d0,
        0x10709a0,
        0x12009a0,
        0x12089a0,
        0x12da9a0,
        0x19dc9a0,
        0x1b749a0,
    ];
    for start in false_positives {
        let proved = prove_region(&bytes, start, start + 0x2000, &config);
        assert!(
            proved.is_none(),
            "false positive at 0x{start:07x} was admitted as {proved:?}"
        );
    }
}

/// And it must still admit genuine code, at the RIGHT address. OoT's boot file
/// is ROM 0x1060 and the header entry is 0x80000400, so the only correct answer
/// is 0x80000460.
#[test]
fn the_proof_admits_known_code_at_its_true_address() {
    let Some(bytes) = rom("FN64_DISCOVER_OOT_ROM") else {
        eprintln!("skip: FN64_DISCOVER_OOT_ROM unset");
        return;
    };
    let proved = prove_region(&bytes, 0x1060, 0x7430, &DeltaVoteConfig::default())
        .expect("OoT's boot file is code and must admit");
    assert_eq!(proved.va_start, 0x8000_0460);
}

/// Whole-extent voting is the unlock. WCW World Tour's two images return `Open`
/// on every fixed 32 KiB window; over their natural extents both admit -- one
/// directly, one only once the `lui`-window narrowing is dropped.
#[test]
fn whole_extents_admit_where_fixed_windows_cannot() {
    let Some(bytes) = rom("FN64_DISCOVER_WCWWT_ROM") else {
        eprintln!("skip: FN64_DISCOVER_WCWWT_ROM unset");
        return;
    };
    let config = DeltaVoteConfig::default();

    let span2 = prove_region(&bytes, 0xa69000, 0xac1000, &config)
        .expect("WCW's 352 KiB image must admit over its whole extent");
    assert_eq!(span2.va_start, 0x8008_f750);
    assert!(
        !span2.full_sweep_required,
        "span2 admits without escalation"
    );

    let span1 = prove_region(&bytes, 0xa21000, 0xa57000, &config)
        .expect("WCW's 216 KiB image must admit once the candidate set is widened");
    assert_eq!(span1.va_start, 0x8008_f6c0);
    assert!(
        span1.full_sweep_required,
        "span1 is the case that motivates the escalation; if it now admits \
         without full_sweep the narrowing changed and the escalation may be dead code"
    );
}
