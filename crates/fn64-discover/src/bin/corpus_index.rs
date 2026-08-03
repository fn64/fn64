//! Persist and query [`corpus_homology`](fn64_discover::corpus_homology)'s
//! cross-ROM function-identity closure at DISCOVERY time, not only inside a
//! grading gate ([`gate_corpus_homology`](../gate_corpus_homology.rs)).
//!
//! [`gate_corpus_homology`] runs [`build_corpus`] once, grades it held-out
//! against dumps, and discards the result. This binary instead makes the
//! SAME closure (identical pairwise engines, identical
//! transitive-uniqueness-and-corroboration guard — no similarity threshold
//! is ever added here) a durable, queryable [`CorpusIndex`]: build it once,
//! extend it with one new ROM at a time via [`extend_corpus`] (never
//! re-matching pairs already folded in), and query which of one ROM's
//! functions carry a corpus identity from another ROM.
//!
//! ROMs are assembled into [`CorpusRom`]s through the exact same
//! `dump.toml`-loading path `gate_corpus_homology` uses
//! ([`dump_toml::load_rom`]) — this binary does not reimplement that
//! assembly, and it does not reimplement an edge decision: [`build`] and
//! [`extend`] call straight into [`CorpusIndex::build`]/[`extend_corpus`].
//!
//! # Subcommands
//!
//! ```text
//! corpus_index build  --index PATH --rom LABEL=ROM_PATH:DUMP_PATH [--rom ...]
//! corpus_index extend --index PATH --rom LABEL=ROM_PATH:DUMP_PATH
//! corpus_index query  --index PATH --rom LABEL [--rom LABEL=ROM_PATH:DUMP_PATH ...]
//! ```
//!
//! `build` and `extend` need a ROM's prior, independently-derived function
//! boundaries, which (matching `gate_corpus_homology`'s own documented
//! frontier) only a `dump.toml` currently supplies here — a ROM with no
//! dump has no boundary source this binary is allowed to invent.
//!
//! `query`'s `--rom LABEL=ROM_PATH:DUMP_PATH` form is optional and used only
//! to supply the held-out real-name annotation on OTHER ROMs' members of an
//! identity (never as matcher input, and never required for `--rom LABEL`
//! alone to report which VAs of `LABEL` carry an identity).
//!
//! `query` and `extend` fail closed on a stale index: before trusting any
//! ROM already present under a label, the freshly-normalized SHA-256 is
//! checked against the index's recorded entry ([`CorpusIndex::verify_sha`]).

use fn64_discover::corpus_homology::{
    extend_corpus, CorpusConfig, CorpusIndex, CorpusIndexError, NewCorpusRom,
};
use fn64_discover::dump_toml;
use std::collections::BTreeMap;

fn main() {
    if let Err(error) = run() {
        eprintln!("corpus_index: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let subcommand = args
        .next()
        .ok_or_else(|| "usage: corpus_index <build|extend|query> ...".to_string())?;
    let rest: Vec<String> = args.collect();
    match subcommand.as_str() {
        "build" => run_build(&rest),
        "extend" => run_extend(&rest),
        "query" => run_query(&rest),
        other => Err(format!(
            "unknown subcommand {other:?} (expected build, extend, or query)"
        )),
    }
}

/// One `--rom LABEL=ROM_PATH:DUMP_PATH` argument, parsed.
struct RomArg {
    label: String,
    rom_path: String,
    dump_path: String,
}

fn parse_rom_arg(raw: &str) -> Result<RomArg, String> {
    let (label, rest) = raw
        .split_once('=')
        .ok_or_else(|| format!("--rom {raw:?} must be LABEL=ROM_PATH:DUMP_PATH"))?;
    let (rom_path, dump_path) = rest
        .split_once(':')
        .ok_or_else(|| format!("--rom {raw:?} must be LABEL=ROM_PATH:DUMP_PATH"))?;
    if label.is_empty() {
        return Err(format!("--rom {raw:?} has an empty label"));
    }
    Ok(RomArg {
        label: label.to_string(),
        rom_path: rom_path.to_string(),
        dump_path: dump_path.to_string(),
    })
}

/// Parse `--flag value` pairs plus repeatable `--rom` args from a flat
/// argument list. Unknown flags are a loud error, never silently ignored.
struct Flags {
    index: Option<String>,
    roms: Vec<RomArg>,
    /// `query`'s single `--rom LABEL` (no `=`) target.
    query_label: Option<String>,
}

fn parse_flags(args: &[String], allow_bare_rom_label: bool) -> Result<Flags, String> {
    let mut index = None;
    let mut roms = Vec::new();
    let mut query_label = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                let value = args
                    .get(i + 1)
                    .ok_or("--index requires a path")?
                    .to_string();
                index = Some(value);
                i += 2;
            }
            "--rom" => {
                let value = args.get(i + 1).ok_or("--rom requires a value")?;
                if !value.contains('=') {
                    if !allow_bare_rom_label {
                        return Err(format!(
                            "--rom {value:?} must be LABEL=ROM_PATH:DUMP_PATH"
                        ));
                    }
                    if query_label.is_some() {
                        return Err("only one bare --rom LABEL is allowed (query target)".into());
                    }
                    query_label = Some(value.clone());
                } else {
                    roms.push(parse_rom_arg(value)?);
                }
                i += 2;
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    Ok(Flags {
        index,
        roms,
        query_label,
    })
}

fn require_index_path(flags: &Flags) -> Result<&str, String> {
    flags.index.as_deref().ok_or_else(|| "--index PATH is required".to_string())
}

fn load_new_rom(arg: &RomArg) -> Result<NewCorpusRom, String> {
    if !std::path::Path::new(&arg.rom_path).exists() {
        return Err(format!("{} ROM {} does not exist", arg.label, arg.rom_path));
    }
    let bytes = std::fs::read(&arg.rom_path)
        .map_err(|error| format!("reading {}: {error}", arg.rom_path))?;
    let normalized =
        fn64_discover::normalize(&bytes).map_err(|error| format!("{} ROM: {error}", arg.label))?;
    let loaded = dump_toml::load_rom(&arg.label, &arg.rom_path, &arg.dump_path)?;
    Ok(NewCorpusRom {
        label: loaded.label,
        sha256: normalized.sha256,
        functions: loaded.functions,
    })
}

fn read_index(path: &str) -> Result<CorpusIndex, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("reading index {path}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("parsing index {path}: {error}"))
}

fn write_index(path: &str, index: &CorpusIndex) -> Result<(), String> {
    let json = serde_json::to_string_pretty(index)
        .map_err(|error| format!("serializing index: {error}"))?;
    std::fs::write(path, json).map_err(|error| format!("writing index {path}: {error}"))
}

fn run_build(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, false)?;
    let index_path = require_index_path(&flags)?;
    if flags.roms.is_empty() {
        return Err("build requires at least one --rom LABEL=ROM_PATH:DUMP_PATH".to_string());
    }

    let mut new_roms = Vec::with_capacity(flags.roms.len());
    for arg in &flags.roms {
        let rom = load_new_rom(arg)?;
        println!(
            "  rom {label} functions={fns} sha256={sha}",
            label = rom.label,
            fns = rom.functions.len(),
            sha = &rom.sha256[..16],
        );
        new_roms.push(rom);
    }

    let index = CorpusIndex::build(&new_roms, CorpusConfig::default())
        .map_err(|error| format!("build rejected: {error}"))?;
    let report = index.report();
    println!(
        "corpus_index build: roms={} total_functions={} pairwise_edges={} identities={} max_span={} singletons={} ambiguous={}",
        index.rom_labels().len(),
        report.total_functions,
        report.pairwise_edges,
        report.identity_count,
        report.max_span,
        report.singletons,
        report.ambiguous.len(),
    );
    write_index(index_path, &index)?;
    println!("  wrote {index_path}");
    Ok(())
}

fn run_extend(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, false)?;
    let index_path = require_index_path(&flags)?;
    let arg = match flags.roms.as_slice() {
        [single] => single,
        [] => return Err("extend requires exactly one --rom LABEL=ROM_PATH:DUMP_PATH".to_string()),
        _ => return Err("extend accepts exactly one --rom at a time".to_string()),
    };

    let index = read_index(index_path)?;
    // Fail closed: if this label is already indexed, its bytes must match
    // exactly what the closure was computed from — extend_corpus itself
    // additionally refuses ANY re-add of an existing label outright, but
    // verifying here gives a precise "stale ROM" diagnosis rather than a
    // bare "duplicate label" when that's what actually happened.
    let new_rom = load_new_rom(arg)?;
    if let Some(existing_sha) = index.sha256_of(&new_rom.label) {
        if existing_sha != new_rom.sha256 {
            return Err(format!(
                "{}",
                CorpusIndexError::ShaMismatch {
                    label: new_rom.label.clone(),
                    indexed: existing_sha.to_string(),
                    found: new_rom.sha256.clone(),
                }
            ));
        }
        return Err(format!(
            "{}",
            CorpusIndexError::DuplicateRomLabel(new_rom.label.clone())
        ));
    }

    println!(
        "  rom {label} functions={fns} sha256={sha}",
        label = new_rom.label,
        fns = new_rom.functions.len(),
        sha = &new_rom.sha256[..16],
    );

    let extended = extend_corpus(index, new_rom, CorpusConfig::default())
        .map_err(|error| format!("extend rejected: {error}"))?;
    let report = extended.report();
    println!(
        "corpus_index extend: roms={} total_functions={} pairwise_edges={} identities={} max_span={} singletons={} ambiguous={}",
        extended.rom_labels().len(),
        report.total_functions,
        report.pairwise_edges,
        report.identity_count,
        report.max_span,
        report.singletons,
        report.ambiguous.len(),
    );
    write_index(index_path, &extended)?;
    println!("  wrote {index_path}");
    Ok(())
}

fn run_query(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, true)?;
    let index_path = require_index_path(&flags)?.to_string();
    let target = flags
        .query_label
        .ok_or_else(|| "query requires --rom LABEL (the ROM to query)".to_string())?;

    let index = read_index(&index_path)?;

    // Any additional --rom LABEL=ROM_PATH:DUMP_PATH args are used ONLY to
    // supply the held-out real-name annotation; they are re-verified against
    // the index's recorded SHA-256 for their label (if present) so an
    // annotation never comes from a byte-different ROM than the closure used.
    let mut real_name_by_rom: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for arg in &flags.roms {
        let loaded = dump_toml::load_rom(&arg.label, &arg.rom_path, &arg.dump_path)?;
        if let Some(existing_sha) = index.sha256_of(&arg.label) {
            let bytes = std::fs::read(&arg.rom_path)
                .map_err(|error| format!("reading {}: {error}", arg.rom_path))?;
            let normalized = fn64_discover::normalize(&bytes)
                .map_err(|error| format!("{} ROM: {error}", arg.label))?;
            if normalized.sha256 != existing_sha {
                return Err(format!(
                    "{}",
                    CorpusIndexError::ShaMismatch {
                        label: arg.label.clone(),
                        indexed: existing_sha.to_string(),
                        found: normalized.sha256,
                    }
                ));
            }
        }
        real_name_by_rom.insert(loaded.label, loaded.real_name_by_va);
    }

    if !index.rom_labels().contains(&target.as_str()) {
        return Err(format!(
            "{target:?} is not in the index (present: {:?})",
            index.rom_labels()
        ));
    }

    let entries = index.identities_for(&target, &real_name_by_rom);
    println!(
        "corpus_index query: rom={target} functions_with_identity={}",
        entries.len()
    );
    for entry in &entries {
        let shared: Vec<String> = entry
            .shared_with
            .iter()
            .map(|s| {
                if let Some(name) = &s.real_name {
                    format!("{}:{}({name})", s.rom_label, s.identity)
                } else {
                    format!("{}:{}", s.rom_label, s.identity)
                }
            })
            .collect();
        println!(
            "  va=0x{:08x} identity={} shared_with={}",
            entry.va_start,
            entry.identity,
            shared.join(",")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rom_arg_splits_label_path_dump() {
        let arg = parse_rom_arg("NWXE=/roms/nwxe.z64:/dumps/nwxe.toml").unwrap();
        assert_eq!(arg.label, "NWXE");
        assert_eq!(arg.rom_path, "/roms/nwxe.z64");
        assert_eq!(arg.dump_path, "/dumps/nwxe.toml");
    }

    #[test]
    fn parse_rom_arg_rejects_missing_equals() {
        assert!(parse_rom_arg("NWXE/roms/nwxe.z64:/dumps/nwxe.toml").is_err());
    }

    #[test]
    fn parse_rom_arg_rejects_missing_colon() {
        assert!(parse_rom_arg("NWXE=/roms/nwxe.z64").is_err());
    }

    #[test]
    fn parse_rom_arg_rejects_empty_label() {
        assert!(parse_rom_arg("=/roms/nwxe.z64:/dumps/nwxe.toml").is_err());
    }

    #[test]
    fn parse_flags_collects_repeated_rom_args() {
        let args: Vec<String> = [
            "--index",
            "/tmp/idx.json",
            "--rom",
            "A=/a.z64:/a.toml",
            "--rom",
            "B=/b.z64:/b.toml",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let flags = parse_flags(&args, false).unwrap();
        assert_eq!(flags.index.as_deref(), Some("/tmp/idx.json"));
        assert_eq!(flags.roms.len(), 2);
        assert_eq!(flags.roms[0].label, "A");
        assert_eq!(flags.roms[1].label, "B");
    }

    #[test]
    fn parse_flags_allows_a_single_bare_rom_label_when_permitted() {
        let args: Vec<String> = ["--index", "/tmp/idx.json", "--rom", "NWXE"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let flags = parse_flags(&args, true).unwrap();
        assert_eq!(flags.query_label.as_deref(), Some("NWXE"));
        assert!(flags.roms.is_empty());
    }

    #[test]
    fn parse_flags_rejects_bare_rom_label_when_not_permitted() {
        let args: Vec<String> = ["--index", "/tmp/idx.json", "--rom", "NWXE"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse_flags(&args, false).is_err());
    }

    #[test]
    fn parse_flags_rejects_unknown_flag() {
        let args: Vec<String> = ["--bogus", "value"].iter().map(|s| s.to_string()).collect();
        assert!(parse_flags(&args, false).is_err());
    }
}
