//! Donor-signature scan: a cited-claim recall lane for statically-dead code.
//!
//! The 2026-07-20 boot-bank frontier measurement showed the open answer
//! functions are dominated by code with no static caller and no runtime
//! execution in normal play (osPfs* pak family, Sleep_*, libgcc __ll_*):
//! unreachable by CFG descent AND by dynamic tracing. What does recover
//! them is the same evidence n64sym-style SDK signature databases use —
//! the bodies are byte-identical modulo relocation across every libultra
//! game.
//!
//! A signature here is the relocation-masked full body
//! ([`crate::homology::relocation_masked_word`]) of one function from a
//! DONOR ROM's answer key: a cited external claim encoding prior RE of a
//! *different* ROM. The target ROM's own answer key is never read — the
//! grading firewall stands. Matches seed CFG roots that are deliberately
//! NOT excused as interior entries: nothing machine-checks a signature,
//! so a false match that splits a real answer function must FAIL the
//! wrong==0 gate loudly rather than be excused.
//!
//! Masking loses all immediate/offset fields, so short bodies collide.
//! Two guards keep the lane honest without a confidence score:
//! signatures shorter than [`MIN_SIGNATURE_WORDS`] are never built, and a
//! signature matching more than [`MAX_MATCHES_PER_SIGNATURE`] sites is
//! degenerate (epilogue-shaped filler) and is dropped entirely.

use crate::homology::relocation_masked_word;
use std::collections::HashMap;

/// Shortest body worth a signature: 8 words / 32 bytes. Below this the
/// masked body is mostly opcode skeleton and collides across unrelated
/// leaf functions.
pub const MIN_SIGNATURE_WORDS: usize = 8;

/// A signature matching more sites than this is not identifying anything.
pub const MAX_MATCHES_PER_SIGNATURE: usize = 4;

/// One donor function's relocation-masked body. `name` is diagnostic
/// only — grading never consumes it.
#[derive(Debug, Clone)]
pub struct DonorSignature {
    pub name: String,
    pub masked_body: Vec<u32>,
}

/// One accepted match: the target VA whose masked words equal a donor
/// body exactly, end to end.
#[derive(Debug, Clone)]
pub struct SigMatch {
    pub va: u32,
    pub name: String,
}

/// Build signatures from a donor bank image. `functions` carries
/// `(name, va, size_bytes)` for the donor's answer-key functions; entries
/// outside the bank image or shorter than `min_words` are skipped.
pub fn donor_signatures(
    functions: &[(String, u32, u32)],
    bank_words: &[u32],
    bank_va_start: u32,
    min_words: usize,
) -> Vec<DonorSignature> {
    let mut signatures = Vec::new();
    for (name, va, size) in functions {
        let word_len = (*size as usize) / 4;
        if word_len < min_words {
            continue;
        }
        let Some(offset) = va.checked_sub(bank_va_start).map(|d| (d / 4) as usize) else {
            continue;
        };
        let Some(body) = bank_words.get(offset..offset + word_len) else {
            continue;
        };
        // No real function starts with a nop: a donor body that does is a
        // key artifact (leading padding folded into the cited size), and
        // matching it against a target's padding run seeds a root inside
        // the neighbor's padded extent (observed on MM's
        // __osInitialize_autodetect padding).
        if body.first() == Some(&0) {
            continue;
        }
        signatures.push(DonorSignature {
            name: name.clone(),
            masked_body: body.iter().map(|w| relocation_masked_word(*w)).collect(),
        });
    }
    signatures
}

/// Scan the target window for full-body masked matches. Returns accepted
/// matches sorted by VA, deduplicated by VA (identical SDK bodies under
/// several donor names are one entry claim). Degenerate signatures — more
/// than [`MAX_MATCHES_PER_SIGNATURE`] sites — are dropped whole.
pub fn scan_signatures(
    signatures: &[DonorSignature],
    target_words: &[u32],
    target_va_start: u32,
) -> Vec<SigMatch> {
    let masked_target: Vec<u32> = target_words
        .iter()
        .map(|w| relocation_masked_word(*w))
        .collect();
    // First-masked-word index; bodies this short were already rejected.
    let mut by_first: HashMap<u32, Vec<usize>> = HashMap::new();
    for (index, signature) in signatures.iter().enumerate() {
        if let Some(first) = signature.masked_body.first() {
            by_first.entry(*first).or_default().push(index);
        }
    }

    let mut per_signature: Vec<Vec<u32>> = vec![Vec::new(); signatures.len()];
    for (offset, first) in masked_target.iter().enumerate() {
        if !plausible_function_boundary(target_words, offset) {
            continue;
        }
        let Some(candidates) = by_first.get(first) else {
            continue;
        };
        for &index in candidates {
            let body = &signatures[index].masked_body;
            if masked_target.len() - offset < body.len() {
                continue;
            }
            if &masked_target[offset..offset + body.len()] == body.as_slice() {
                per_signature[index].push(target_va_start + (offset as u32) * 4);
            }
        }
    }

    let mut matches: Vec<SigMatch> = Vec::new();
    for (index, sites) in per_signature.iter().enumerate() {
        if sites.is_empty() || sites.len() > MAX_MATCHES_PER_SIGNATURE {
            continue;
        }
        for va in sites {
            matches.push(SigMatch {
                va: *va,
                name: signatures[index].name.clone(),
            });
        }
    }
    matches.sort_by_key(|m| m.va);
    matches.dedup_by_key(|m| m.va);
    matches
}

/// True where a function can plausibly start. Shared by the signature
/// scan and the stored-pointer harvest.
///
/// A signature match is only credible where a function can actually
/// start: the window base, after nop padding, or directly after a
/// function terminator — `jr $ra` or `eret` in the second-to-last slot
/// (their delay/successor slot is the word immediately before the
/// candidate), or an unconditional `j` (a tail transfer ends its
/// function the same way). Without this, a donor SDK whose linker cut
/// the same code into different function boundaries full-body-matches
/// INSIDE a larger target function and splits it (observed: MM's
/// __osException vs an older donor SDK).
pub fn plausible_function_boundary(raw_words: &[u32], offset: usize) -> bool {
    if offset == 0 {
        return true;
    }
    if offset < 2 {
        return false;
    }
    let prev1 = raw_words[offset - 1];
    let prev2 = raw_words[offset - 2];
    // Directly after a terminator (its delay/successor slot is prev1,
    // which may be any instruction).
    if prev2 == 0x03e0_0008 /* jr $ra */ || prev2 == 0x4200_0018 /* eret */ {
        return true;
    }
    // Alignment padding: two consecutive nops. A single nop is NOT
    // enough — it is routinely a branch delay slot mid-function
    // (observed: `b; nop` inside MM's __osException). A tail-`j` is not
    // accepted either: handwritten SDK assembly uses `j` for loops.
    prev1 == 0 && prev2 == 0
}

/// Find embedded handler-table entries: function pointers stored in a
/// dense `const Func[]` array in a bank's data.
///
/// SM64-class code dispatches object actions, camera modes, and cutscene
/// shots through static `const Func table[]` arrays (`sBowserActions`,
/// `sCameraModes`, `sCutsceneShots`), NOT through instruction-materialized
/// pointers or a bytecode interpreter. Each array is a run of consecutive
/// 4-aligned words that are all valid in-window code addresses, and each
/// entry is a callable function the CFG never reaches statically (the
/// interpreter indexes the array at runtime). This is the dispatch
/// equivalent of a jump table, one level up.
///
/// Detection is a run of `>= min_run` consecutive words where every word
/// is a 4-aligned address inside `[va_start, code_end)` AND lands on a
/// [`plausible_function_boundary`]. The boundary requirement is the guard
/// against a run of float/fixed-point constants that alias as `0x80xxxxxx`
/// addresses: an aliasing run essentially never has all of its words point
/// at real function starts, whereas a genuine handler table does by
/// construction. Returns each such entry VA (deduplicated, sorted); the
/// caller seeds them as callable-entry roots and `wrong == 0` is the final
/// adversarial judge.
pub fn detect_handler_tables(
    bank_words: &[u32],
    text_words: &[u32],
    va_start: u32,
    code_end: u32,
    min_run: usize,
) -> Vec<u32> {
    let is_entry = |word: u32| -> bool {
        if !word.is_multiple_of(4) || word < va_start || word >= code_end {
            return false;
        }
        let offset = ((word - va_start) / 4) as usize;
        text_words
            .get(offset)
            .is_some_and(|&first| first != 0)
            && plausible_function_boundary(text_words, offset)
    };

    let mut entries: Vec<u32> = Vec::new();
    let mut run_start: Option<usize> = None;
    for index in 0..=bank_words.len() {
        let in_run = index < bank_words.len() && is_entry(bank_words[index]);
        match (run_start, in_run) {
            (None, true) => run_start = Some(index),
            (Some(start), false) => {
                if index - start >= min_run {
                    entries.extend(bank_words[start..index].iter().copied());
                }
                run_start = None;
            }
            _ => {}
        }
    }
    entries.sort_unstable();
    entries.dedup();
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 8-word body: prologue, a lui/addiu pair (address material that
    // differs between donor and target), work, epilogue.
    fn body(hi: u32, lo: u32) -> Vec<u32> {
        vec![
            0x27bdffe8,      // addiu sp, sp, -0x18
            0xafbf0014,      // sw ra, 0x14(sp)
            0x3c040000 | hi, // lui a0, HI  (masked)
            0x24840000 | lo, // addiu a0, a0, LO  (masked)
            0x0c000000 | hi, // jal ...  (masked)
            0x00000000,      // nop
            0x8fbf0014,      // lw ra, 0x14(sp)
            0x03e00008,      // jr ra
        ]
    }

    #[test]
    fn relocated_copy_matches_and_seeds_correct_va() {
        let donor_words = body(0x1234, 0x5678);
        let functions = vec![("osThing".to_string(), 0x80000000, 32)];
        let signatures = donor_signatures(&functions, &donor_words, 0x80000000, 8);
        assert_eq!(signatures.len(), 1);

        // Target: two padding words, then the same body at different
        // immediates (relocated), then padding.
        let mut target = vec![0x00000000, 0x00000000];
        target.extend(body(0x4321, 0x8765));
        target.push(0x03e00008);
        let matches = scan_signatures(&signatures, &target, 0x80080000);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].va, 0x80080008);
        assert_eq!(matches[0].name, "osThing");
    }

    #[test]
    fn short_functions_get_no_signature() {
        let donor_words = body(0, 0);
        let functions = vec![("tiny".to_string(), 0x80000000, 16)];
        assert!(donor_signatures(&functions, &donor_words, 0x80000000, 8).is_empty());
    }

    #[test]
    fn degenerate_signature_matching_everywhere_is_dropped() {
        let filler = vec![0x00000000u32; 8];
        let signatures = vec![DonorSignature {
            name: "filler".to_string(),
            masked_body: filler.clone(),
        }];
        // 6 overlapping match sites in a run of 13 zero words.
        let target = vec![0x00000000u32; 13];
        assert!(scan_signatures(&signatures, &target, 0x80000000).is_empty());
    }

    #[test]
    fn match_inside_a_larger_function_is_rejected() {
        let donor_words = body(0x1234, 0x5678);
        let functions = vec![("osThing".to_string(), 0x80000000, 32)];
        let signatures = donor_signatures(&functions, &donor_words, 0x80000000, 8);
        // Target: the body appears mid-function — preceded by ordinary
        // instructions (no jr $ra / eret / j terminator, no nop padding).
        let mut target = vec![0x00431021u32, 0x24630004]; // addu, addiu
        target.extend(body(0x4321, 0x8765));
        assert!(scan_signatures(&signatures, &target, 0x80000000).is_empty());
    }

    #[test]
    fn match_after_jr_ra_terminator_is_accepted() {
        let donor_words = body(0x1234, 0x5678);
        let functions = vec![("osThing".to_string(), 0x80000000, 32)];
        let signatures = donor_signatures(&functions, &donor_words, 0x80000000, 8);
        // Target: previous function ends jr $ra + non-nop delay slot.
        let mut target = vec![0x03e00008u32, 0x24020001]; // jr ra; addiu v0,1
        target.extend(body(0x4321, 0x8765));
        let matches = scan_signatures(&signatures, &target, 0x80000000);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].va, 0x80000008);
    }

    #[test]
    fn mid_body_prefix_does_not_match_without_full_body() {
        let donor_words = body(0, 0);
        let functions = vec![("osThing".to_string(), 0x80000000, 32)];
        let signatures = donor_signatures(&functions, &donor_words, 0x80000000, 8);
        // Target contains only the first 6 words of the body.
        let target: Vec<u32> = donor_words[..6].to_vec();
        assert!(scan_signatures(&signatures, &target, 0x80000000).is_empty());
    }

    #[test]
    fn detect_handler_tables_finds_dense_pointer_run_not_floats() {
        // text: 4 leaf functions, each `jr $ra; nop` so an address into
        // one is a plausible boundary. Layout: fn at +0x00, +0x08, +0x10,
        // +0x18 (each 2 words, jr ra / nop).
        let va = 0x8024_6000;
        let text: Vec<u32> = vec![
            0x03e00008, 0x00000000, // fn @ 0x...00
            0x03e00008, 0x00000000, // fn @ 0x...08
            0x03e00008, 0x00000000, // fn @ 0x...10
            0x03e00008, 0x00000000, // fn @ 0x...18
        ];
        let code_end = va + (text.len() as u32) * 4;
        // A handler table of 4 entries pointing at the 4 fn starts.
        let table = [va, va + 0x08, va + 0x10, va + 0x18];
        // Bank words = text, then some non-pointer data, then the table.
        let mut bank = text.clone();
        bank.extend_from_slice(&[0x3f800000, 0x42280000]); // floats, not ptrs
        bank.extend_from_slice(&table);
        let entries = detect_handler_tables(&bank, &text, va, code_end, 4);
        assert_eq!(entries, vec![va, va + 0x08, va + 0x10, va + 0x18]);

        // A run below the threshold is not a table.
        let mut short = text.clone();
        short.extend_from_slice(&[va, va + 0x08, va + 0x10]); // only 3
        assert!(detect_handler_tables(&short, &text, va, code_end, 4).is_empty());

        // A run of float aliases that do NOT land on boundaries is rejected
        // even at length >= min_run (they point mid-function / at nops).
        let aliases = [va + 0x04, va + 0x0c, va + 0x14, va + 0x1c]; // +4 = delay-slot nops
        let mut floaty = text.clone();
        floaty.extend_from_slice(&aliases);
        assert!(
            detect_handler_tables(&floaty, &text, va, code_end, 4).is_empty(),
            "addresses pointing at delay-slot nops are not function boundaries"
        );
    }
}
