//! Does the ledger's code test actually detect code?
//!
//! The ledger calls its residue `structured_data` -- a positive claim that a
//! span is content but not code. That claim is only worth anything if the code
//! test would have fired had code been there. This measures exactly that,
//! against extents each ROM's own dmadata declares to be its `boot` file.
//!
//! Without this, "structured_data" would be indistinguishable from "my detector
//! is broken", which is the failure mode that nearly fooled me: running the
//! same test over IPL3's full 1 MiB copy window shows 2% on OoT, because only
//! ~25 KB of that window is actually the boot file. The extents below are the
//! real ones.

use fn64_discover::delta_vote::{infer_region_delta, DeltaVoteConfig};

const CHUNK: usize = 0x2000;

fn sensitivity(bytes: &[u8], lo: usize, hi: usize) -> (usize, usize) {
    let config = DeltaVoteConfig::default();
    let (mut fired, mut total) = (0, 0);
    let mut offset = lo;
    while offset + 0x400 < hi {
        let end = (offset + CHUNK).min(hi);
        let scan = infer_region_delta(&bytes[offset..end], offset as u32, &[], &config).scan;
        total += 1;
        if scan.prologue_sites > 0 && scan.jal_sites > 0 {
            fired += 1;
        }
        offset = end;
    }
    (fired, total)
}

#[test]
fn the_code_test_fires_on_known_code() {
    // (name, env var, boot-file extent from the ROM's own dmadata)
    let corpus = [
        ("OoT", "FN64_DISCOVER_OOT_ROM", 0x1060usize, 0x7430usize),
        ("Majora's Mask", "FN64_DISCOVER_MM_ROM", 0x1060, 0x1a500),
    ];
    let mut checked = 0;
    for (name, var, lo, hi) in corpus {
        let Some(path) = std::env::var_os(var) else {
            eprintln!("skip {name}: {var} unset");
            continue;
        };
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{var}: {e}"));
        let (fired, total) = sensitivity(&bytes, lo, hi);
        assert!(total > 0, "{name}: no chunks examined");
        let percent = 100 * fired / total;
        assert!(
            percent >= 90,
            "{name}: code test fired on only {fired}/{total} ({percent}%) of known code; \
             the ledger's `structured_data` class would no longer be defensible"
        );
        eprintln!("{name}: {fired}/{total} known-code chunks detected ({percent}%)");
        checked += 1;
    }
    eprintln!("measured code-test sensitivity on {checked} ROM(s)");
}
