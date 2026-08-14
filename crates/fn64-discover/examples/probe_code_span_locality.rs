//! Does a ROM's executable code live in ONE resident span, or several?
//!
//! `DiscoveryStrategy::BootBankOnly` is a single verdict covering two very
//! different ROMs: one that has no overlays at all (nothing to find), and one
//! whose overlays this build cannot recover (something missed). Planning
//! treats those identically today, which is why "229 ROMs need a new
//! descriptor family" over-counted the reachable work by 4x.
//!
//! `JR RA` (`0x03e00008`) is the cheapest structural proxy for "code is here":
//! every non-leaf MIPS function ends with one, and it is a full 32-bit pattern
//! so false positives in data are rare. If nearly all of them sit in one
//! leading span, the ROM is single-bank and no descriptor family can unlock
//! it -- there is nothing to describe.
//!
//! This measures a PROPERTY OF THE ROM, not of the current search, so it is
//! worth recording once per ROM rather than recomputing per investigation.

fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let label = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    // The measurement now lives in the library so discovery itself can emit it;
    // this probe just prints what the library computes, so the corpus TSV and
    // any future fact cannot drift apart.
    let locality = fn64_discover::code_span_locality::measure_code_span_locality(&rom.bytes);
    let class = match locality.class {
        fn64_discover::code_span_locality::CodeSpanClass::NoCodeFound => "NO_CODE_FOUND",
        fn64_discover::code_span_locality::CodeSpanClass::SingleBank => "SINGLE_BANK",
        fn64_discover::code_span_locality::CodeSpanClass::MostlySingleBank => "MOSTLY_SINGLE_BANK",
        fn64_discover::code_span_locality::CodeSpanClass::MultiSpan => "MULTI_SPAN",
    };
    match locality.largest_span {
        Some((start, end)) => println!(
            "{class}\t{label}\tjr_ra={} spans={} concentration={:.2} largest=[{start:#x},{end:#x})",
            locality.jr_ra_sites, locality.span_count, locality.largest_span_concentration,
        ),
        None => println!("{class}\t{label}\tjr_ra=0"),
    }
}
