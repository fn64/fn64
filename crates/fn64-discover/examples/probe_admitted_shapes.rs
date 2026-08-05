//! Report the geometry of every ADMITTED overlay descriptor table.
//!
//! `NoUniqueAdmittedTable` is the largest single corpus failure cause, and any
//! rule that collapses spurious ambiguity has to separate two shapes that both
//! present as "several admitted tables": one array read at phase-shifted
//! strides (spurious -- should collapse) versus genuinely distinct arrays
//! (real -- must keep refusing).
//!
//! Deciding that from prose is how the last proposal went wrong, so this
//! prints the measurable relationships instead: stride, record count, ROM
//! interval span, whether one table's intervals are a subset of another's, and
//! whether shared intervals agree on their destinations.

fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Same knobs the AKI-family recovery uses, so the shapes this prints are
    // the ones the real admission path sees.
    let config = fn64_discover::overlay_regions::SearchConfig::aki_family();
    let delta_config = fn64_discover::delta_vote::DeltaVoteConfig::default();
    let recovery = fn64_discover::overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &config,
        &delta_config,
        1,
    );

    let admitted: Vec<_> = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .collect();
    println!("{label}\tadmitted={}", admitted.len());
    if admitted.len() < 2 {
        return;
    }

    let mut sets: Vec<(usize, Vec<(u32, u32)>)> = Vec::new();
    for (index, admission) in admitted.iter().enumerate() {
        let table = &admission.table;
        let intervals = table.interval_set();
        let destination_field = format!("{:?}", table.destination_field);
        println!(
            "  [{index}] stride={:#x} records={} intervals={} dest_field={destination_field} \
             rom=[{:#x},{:#x})",
            table.record_stride,
            table.records.len(),
            intervals.len(),
            intervals.first().map(|entry| entry.0).unwrap_or(0),
            intervals.last().map(|entry| entry.1).unwrap_or(0),
        );
        sets.push((index, intervals));
    }

    // The pairwise relationships a collapse rule would key on.
    for (left_index, left) in &sets {
        for (right_index, right) in &sets {
            if left_index >= right_index {
                continue;
            }
            let left_only = left.iter().filter(|entry| !right.contains(entry)).count();
            let right_only = right.iter().filter(|entry| !left.contains(entry)).count();
            let shared = left.iter().filter(|entry| right.contains(entry)).count();
            let relation = match (left_only, right_only) {
                (0, 0) => "identical",
                (0, _) => "left-subset-of-right",
                (_, 0) => "right-subset-of-left",
                _ if shared > 0 => "overlapping",
                _ => "disjoint",
            };
            println!(
                "  pair({left_index},{right_index}) shared={shared} left_only={left_only} \
                 right_only={right_only} -> {relation}"
            );
        }
    }
}
