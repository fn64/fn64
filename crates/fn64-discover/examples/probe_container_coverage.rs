//! Report container-compression coverage for a ROM.
fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let coverage = fn64_discover::container_coverage::measure_container_coverage(&rom.bytes);
    let detail: Vec<String> = coverage
        .streams
        .iter()
        .map(|entry| {
            format!(
                "{:?}={}{}",
                entry.scheme,
                entry.stream_count,
                if entry.scheme.is_decodable_here() { "" } else { "!" },
            )
        })
        .collect();
    let class = if coverage.total_streams == 0 {
        "NO_CONTAINERS"
    } else if coverage.has_undecodable_content() {
        "HAS_UNDECODABLE"
    } else {
        "ALL_DECODABLE"
    };
    println!("{class}\t{label}\tstreams={} {}", coverage.total_streams, detail.join(" "));
}
