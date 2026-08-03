//! Content-silent inventory for exact generated-body reuse.
//!
//! This module measures reuse before the emitter changes execution shape. It
//! deliberately keys bodies by exact raw words: operand or target
//! parameterization belongs to a later, separately validated optimization.

use std::collections::BTreeSet;

use fn64_recomp_rs::decoder::decode;

/// Exact-body reuse available when sharing is confined to one partition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DenseBodyReuseInventory {
    pub instruction_words: usize,
    pub partition_words: usize,
    pub total_bodies: usize,
    pub unique_bodies: usize,
    /// Generated semantic slots count delay words once as direct entries and
    /// again inside each parent control body, matching the current emitter.
    pub total_semantic_word_slots: usize,
    pub unique_semantic_word_slots: usize,
}

impl DenseBodyReuseInventory {
    pub const fn reusable_bodies(self) -> usize {
        self.total_bodies - self.unique_bodies
    }

    pub const fn reusable_semantic_word_slots(self) -> usize {
        self.total_semantic_word_slots - self.unique_semantic_word_slots
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ExactBody {
    Straight(Vec<u32>),
    Control { word: u32, delay: Option<u32> },
}

/// Inventory exact straight-run and control/delay bodies without exposing ROM
/// words, addresses, paths, or generated source.
///
/// Sharing never crosses `partition_words`. A 512-word partition models the
/// current 2 KiB subrunner boundary; 16,384 words model one 64 KiB artifact.
/// A control word at a partition edge may use the following global word as its
/// delay body, exactly like the current affine lookahead path.
pub fn inventory_dense_body_reuse(
    words: &[u32],
    partition_words: usize,
) -> DenseBodyReuseInventory {
    assert!(
        partition_words > 0,
        "body-reuse partition must contain words"
    );
    let mut result = DenseBodyReuseInventory {
        instruction_words: words.len(),
        partition_words,
        ..DenseBodyReuseInventory::default()
    };

    for partition_start in (0..words.len()).step_by(partition_words) {
        let partition_end = (partition_start + partition_words).min(words.len());
        let mut unique = BTreeSet::new();
        let mut index = partition_start;
        while index < partition_end {
            if decode(words[index]).has_delay_slot() {
                let delay = words.get(index + 1).copied();
                let slots = 1 + usize::from(delay.is_some());
                result.total_bodies += 1;
                result.total_semantic_word_slots += slots;
                unique.insert(ExactBody::Control {
                    word: words[index],
                    delay,
                });
                index += 1;
                continue;
            }

            let run_start = index;
            index += 1;
            while index < partition_end && !decode(words[index]).has_delay_slot() {
                index += 1;
            }
            let run = words[run_start..index].to_vec();
            result.total_bodies += 1;
            result.total_semantic_word_slots += run.len();
            unique.insert(ExactBody::Straight(run));
        }

        result.unique_bodies += unique.len();
        result.unique_semantic_word_slots += unique
            .iter()
            .map(|body| match body {
                ExactBody::Straight(words) => words.len(),
                ExactBody::Control { delay, .. } => 1 + usize::from(delay.is_some()),
            })
            .sum::<usize>();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_reuse_is_partition_scoped_and_counts_delay_duplication() {
        let control = 0x1000_0001; // beq $zero,$zero,+1
        let words = [control, 0, control, 0];
        let whole = inventory_dense_body_reuse(&words, 4);
        assert_eq!(whole.total_bodies, 4);
        assert_eq!(whole.unique_bodies, 2);
        assert_eq!(whole.total_semantic_word_slots, 6);
        assert_eq!(whole.unique_semantic_word_slots, 3);

        let split = inventory_dense_body_reuse(&words, 2);
        assert_eq!(split.total_bodies, 4);
        assert_eq!(split.unique_bodies, 4);
        assert_eq!(split.reusable_semantic_word_slots(), 0);
    }

    #[test]
    fn control_at_partition_edge_uses_global_delay_word() {
        let control = 0x0800_0000; // j 0
        let inventory = inventory_dense_body_reuse(&[0, control, 7], 2);
        assert_eq!(inventory.total_semantic_word_slots, 4);
        assert_eq!(inventory.unique_semantic_word_slots, 4);
    }

    #[test]
    fn inventory_is_content_silent_and_deterministic() {
        let words = [0, 0, 0x1000_0000, 0, 0, 0];
        let first = inventory_dense_body_reuse(&words, 3);
        assert_eq!(first, inventory_dense_body_reuse(&words, 3));
        assert_eq!(format!("{first:?}").matches("214748").count(), 0);
    }
}
