//! VA-slot aliasing for recovered overlay banks.
//!
//! Recovered overlay images do not each get their own address range. A game
//! reuses one RDRAM slot for several images and DMAs whichever it needs, so
//! several proven `RomMapping` facts can name the same `va_start`. Measured on
//! the AKI corpus: WM2000 loads bank1 and bank4 both at `0x800E_1B90` and
//! bank2/bank3 both at `0x8011_C900`; No Mercy loads R1/R4 at `0x800D_9960`
//! and R2/R3/R5 at `0x8010_6760`.
//!
//! That makes a VA an ambiguous name. "The function at `0x8011_C930`" is not
//! one function — it is two different functions depending on which image is
//! resident. Any lane that converts a VA into a byte-level claim (a jal target
//! becoming a callable root, an answer function becoming gradeable) must first
//! establish WHICH image it means, or admit that it cannot.
//!
//! This module states that rule once, mechanically, from proven mappings and
//! materialized bytes alone. No answer key, ROM identity, or per-game constant
//! enters it.

use std::collections::BTreeMap;

/// One recovered image occupying a VA range: enough to resolve a VA to bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotImage {
    /// Proven bank name. Unique per image even when `va_start` is not.
    pub bank: String,
    pub va_start: u32,
    /// Materialized image bytes, big-endian words as stored.
    pub bytes: Vec<u8>,
}

impl SlotImage {
    fn va_end(&self) -> u32 {
        self.va_start.wrapping_add(self.bytes.len() as u32)
    }

    fn contains(&self, va: u32) -> bool {
        va >= self.va_start && va < self.va_end()
    }

    /// The word at `va`, or `None` when `va` is outside or unaligned.
    fn word_at(&self, va: u32) -> Option<u32> {
        if !va.is_multiple_of(4) || !self.contains(va) {
            return None;
        }
        let offset = (va - self.va_start) as usize;
        self.bytes
            .get(offset..offset + 4)
            .map(|slice| u32::from_be_bytes(slice.try_into().unwrap()))
    }
}

/// Why a VA could (or could not) be converted into a byte-level claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotResolution {
    /// Exactly one recovered image covers this VA. The VA names those bytes
    /// unconditionally, so a claim about it is a claim about one function.
    Unique { bank: String },
    /// Several images cover the VA and every one of them holds the SAME word
    /// there. Whichever image is resident, the claim lands on identical bytes,
    /// so it is still a claim about one instruction stream at that address.
    /// Weaker than `Unique` and deliberately so: it says the disagreement
    /// cannot matter here, not that there is no disagreement.
    AgreedAcrossAliases { banks: Vec<String> },
    /// Several images cover the VA and disagree about its contents. The VA
    /// names different code in different images; nothing in the ROM says which
    /// one a given call site meant. Callers must leave the claim unconverted.
    Ambiguous { banks: Vec<String> },
    /// No recovered image covers the VA.
    Uncovered,
}

impl SlotResolution {
    /// True where a VA→bytes claim is admissible. This is the M1 rule: either
    /// the slot is filled by exactly one image, or every aliased image agrees
    /// byte-identically at that address.
    pub fn admissible(&self) -> bool {
        matches!(self, Self::Unique { .. } | Self::AgreedAcrossAliases { .. })
    }
}

/// Recovered images indexed by the VA slot they fill.
#[derive(Debug, Clone, Default)]
pub struct SlotCatalog {
    images: Vec<SlotImage>,
}

impl SlotCatalog {
    pub fn new(images: Vec<SlotImage>) -> Self {
        let mut images = images;
        images
            .sort_by(|left, right| (left.va_start, &left.bank).cmp(&(right.va_start, &right.bank)));
        Self { images }
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn images(&self) -> &[SlotImage] {
        &self.images
    }

    /// Group images by the VA they load at. Several entries under one key is
    /// exactly the aliasing hazard.
    pub fn slots(&self) -> BTreeMap<u32, Vec<&SlotImage>> {
        let mut slots: BTreeMap<u32, Vec<&SlotImage>> = BTreeMap::new();
        for image in &self.images {
            slots.entry(image.va_start).or_default().push(image);
        }
        slots
    }

    /// How many VA slots hold more than one image.
    pub fn aliased_slot_count(&self) -> usize {
        self.slots()
            .values()
            .filter(|images| images.len() > 1)
            .count()
    }

    /// Decide whether `va` names one instruction stream across every recovered
    /// image that covers it.
    pub fn resolve(&self, va: u32) -> SlotResolution {
        let covering: Vec<&SlotImage> = self
            .images
            .iter()
            .filter(|image| image.contains(va))
            .collect();
        match covering.as_slice() {
            [] => SlotResolution::Uncovered,
            [only] => SlotResolution::Unique {
                bank: only.bank.clone(),
            },
            many => {
                let banks: Vec<String> = many.iter().map(|image| image.bank.clone()).collect();
                // Compare the word each image holds at this VA. A VA that is
                // unaligned or otherwise unreadable in ANY covering image is
                // treated as disagreement: an unreadable claim is not an
                // agreed one.
                let mut words = many.iter().map(|image| image.word_at(va));
                let first = words.next().flatten();
                match first {
                    Some(word) if words.all(|other| other == Some(word)) => {
                        SlotResolution::AgreedAcrossAliases { banks }
                    }
                    _ => SlotResolution::Ambiguous { banks },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(bank: &str, va_start: u32, words: &[u32]) -> SlotImage {
        SlotImage {
            bank: bank.to_string(),
            va_start,
            bytes: words.iter().flat_map(|word| word.to_be_bytes()).collect(),
        }
    }

    #[test]
    fn a_va_covered_by_one_image_resolves_uniquely() {
        let catalog = SlotCatalog::new(vec![image("ovl_a", 0x8009_0000, &[1, 2, 3, 4])]);
        assert_eq!(
            catalog.resolve(0x8009_0004),
            SlotResolution::Unique {
                bank: "ovl_a".into()
            }
        );
        assert!(catalog.resolve(0x8009_0004).admissible());
    }

    #[test]
    fn a_va_no_image_covers_is_uncovered_and_inadmissible() {
        let catalog = SlotCatalog::new(vec![image("ovl_a", 0x8009_0000, &[1, 2])]);
        assert_eq!(catalog.resolve(0x8009_0100), SlotResolution::Uncovered);
        assert!(!catalog.resolve(0x8009_0100).admissible());
    }

    #[test]
    fn aliased_images_that_disagree_leave_the_va_ambiguous() {
        // The measured hazard: two images at one slot (WM2000 bank1/bank4 at
        // 0x800E1B90). At an address where they hold different words, the VA
        // names two different functions.
        let catalog = SlotCatalog::new(vec![
            image("bank1", 0x800E_1B90, &[0x27bd_ffe8, 0xafbf_0014]),
            image("bank4", 0x800E_1B90, &[0x27bd_ffe8, 0x3c02_8010]),
        ]);
        let resolution = catalog.resolve(0x800E_1B94);
        assert_eq!(
            resolution,
            SlotResolution::Ambiguous {
                banks: vec!["bank1".into(), "bank4".into()]
            }
        );
        assert!(!resolution.admissible());
    }

    #[test]
    fn aliased_images_that_agree_byte_identically_are_admissible() {
        // Same slot, same word: whichever image is resident, a claim about
        // this VA lands on identical bytes. Admissible, but recorded as the
        // weaker `AgreedAcrossAliases`, never as `Unique`.
        let catalog = SlotCatalog::new(vec![
            image("bank1", 0x800E_1B90, &[0x27bd_ffe8, 0xafbf_0014]),
            image("bank4", 0x800E_1B90, &[0x27bd_ffe8, 0x3c02_8010]),
        ]);
        let resolution = catalog.resolve(0x800E_1B90);
        assert_eq!(
            resolution,
            SlotResolution::AgreedAcrossAliases {
                banks: vec!["bank1".into(), "bank4".into()]
            }
        );
        assert!(resolution.admissible());
    }

    #[test]
    fn a_va_covered_by_only_one_of_two_aliases_is_unique_not_ambiguous() {
        // Aliased images need not be the same length. Past the shorter one's
        // end, only the longer image covers the VA, so it is unambiguous.
        let catalog = SlotCatalog::new(vec![
            image("short", 0x800E_1B90, &[1, 2]),
            image("long", 0x800E_1B90, &[1, 9, 9, 9]),
        ]);
        assert_eq!(
            catalog.resolve(0x800E_1B98),
            SlotResolution::Unique {
                bank: "long".into()
            }
        );
    }

    #[test]
    fn an_unaligned_va_inside_aliases_is_never_admissible() {
        // A misaligned VA cannot be read as a word in either image, so the
        // "they agree" test has nothing to compare. It must not fall through
        // to admissible.
        let catalog = SlotCatalog::new(vec![
            image("bank1", 0x800E_1B90, &[0x27bd_ffe8, 0xafbf_0014]),
            image("bank4", 0x800E_1B90, &[0x27bd_ffe8, 0xafbf_0014]),
        ]);
        let resolution = catalog.resolve(0x800E_1B92);
        assert!(
            !resolution.admissible(),
            "unaligned VA must not be admitted"
        );
    }

    #[test]
    fn aliased_slot_count_reports_the_hazard() {
        let catalog = SlotCatalog::new(vec![
            image("bank1", 0x800E_1B90, &[1]),
            image("bank4", 0x800E_1B90, &[2]),
            image("bank2", 0x8011_C900, &[3]),
            image("solo", 0x8020_0000, &[4]),
        ]);
        assert_eq!(catalog.aliased_slot_count(), 1);
        assert_eq!(catalog.slots().len(), 3);
    }
}
