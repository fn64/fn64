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

use std::collections::{BTreeMap, BTreeSet};

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

    /// Iterate cross-image callable authority to a fixpoint, bank-scoped.
    ///
    /// M1 roots each overlay from its own text alone, so a function reachable
    /// only from a *sibling* image is never rooted: overlay→overlay authority
    /// is circular until something outside breaks in. `boot_roots` are the
    /// entries the boot bank already proved (M1's cross-bank lane in reverse),
    /// and this closes the induction — a newly authorized entry becomes a root
    /// whose own text can authorize further targets, repeating until nothing
    /// new appears.
    ///
    /// Soundness rests on the result being keyed by BANK, never by VA. A jal
    /// site lives in one identified image at a known offset, so the site half
    /// is never ambiguous. The target half is where aliasing bites, and the
    /// two directions are treated differently because the evidence differs:
    ///
    /// * A target inside the site's OWN image is same-image by construction —
    ///   one image, one meaning, no slot question to ask.
    /// * A target in a sibling image must survive [`SlotCatalog::resolve`]:
    ///   either exactly one image covers it, or every alias agrees
    ///   byte-identically. An `Ambiguous` VA names different code depending on
    ///   what is resident, and nothing in the ROM says which the site meant.
    ///
    /// The residency constraint follows from the same keying. Authority is
    /// only ever recorded for the bank that actually contains the target's
    /// bytes, so an image can never authorize an entry in a sibling that
    /// cannot be co-resident with it: the sibling's own entry is what gets
    /// recorded, in the sibling's own universe, and it stands or falls on its
    /// own bytes.
    ///
    /// `max_rounds` bounds the induction. Convergence is expected in a handful
    /// of rounds (the call graph's cross-image depth is small), so exhausting
    /// the budget means the fixpoint did not settle and the caller gets `None`
    /// — fail closed rather than ship a half-converged root set.
    pub fn inductive_bank_roots(
        &self,
        boot_roots: &BTreeMap<String, BTreeSet<u32>>,
        max_rounds: usize,
    ) -> Option<InductiveRoots> {
        let mut roots: BTreeMap<String, BTreeSet<u32>> = boot_roots.clone();
        for image in &self.images {
            roots.entry(image.bank.clone()).or_default();
        }
        let mut rejected_ambiguous = 0usize;
        for round in 1..=max_rounds {
            let mut added = 0usize;
            // Sweep every image's text for direct jals. A root's own text is
            // what carries authority forward, so the sweep is over the whole
            // image each round; the root set only grows, so re-sweeping is
            // idempotent and the fixpoint is well defined.
            for image in &self.images {
                for (index, chunk) in image.bytes.chunks_exact(4).enumerate() {
                    let word = u32::from_be_bytes(chunk.try_into().unwrap());
                    if word >> 26 != 0x03 {
                        continue;
                    }
                    let pc = image.va_start.wrapping_add((index as u32) * 4);
                    let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
                    // Same-image target: unambiguous by construction.
                    let owner = if image.contains(target) {
                        image.bank.clone()
                    } else {
                        match self.resolve(target) {
                            SlotResolution::Unique { bank } => bank,
                            SlotResolution::AgreedAcrossAliases { banks } => {
                                // Every alias holds identical bytes here, so
                                // the claim lands on one instruction stream.
                                // Record it for each, in its own universe.
                                let mut any = false;
                                for bank in banks {
                                    if roots.entry(bank).or_default().insert(target) {
                                        any = true;
                                    }
                                }
                                if any {
                                    added += 1;
                                }
                                continue;
                            }
                            SlotResolution::Ambiguous { .. } => {
                                rejected_ambiguous += 1;
                                continue;
                            }
                            SlotResolution::Uncovered => continue,
                        }
                    };
                    if roots.entry(owner).or_default().insert(target) {
                        added += 1;
                    }
                }
            }
            if added == 0 {
                return Some(InductiveRoots {
                    roots,
                    rounds: round,
                    rejected_ambiguous,
                });
            }
        }
        None
    }
}

/// The fixpoint of [`SlotCatalog::inductive_bank_roots`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InductiveRoots {
    /// Callable entries per bank name. Keyed by bank, never by VA, so two
    /// images sharing a slot keep separate root sets.
    pub roots: BTreeMap<String, BTreeSet<u32>>,
    /// Rounds taken to converge, including the final no-op round.
    pub rounds: usize,
    /// Jal targets left unconverted because aliased images disagreed there.
    pub rejected_ambiguous: usize,
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

    /// `jal target` encoded at a site whose region shares `target`'s top nibble.
    fn jal(target: u32) -> u32 {
        0x0c00_0000 | ((target & 0x0fff_ffff) >> 2)
    }

    #[test]
    fn induction_carries_authority_across_images_to_a_fixpoint() {
        // Two non-aliased images. A boot-proven entry in `first` calls into
        // `second`, and `second`'s own text then calls further within itself.
        // One M1-style sweep roots only what boot named; the fixpoint must
        // reach the third function through the second.
        let first = image(
            "first",
            0x8010_0000,
            &[jal(0x8020_0000), 0x0000_0000, 0x03e0_0008, 0x0000_0000],
        );
        let second = image(
            "second",
            0x8020_0000,
            &[jal(0x8020_0010), 0x0000_0000, 0x03e0_0008, 0x0000_0000,
              0x27bd_ffe8, 0x03e0_0008, 0x0000_0000, 0x0000_0000],
        );
        let catalog = SlotCatalog::new(vec![first, second]);
        let mut boot = BTreeMap::new();
        boot.insert("first".to_string(), BTreeSet::from([0x8010_0000]));

        let fixpoint = catalog
            .inductive_bank_roots(&boot, 8)
            .expect("induction converges");
        assert!(
            fixpoint.roots["second"].contains(&0x8020_0000),
            "boot-rooted image must authorize its sibling's entry"
        );
        assert!(
            fixpoint.roots["second"].contains(&0x8020_0010),
            "the sibling's own text must then authorize further entries"
        );
        assert_eq!(fixpoint.rejected_ambiguous, 0);
    }

    #[test]
    fn induction_refuses_a_target_two_aliases_disagree_about() {
        // Both images fill 0x8030_0000 and hold DIFFERENT words at the called
        // address. The VA names different code depending on residency, so the
        // call must stay unconverted rather than pick one.
        let caller = image("caller", 0x8010_0000, &[jal(0x8030_0004), 0x0000_0000]);
        let alias_a = image("alias_a", 0x8030_0000, &[0x0000_0000, 0x27bd_ffe8]);
        let alias_b = image("alias_b", 0x8030_0000, &[0x0000_0000, 0x3c02_8010]);
        let catalog = SlotCatalog::new(vec![caller, alias_a, alias_b]);

        let fixpoint = catalog
            .inductive_bank_roots(&BTreeMap::new(), 8)
            .expect("induction converges");
        assert_eq!(fixpoint.rejected_ambiguous, 1);
        assert!(!fixpoint.roots["alias_a"].contains(&0x8030_0004));
        assert!(!fixpoint.roots["alias_b"].contains(&0x8030_0004));
    }

    #[test]
    fn induction_admits_a_target_every_alias_agrees_about_for_each_bank() {
        // Same slot, and both images hold the SAME word at the called address.
        // Whichever is resident the call lands on identical bytes, so the
        // entry is recorded in EACH bank's own universe -- never merged.
        let caller = image("caller", 0x8010_0000, &[jal(0x8030_0004), 0x0000_0000]);
        let alias_a = image("alias_a", 0x8030_0000, &[0x0000_0000, 0x27bd_ffe8]);
        let alias_b = image("alias_b", 0x8030_0000, &[0x0000_0000, 0x27bd_ffe8]);
        let catalog = SlotCatalog::new(vec![caller, alias_a, alias_b]);

        let fixpoint = catalog
            .inductive_bank_roots(&BTreeMap::new(), 8)
            .expect("induction converges");
        assert_eq!(fixpoint.rejected_ambiguous, 0);
        assert!(fixpoint.roots["alias_a"].contains(&0x8030_0004));
        assert!(fixpoint.roots["alias_b"].contains(&0x8030_0004));
        // Bank-keyed, so the caller did not inherit the sibling's entry.
        assert!(!fixpoint.roots["caller"].contains(&0x8030_0004));
    }

    #[test]
    fn induction_fails_closed_when_it_cannot_converge() {
        // A one-round budget cannot even complete the first discovery round,
        // so the caller must get None rather than a half-converged set.
        let caller = image("caller", 0x8010_0000, &[jal(0x8010_0004), 0x27bd_ffe8]);
        let catalog = SlotCatalog::new(vec![caller]);
        assert!(catalog.inductive_bank_roots(&BTreeMap::new(), 1).is_none());
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
