//! Deterministic multi-scale views over normalized ROM bytes. These features
//! propose region boundaries and content kinds; they never prove code or data
//! by themselves.

use fn64_recomp_rs::{decode, Instruction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionWindow {
    pub rom_start: u32,
    pub rom_end: u32,
    pub word_count: u32,
    pub nonzero_bytes: u32,
    pub distinct_byte_values: u16,
    pub zero_words: u32,
    pub jump_words: u32,
    pub coherent_jump_words: u32,
    pub branch_words: u32,
    pub coherent_branch_words: u32,
    pub return_words: u32,
    pub plausible_rdram_pointers: u32,
    pub plausible_rom_offsets: u32,
}

impl RegionWindow {
    fn structured_control_words(&self) -> u32 {
        self.coherent_jump_words
            .saturating_add(self.coherent_branch_words)
            .saturating_add(self.return_words)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionTransition {
    pub boundary_rom: u32,
    pub left_rom_start: u32,
    pub right_rom_end: u32,
    pub structured_control_delta_per_mille: u16,
    pub pointer_delta_per_mille: u16,
    pub zero_word_delta_per_mille: u16,
    pub nonzero_byte_delta_per_mille: u16,
    pub byte_diversity_delta_per_mille: u16,
    pub structured_control_drop_per_mille: u16,
    pub pointer_rise_per_mille: u16,
    pub zero_word_rise_per_mille: u16,
    /// An ordering aid only: the sum of the five named deltas. It is never a
    /// proof state or an opaque confidence value.
    pub rank_score: u32,
    /// Directional text-to-data ordering aid: structured control decreases
    /// while pointer or zero-word structure increases.
    pub code_to_data_score: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionView {
    pub window_bytes: u32,
    pub windows: Vec<RegionWindow>,
    pub transitions: Vec<RegionTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryConsensus {
    pub boundary_rom: u32,
    /// Number of scales whose nearest transition ranks within the requested
    /// top fraction for that scale.
    pub scale_support: u16,
    /// Sum of `1000 - rank_percentile_per_mille` across supporting scales.
    /// This orders equally-supported candidates; it is not a proof state.
    pub rank_points: u32,
    pub finest_rank_per_mille: u16,
}

/// Analyze one contiguous ROM-to-VA image at a fixed scale. `rom_start` and
/// `va_start` identify `bytes[0]`; `total_rom_bytes` bounds words that look
/// like physical ROM offsets. A zero window or non-word-aligned window is
/// rejected loudly rather than rounded into a different analysis.
pub fn analyze(
    bytes: &[u8],
    rom_start: u32,
    va_start: u32,
    total_rom_bytes: usize,
    window_bytes: u32,
) -> Result<RegionView, String> {
    if window_bytes == 0 || !window_bytes.is_multiple_of(4) {
        return Err(format!(
            "region window size 0x{window_bytes:x} must be a nonzero multiple of four"
        ));
    }
    let windows = bytes
        .chunks(window_bytes as usize)
        .enumerate()
        .map(|(index, chunk)| {
            analyze_window(
                chunk,
                rom_start.saturating_add(index as u32 * window_bytes),
                va_start.saturating_add(index as u32 * window_bytes),
                va_start,
                bytes.len(),
                total_rom_bytes,
            )
        })
        .collect::<Vec<_>>();
    let transitions = windows
        .windows(2)
        .map(|pair| transition(&pair[0], &pair[1]))
        .collect();
    Ok(RegionView {
        window_bytes,
        windows,
        transitions,
    })
}

pub fn analyze_multiscale(
    bytes: &[u8],
    rom_start: u32,
    va_start: u32,
    total_rom_bytes: usize,
    window_sizes: &[u32],
) -> Result<Vec<RegionView>, String> {
    window_sizes
        .iter()
        .map(|&window| analyze(bytes, rom_start, va_start, total_rom_bytes, window))
        .collect()
}

/// Cross-scale boundary voting. Candidates are anchored to every transition
/// in the finest supplied view. Each scale votes only when its nearest
/// transition ranks within `top_fraction_per_mille` for that scale.
pub fn consensus_boundaries(
    views: &[RegionView],
    top_fraction_per_mille: u16,
) -> Result<Vec<BoundaryConsensus>, String> {
    if views.is_empty() {
        return Ok(Vec::new());
    }
    if top_fraction_per_mille == 0 || top_fraction_per_mille > 1000 {
        return Err("top boundary fraction must be in 1..=1000 per mille".to_string());
    }
    let finest_index = views
        .iter()
        .enumerate()
        .min_by_key(|(_, view)| view.window_bytes)
        .map(|(index, _)| index)
        .expect("nonempty views");
    let ranked = views
        .iter()
        .map(|view| {
            let mut by_score = view.transitions.iter().collect::<Vec<_>>();
            by_score.sort_by(|left, right| {
                right
                    .rank_score
                    .cmp(&left.rank_score)
                    .then_with(|| left.boundary_rom.cmp(&right.boundary_rom))
            });
            by_score
                .into_iter()
                .enumerate()
                .map(|(rank, item)| {
                    let per_mille = if view.transitions.is_empty() {
                        1000
                    } else {
                        (rank as u64 * 1000 / view.transitions.len() as u64) as u16
                    };
                    (item.boundary_rom, per_mille)
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();

    let finest = &views[finest_index];
    let mut output = Vec::with_capacity(finest.transitions.len());
    for anchor in &finest.transitions {
        let mut scale_support = 0u16;
        let mut rank_points = 0u32;
        for (view, ranks) in views.iter().zip(&ranked) {
            let Some(nearest) = nearest_transition(&view.transitions, anchor.boundary_rom) else {
                continue;
            };
            let rank = ranks[&nearest.boundary_rom];
            if rank <= top_fraction_per_mille {
                scale_support += 1;
                rank_points += u32::from(1000 - rank);
            }
        }
        output.push(BoundaryConsensus {
            boundary_rom: anchor.boundary_rom,
            scale_support,
            rank_points,
            finest_rank_per_mille: ranked[finest_index][&anchor.boundary_rom],
        });
    }
    output.sort_by(|left, right| {
        right
            .scale_support
            .cmp(&left.scale_support)
            .then_with(|| right.rank_points.cmp(&left.rank_points))
            .then_with(|| left.boundary_rom.cmp(&right.boundary_rom))
    });
    Ok(output)
}

fn nearest_transition(transitions: &[RegionTransition], target: u32) -> Option<&RegionTransition> {
    if transitions.is_empty() {
        return None;
    }
    let index = transitions.partition_point(|item| item.boundary_rom < target);
    [
        index.checked_sub(1),
        (index < transitions.len()).then_some(index),
    ]
    .into_iter()
    .flatten()
    .filter_map(|candidate| transitions.get(candidate))
    .min_by_key(|item| (item.boundary_rom.abs_diff(target), item.boundary_rom))
}

fn analyze_window(
    bytes: &[u8],
    rom_start: u32,
    va_start: u32,
    image_va_start: u32,
    image_len: usize,
    total_rom_bytes: usize,
) -> RegionWindow {
    let mut distinct = [false; 256];
    let mut nonzero_bytes = 0;
    for &byte in bytes {
        distinct[byte as usize] = true;
        nonzero_bytes += u32::from(byte != 0);
    }

    let mut zero_words = 0;
    let mut jump_words = 0;
    let mut coherent_jump_words = 0;
    let mut branch_words = 0;
    let mut coherent_branch_words = 0;
    let mut return_words = 0;
    let mut plausible_rdram_pointers = 0;
    let mut plausible_rom_offsets = 0;
    let image_va_end = image_va_start.saturating_add(image_len as u32);

    for (index, word_bytes) in bytes.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(word_bytes.try_into().unwrap());
        let pc = va_start.saturating_add(index as u32 * 4);
        zero_words += u32::from(word == 0);
        plausible_rdram_pointers += u32::from(is_rdram_pointer(word));
        plausible_rom_offsets +=
            u32::from(word >= 0x1000 && word < total_rom_bytes as u32 && word.is_multiple_of(4));

        match decode(word) {
            Instruction::J { target } | Instruction::Jal { target } => {
                jump_words += 1;
                let target = (pc.wrapping_add(4) & 0xf000_0000) | (target << 2);
                coherent_jump_words += u32::from(target >= image_va_start && target < image_va_end);
            }
            instruction => {
                if let Some(off) = branch_offset(&instruction) {
                    branch_words += 1;
                    let target = pc
                        .wrapping_add(4)
                        .wrapping_add((off as i32).wrapping_mul(4) as u32);
                    coherent_branch_words +=
                        u32::from(target >= image_va_start && target < image_va_end);
                }
                return_words += u32::from(matches!(instruction, Instruction::Jr { rs: 31 }));
            }
        }
    }

    RegionWindow {
        rom_start,
        rom_end: rom_start.saturating_add(bytes.len() as u32),
        word_count: (bytes.len() / 4) as u32,
        nonzero_bytes,
        distinct_byte_values: distinct.into_iter().filter(|present| *present).count() as u16,
        zero_words,
        jump_words,
        coherent_jump_words,
        branch_words,
        coherent_branch_words,
        return_words,
        plausible_rdram_pointers,
        plausible_rom_offsets,
    }
}

fn branch_offset(instruction: &Instruction) -> Option<i16> {
    use Instruction::*;
    match instruction {
        Beq { off, .. }
        | Bne { off, .. }
        | Blez { off, .. }
        | Bgtz { off, .. }
        | Bltz { off, .. }
        | Bgez { off, .. }
        | Bltzal { off, .. }
        | Bgezal { off, .. }
        | Beql { off, .. }
        | Bnel { off, .. }
        | Blezl { off, .. }
        | Bgtzl { off, .. }
        | Bltzl { off, .. }
        | Bgezl { off, .. }
        | Bltzall { off, .. }
        | Bgezall { off, .. }
        | Bc0f { off }
        | Bc0t { off }
        | Bc0fl { off }
        | Bc0tl { off }
        | Bc1f { off }
        | Bc1t { off }
        | Bc1fl { off }
        | Bc1tl { off } => Some(*off),
        _ => None,
    }
}

fn is_rdram_pointer(word: u32) -> bool {
    matches!(word, 0x8000_0000..=0x807f_ffff | 0xa000_0000..=0xa07f_ffff) && word.is_multiple_of(4)
}

fn transition(left: &RegionWindow, right: &RegionWindow) -> RegionTransition {
    let left_control = per_mille(left.structured_control_words(), left.word_count);
    let right_control = per_mille(right.structured_control_words(), right.word_count);
    let control = abs_diff(left_control, right_control);
    let left_pointers = left
        .plausible_rdram_pointers
        .saturating_add(left.plausible_rom_offsets);
    let right_pointers = right
        .plausible_rdram_pointers
        .saturating_add(right.plausible_rom_offsets);
    let left_pointer = per_mille(left_pointers, left.word_count);
    let right_pointer = per_mille(right_pointers, right.word_count);
    let pointer = abs_diff(left_pointer, right_pointer);
    let left_zero = per_mille(left.zero_words, left.word_count);
    let right_zero = per_mille(right.zero_words, right.word_count);
    let zero = abs_diff(left_zero, right_zero);
    let nonzero = abs_diff(
        per_mille(left.nonzero_bytes, left.rom_end - left.rom_start),
        per_mille(right.nonzero_bytes, right.rom_end - right.rom_start),
    );
    let diversity = abs_diff(
        per_mille(left.distinct_byte_values as u32, 256),
        per_mille(right.distinct_byte_values as u32, 256),
    );
    RegionTransition {
        boundary_rom: right.rom_start,
        left_rom_start: left.rom_start,
        right_rom_end: right.rom_end,
        structured_control_delta_per_mille: control,
        pointer_delta_per_mille: pointer,
        zero_word_delta_per_mille: zero,
        nonzero_byte_delta_per_mille: nonzero,
        byte_diversity_delta_per_mille: diversity,
        structured_control_drop_per_mille: left_control.saturating_sub(right_control),
        pointer_rise_per_mille: right_pointer.saturating_sub(left_pointer),
        zero_word_rise_per_mille: right_zero.saturating_sub(left_zero),
        rank_score: [control, pointer, zero, nonzero, diversity]
            .into_iter()
            .map(u32::from)
            .sum(),
        code_to_data_score: [
            left_control.saturating_sub(right_control),
            right_pointer.saturating_sub(left_pointer),
            right_zero.saturating_sub(left_zero),
        ]
        .into_iter()
        .map(u32::from)
        .sum(),
    }
}

fn per_mille(numerator: u32, denominator: u32) -> u16 {
    if denominator == 0 {
        return 0;
    }
    ((numerator as u64 * 1000 / denominator as u64).min(1000)) as u16
}

fn abs_diff(left: u16, right: u16) -> u16 {
    left.abs_diff(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    #[test]
    fn rejects_invalid_window_sizes() {
        assert!(analyze(&[0; 16], 0, 0x8000_0000, 16, 0).is_err());
        assert!(analyze(&[0; 16], 0, 0x8000_0000, 16, 6).is_err());
    }

    #[test]
    fn separates_structured_control_pointer_and_zero_views() {
        let code = words(&[
            0x0c00_0008, // jal 0x80000020, coherent within the image
            0x0000_0000,
            0x1000_0001, // branch to 0x80000014, coherent
            0x0000_0000,
            0x03e0_0008,
            0x0000_0000,
            0x0000_0000,
            0x0000_0000,
        ]);
        let pointers = words(&[
            0x8000_0100,
            0x8000_0200,
            0x0000_1000,
            0x0000_2000,
            0x8000_0300,
            0x8000_0400,
            0x0000_3000,
            0x0000_4000,
        ]);
        let zero = vec![0u8; 32];
        let bytes = [code, pointers, zero].concat();
        let view = analyze(&bytes, 0x1000, 0x8000_0000, 0x10000, 32).unwrap();
        assert_eq!(view.windows.len(), 3);
        assert_eq!(view.windows[0].coherent_jump_words, 1);
        assert_eq!(view.windows[0].coherent_branch_words, 1);
        assert_eq!(view.windows[0].return_words, 1);
        assert_eq!(view.windows[1].plausible_rdram_pointers, 4);
        assert_eq!(view.windows[1].plausible_rom_offsets, 4);
        assert_eq!(view.windows[2].zero_words, 8);
        assert!(view.transitions.iter().all(|item| item.rank_score > 0));
    }

    #[test]
    fn control_statistics_follow_the_shared_decoder() {
        let bc0f = (0x10u32 << 26) | (0x08 << 21) | 1;
        let bc1tl = (0x11u32 << 26) | (0x08 << 21) | (0x03 << 16) | 1;
        let bltzal = (0x01u32 << 26) | (1 << 21) | (0x10 << 16) | 1;
        let bytes = words(&[bc0f, bc1tl, bltzal, 0x03e0_0008, 0x7801_2345]);
        let view = analyze(&bytes, 0x1000, 0x8000_0000, 0x2000, bytes.len() as u32).unwrap();
        let window = &view.windows[0];
        assert_eq!(window.branch_words, 3);
        assert_eq!(window.coherent_branch_words, 3);
        assert_eq!(window.return_words, 1);
    }

    #[test]
    fn analysis_is_byte_identical_across_runs_and_scales() {
        let bytes = (0..1024u32)
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        let a = analyze_multiscale(&bytes, 0x1000, 0x8000_0400, 0x200000, &[64, 256]).unwrap();
        let b = analyze_multiscale(&bytes, 0x1000, 0x8000_0400, 0x200000, &[64, 256]).unwrap();
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
    }

    #[test]
    fn consensus_requires_high_ranked_support_at_multiple_scales() {
        let make_view = |window_bytes, scores: &[(u32, u32)]| RegionView {
            window_bytes,
            windows: vec![],
            transitions: scores
                .iter()
                .map(|&(boundary_rom, rank_score)| RegionTransition {
                    boundary_rom,
                    left_rom_start: boundary_rom - window_bytes,
                    right_rom_end: boundary_rom + window_bytes,
                    structured_control_delta_per_mille: 0,
                    pointer_delta_per_mille: 0,
                    zero_word_delta_per_mille: 0,
                    nonzero_byte_delta_per_mille: 0,
                    byte_diversity_delta_per_mille: 0,
                    structured_control_drop_per_mille: 0,
                    pointer_rise_per_mille: 0,
                    zero_word_rise_per_mille: 0,
                    rank_score,
                    code_to_data_score: rank_score,
                })
                .collect(),
        };
        let fine = make_view(0x40, &[(0x100, 90), (0x140, 10), (0x180, 80)]);
        let coarse = make_view(0x100, &[(0x100, 100), (0x200, 1)]);
        let consensus = consensus_boundaries(&[fine, coarse], 500).unwrap();
        assert_eq!(consensus[0].boundary_rom, 0x100);
        assert_eq!(consensus[0].scale_support, 2);
        assert!(consensus[0].rank_points > consensus[1].rank_points);
    }
}
