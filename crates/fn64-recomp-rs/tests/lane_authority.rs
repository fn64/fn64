//! Mechanical authority check for the whole-ROM C/Rust lane differential.
//!
//! This test consumes only generated game artifacts supplied by the caller.
//! It never ships those artifacts or assumes a machine-local path.  The C
//! input is N64Recomp's MIT-generated output; the Rust input is this crate's
//! generated whole-ROM output.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Eq, PartialEq)]
struct Body {
    pcs: BTreeSet<u32>,
}

fn instruction_pc(line: &str) -> Option<u32> {
    let comment = line.find("//")?;
    let marker = line[comment..].find("0x")? + comment + 2;
    let hex = line.get(marker..marker + 8)?;
    (line.as_bytes().get(marker + 8) == Some(&b':'))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

fn function_name(line: &str, prefix: &str, suffix: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(prefix)?;
    let end = rest.find(suffix)?;
    let name = &rest[..end];
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric()))
    .then(|| name.to_string())
}

fn brace_delta(line: &str) -> i32 {
    line.bytes().fold(0, |depth, byte| match byte {
        b'{' => depth + 1,
        b'}' => depth - 1,
        _ => depth,
    })
}

fn parse_file(text: &str, language: Language) -> BTreeMap<String, Body> {
    let mut functions = BTreeMap::new();
    let mut current: Option<(String, Body, i32)> = None;

    for line in text.lines() {
        if let Some((_name, body, depth)) = &mut current {
            if let Some(pc) = instruction_pc(line) {
                body.pcs.insert(pc);
            }
            *depth += brace_delta(line);
            if *depth == 0 {
                let (name, body, _) = current.take().expect("current function exists");
                assert!(
                    functions.insert(name.clone(), body).is_none(),
                    "duplicate generated function {name}"
                );
            }
            continue;
        }

        let name = match language {
            Language::C => function_name(line, "RECOMP_FUNC void ", "("),
            Language::Rust => function_name(line, "pub fn ", "("),
        };
        if let Some(name) = name {
            let depth = brace_delta(line);
            assert!(depth > 0, "generated function header has no body: {line}");
            current = Some((name, Body::default(), depth));
        }
    }

    assert!(current.is_none(), "unterminated generated function body");
    functions
}

#[derive(Clone, Copy)]
enum Language {
    C,
    Rust,
}

fn sorted_files(dir: &Path, prefix: &str, extension: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(extension))
        })
        .collect();
    paths.sort();
    paths
}

fn parse_c_dir(dir: &Path) -> BTreeMap<String, Body> {
    let files = sorted_files(dir, "funcs_", ".c");
    assert!(!files.is_empty(), "no funcs_*.c files in {}", dir.display());
    parse_files(&files, Language::C)
}

fn parse_rust_dir(dir: &Path) -> BTreeMap<String, Body> {
    let src = dir.join("src");
    let mut files = sorted_files(&src, "part_", ".rs");
    if files.is_empty() {
        files.push(src.join("lib.rs"));
    }
    assert!(
        files.iter().all(|path| path.is_file()),
        "no generated Rust sources in {}",
        src.display()
    );
    parse_files(&files, Language::Rust)
}

fn parse_files(paths: &[PathBuf], language: Language) -> BTreeMap<String, Body> {
    let mut all = BTreeMap::new();
    for path in paths {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for (name, body) in parse_file(&text, language) {
            assert!(
                all.insert(name.clone(), body).is_none(),
                "duplicate generated function {name} across source files"
            );
        }
    }
    all
}

fn c_missing_bodies<'a>(
    c: &'a BTreeMap<String, Body>,
    rust: &'a BTreeMap<String, Body>,
) -> Vec<&'a str> {
    c.iter()
        .filter_map(|(name, c_body)| {
            let rust_body = rust.get(name)?;
            (c_body.pcs.is_empty() && !rust_body.pcs.is_empty()).then_some(name.as_str())
        })
        .collect()
}

fn shared_pc_mismatches<'a>(
    c: &'a BTreeMap<String, Body>,
    rust: &'a BTreeMap<String, Body>,
) -> Vec<&'a str> {
    c.iter()
        .filter_map(|(name, c_body)| {
            let rust_body = rust.get(name)?;
            (!c_body.pcs.is_empty() && !rust_body.pcs.is_empty() && c_body.pcs != rust_body.pcs)
                .then_some(name.as_str())
        })
        .collect()
}

#[test]
fn parsers_distinguish_real_bodies_from_callable_empty_stubs() {
    let c = parse_file(
        r#"
RECOMP_FUNC void real_body(uint8_t* rdram, recomp_context* ctx) {
    // 0x80000000: nop
    if (ctx->r1) { ctx->r2 = 1; }
}
RECOMP_FUNC void empty_stub(uint8_t* rdram, recomp_context* ctx) {
    uint64_t hi = 0, lo = 0, result = 0;
    int c1cs = 0;
}
"#,
        Language::C,
    );
    let rust = parse_file(
        r#"
pub fn real_body(ctx: &mut RecompContext, mem: &mut Rdram) {
    // 0x80000000: Nop
}
pub fn empty_stub(ctx: &mut RecompContext, mem: &mut Rdram) {
    // 0x80000004: Addiu
}
"#,
        Language::Rust,
    );

    assert_eq!(c_missing_bodies(&c, &rust), ["empty_stub"]);
    assert!(shared_pc_mismatches(&c, &rust).is_empty());
}

/// Run explicitly from `scripts/lane-parity.sh`; game-derived inputs are not
/// available to the ordinary workspace test suite.
#[test]
#[ignore = "requires caller-supplied generated C and Rust whole-ROM artifacts"]
fn generated_lane_authority() {
    let c_dir = PathBuf::from(
        std::env::var_os("RECOMPILED_DIR").expect("RECOMPILED_DIR must name generated C"),
    );
    let rust_dir = PathBuf::from(
        std::env::var_os("RECOMP_RS_DIR").expect("RECOMP_RS_DIR must name generated Rust"),
    );
    let mode = std::env::var("FN64_LANE_AUTHORITY_MODE").unwrap_or_else(|_| "require".into());

    let c = parse_c_dir(&c_dir);
    let rust = parse_rust_dir(&rust_dir);
    let missing = c_missing_bodies(&c, &rust);
    let pc_mismatches = shared_pc_mismatches(&c, &rust);
    let c_empty = c.values().filter(|body| body.pcs.is_empty()).count();
    let shared_nonempty = c
        .iter()
        .filter(|(name, body)| {
            !body.pcs.is_empty() && rust.get(*name).is_some_and(|b| !b.pcs.is_empty())
        })
        .count();

    eprintln!(
        "lane-authority: C functions={} ({c_empty} empty), Rust functions={}, shared nonempty bodies={shared_nonempty}",
        c.len(),
        rust.len()
    );
    eprintln!(
        "lane-authority: callable C empty bodies with nonempty Rust counterparts={}",
        missing.len()
    );
    if !missing.is_empty() {
        let sample = missing
            .iter()
            .take(12)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("lane-authority: missing-body sample: {sample}");
    }
    eprintln!(
        "lane-authority: shared functions with unequal unique instruction-PC sets={}",
        pc_mismatches.len()
    );
    if !pc_mismatches.is_empty() {
        let sample = pc_mismatches
            .iter()
            .take(12)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("lane-authority: PC-set mismatch sample: {sample}");
    }

    assert!(
        pc_mismatches.is_empty(),
        "shared generated functions disagree on their unique instruction-PC sets"
    );
    match mode.as_str() {
        "require" => assert!(
            missing.is_empty(),
            "legacy C lane has callable empty bodies; framebuffer equality cannot establish semantic parity"
        ),
        "observe" => {}
        other => panic!("unknown FN64_LANE_AUTHORITY_MODE={other:?}"),
    }
}
