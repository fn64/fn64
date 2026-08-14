//! Why does a ROM admit ZERO descriptor tables?
//!
//! 246 of 287 corpus ROMs reach `NoUniqueAdmittedTable { admitted: 0 }` --
//! 16x more than the ambiguous case, and the dominant reason the corpus fails.
//! "No table" has several possible causes and they need different fixes, so
//! this separates them instead of treating them as one bucket:
//!
//!   NO_CANDIDATES     the family search enumerated nothing at all. The
//!                     descriptor SHAPE (stride set, field layout, vram
//!                     window) does not occur in this ROM.
//!   CANDIDATES_NONE_ADMITTED
//!                     candidates exist but none cleared admission, i.e. the
//!                     regions did not decode/map. A search-space question.
//!
//! For the second class it also reports how close the best candidate came, so
//! a floor change can be evaluated against evidence rather than hope.

fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    let config = fn64_discover::overlay_regions::SearchConfig::aki_family();
    let delta_config = fn64_discover::delta_vote::DeltaVoteConfig::default();
    let recovery =
        fn64_discover::overlay_regions::recover_overlay_regions(&rom.bytes, &config, &delta_config, 1);

    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    let candidates = recovery.candidate_tables.len();

    if admitted > 0 {
        println!("ADMITTED_{admitted}\t{label}\tcandidates={candidates}");
        return;
    }
    if candidates == 0 {
        println!("NO_CANDIDATES\t{label}\tcandidates=0");
        return;
    }

    // Candidates exist but none was admitted: report the best one so the
    // binding constraint is visible.
    let best = recovery
        .admissions
        .iter()
        .max_by_key(|admission| (admission.mapped_regions, admission.table.records.len()));
    match best {
        Some(admission) => println!(
            "CANDIDATES_NONE_ADMITTED\t{label}\tcandidates={candidates} \
             best_mapped={} best_records={} best_stride={:#x}",
            admission.mapped_regions,
            admission.table.records.len(),
            admission.table.record_stride,
        ),
        None => println!("CANDIDATES_NONE_ADMITTED\t{label}\tcandidates={candidates} best=none"),
    }
}
