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

    let sites: Vec<usize> = (0..rom.bytes.len().saturating_sub(4))
        .step_by(4)
        .filter(|&offset| rom.bytes[offset..offset + 4] == [0x03, 0xe0, 0x00, 0x08])
        .collect();

    if sites.is_empty() {
        println!("NO_CODE_FOUND\t{label}\tjr_ra=0");
        return;
    }

    // Cluster into spans: a gap larger than GAP_BYTES starts a new span. The
    // threshold is deliberately generous -- a 256 KiB hole inside one bank of
    // code is normal (data, tables, padding), while a genuine overlay region
    // sits much further out.
    const GAP_BYTES: usize = 0x40000;
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let (mut span_start, mut span_end, mut count) = (sites[0], sites[0], 0usize);
    for &site in &sites {
        if site - span_end > GAP_BYTES {
            spans.push((span_start, span_end, count));
            span_start = site;
            count = 0;
        }
        span_end = site;
        count += 1;
    }
    spans.push((span_start, span_end, count));
    spans.sort_by_key(|&(_, _, count)| std::cmp::Reverse(count));

    let total = sites.len();
    let largest = spans[0].2;
    let concentration = largest as f64 / total as f64;

    // >=95% in one span: the ROM is single-bank. No overlay geometry exists to
    // recover, so a BootBankOnly verdict here is COMPLETE, not a miss.
    let class = if concentration >= 0.95 {
        "SINGLE_BANK"
    } else if concentration >= 0.80 {
        "MOSTLY_SINGLE_BANK"
    } else {
        "MULTI_SPAN"
    };

    println!(
        "{class}\t{label}\tjr_ra={total} spans={} concentration={concentration:.2} \
         largest=[{:#x},{:#x})",
        spans.len(),
        spans[0].0,
        spans[0].1,
    );
}
