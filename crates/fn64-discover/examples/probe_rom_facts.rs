//! Emit every per-ROM metadata fact fn64 records, as one TSV row.
//!
//! Three facts, deliberately of two different kinds:
//!
//!   code_span   PERMANENT property of the image -- does not move as fn64
//!               improves, so it bounds what is even findable.
//!   table       CURRENT BUILD capability -- moves as families are added.
//!   containers  permanent, but decodability is a build property.
//!
//! Reading them together is the point: a ROM that is single-bank AND found no
//! candidates is complete; one that is multi-span and found none is a miss.
fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(rom) = fn64_discover::rom::normalize(&bytes) else {
        println!("{label}\tREJECTED\t-\t-\t-\t-\t-\t-");
        return;
    };

    let locality = fn64_discover::code_span_locality::measure_code_span_locality(&rom.bytes);
    let coverage = fn64_discover::container_coverage::measure_container_coverage(&rom.bytes);

    let config = fn64_discover::overlay_regions::SearchConfig::aki_family();
    let delta_config = fn64_discover::delta_vote::DeltaVoteConfig::default();
    let recovery = fn64_discover::overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &config,
        &delta_config,
        1,
    );
    let candidates = recovery.candidate_tables.len();
    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .count();
    let table = fn64_discover::TableFamilySearchOutcome::classify(candidates, admitted);

    let schemes: Vec<String> = coverage
        .streams
        .iter()
        .map(|entry| format!("{:?}:{}", entry.scheme, entry.stream_count))
        .collect();

    println!(
        "{label}\t{:?}\t{:.2}\t{:?}\t{}\t{}\t{}\t{}",
        locality.class,
        locality.largest_span_concentration,
        table,
        candidates,
        admitted,
        coverage.total_streams,
        if schemes.is_empty() { "-".to_string() } else { schemes.join(",") },
    );
}
