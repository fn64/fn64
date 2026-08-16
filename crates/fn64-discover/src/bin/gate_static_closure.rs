//! Feasibility gate for function-independent, all-address bank translation.
//!
//! This gate deliberately accepts an externally selected interval. It proves
//! that every aligned word in that interval can receive a bank-qualified
//! dispatch arm and that the generated Rust compiles; it does not promote the
//! interval to executable or claim that discovery recovered its boundaries.

use fn64_discover::normalize;
use fn64_cpu_runtime::BankId;
use fn64_recomp_rs_codegen::{
    classify_bank_words, emit_bank_runner, BankInput, BankWordCatalog, BankWordKind,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn main() {
    if let Err(error) = run() {
        eprintln!("static-closure gate FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let rom_path = args.next().ok_or_else(usage)?;
    let rom_start = parse_u32(&args.next().ok_or_else(usage)?, "rom-start")?;
    let va_start = parse_u32(&args.next().ok_or_else(usage)?, "va-start")?;
    let byte_len = parse_u32(&args.next().ok_or_else(usage)?, "byte-len")?;
    let catalog_only = match args.next().as_deref() {
        None => false,
        Some("--catalog-only") => true,
        Some(_) => return Err(usage()),
    };
    if !rom_start.is_multiple_of(4) || !va_start.is_multiple_of(4) || !byte_len.is_multiple_of(4) {
        return Err("rom-start, va-start, and byte-len must be four-byte aligned".into());
    }
    if byte_len == 0 {
        return Err("byte-len must be nonzero".into());
    }

    let input = std::fs::read(&rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = normalize(&input).map_err(|error| format!("normalizing {rom_path}: {error}"))?;
    let rom_end = rom_start
        .checked_add(byte_len)
        .ok_or("ROM interval overflows u32")?;
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_end as usize)
        .ok_or_else(|| {
            format!(
                "ROM interval [{rom_start:#x},{rom_end:#x}) exceeds normalized image length {:#x}",
                rom.bytes.len()
            )
        })?;
    let words = bank_bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
        .collect::<Vec<_>>();
    let bank_id = stable_bank_id(&rom.sha256, rom_start, va_start, byte_len);

    let catalog = classify_bank_words(&words);
    let straight = catalog
        .iter()
        .filter(|kind| matches!(kind, BankWordKind::Straight))
        .count();
    let control = catalog
        .iter()
        .filter(|kind| matches!(kind, BankWordKind::ControlTransfer))
        .count();
    let unknown = catalog
        .iter()
        .filter(|kind| matches!(kind, BankWordKind::Unknown))
        .count();
    let compact = BankWordCatalog::new(va_start, &words);
    let runs = compact.runs().len();
    println!(
        "  compact catalog: straight={straight}, control_transfer={control}, unknown={unknown}, runs={runs}"
    );
    if catalog_only {
        println!("static-closure catalog scan PASSED (runner generation skipped)");
        return Ok(());
    }

    let generate_started = Instant::now();
    let runner = emit_bank_runner(&BankInput {
        name: "run_static_closure_bank",
        bank: BankId::new(bank_id),
        vram: va_start,
        words: &words,
    });
    let generate_elapsed = generate_started.elapsed();
    let dispatch_arms = runner
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("0x") && trimmed.ends_with("=> {")
        })
        .count();
    if dispatch_arms != words.len() {
        return Err(format!(
            "generated {dispatch_arms} dispatch arms for {} words",
            words.len()
        ));
    }

    let runner_sha256 = format!("{:x}", Sha256::digest(runner.as_bytes()));
    let (source_path, metadata_path) = temporary_paths(&rom.sha256, rom_start, byte_len);
    let source = format!(
        "#![allow(clippy::all, unused)]\nuse fn64_cpu_runtime::{{BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError, Rdram, RecompContext}};\n\n{runner}"
    );
    std::fs::write(&source_path, source.as_bytes())
        .map_err(|error| format!("writing {}: {error}", source_path.display()))?;

    let deps = dependency_directory()?;
    let rlib = current_recomp_rlib(&deps)?;
    let compile_started = Instant::now();
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg("--crate-type=lib")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_cpu_runtime={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--emit=metadata")
        .arg("-o")
        .arg(&metadata_path)
        .output()
        .map_err(|error| format!("invoking rustc: {error}"))?;
    let compile_elapsed = compile_started.elapsed();
    if !compile.status.success() {
        return Err(format!(
            "generated runner did not compile:\nstdout: {}\nstderr: {}\nsource retained at {}",
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr),
            source_path.display()
        ));
    }

    println!("static-closure feasibility gate PASSED");
    println!("  normalized ROM sha256={}", rom.sha256);
    println!(
        "  bank rom=[{rom_start:#010x},{rom_end:#010x}) va=[{va_start:#010x},{:#010x}) bank_id={bank_id:#018x}",
        va_start + byte_len
    );
    println!(
        "  {} bytes / {} aligned PCs / {} dispatch arms",
        byte_len,
        words.len(),
        dispatch_arms
    );
    println!(
        "  generated {} bytes in {:.3}s; runner sha256={runner_sha256}",
        runner.len(),
        generate_elapsed.as_secs_f64()
    );
    println!(
        "  rustc metadata accepted in {:.3}s; source retained at {}",
        compile_elapsed.as_secs_f64(),
        source_path.display()
    );
    Ok(())
}

fn usage() -> String {
    "usage: gate_static_closure <rom> <rom-start> <va-start> <byte-len> [--catalog-only] (numbers accept 0x...)"
        .into()
}

fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    let parsed = value.strip_prefix("0x").unwrap_or(value);
    u32::from_str_radix(parsed, 16).map_err(|error| format!("invalid {name} {value:?}: {error}"))
}

fn stable_bank_id(rom_sha256: &str, rom_start: u32, va_start: u32, byte_len: u32) -> u64 {
    let digest = Sha256::digest(
        format!("fn64:static-closure-gate:v1:{rom_sha256}:{rom_start}:{va_start}:{byte_len}")
            .as_bytes(),
    );
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

fn temporary_paths(rom_sha256: &str, rom_start: u32, byte_len: u32) -> (PathBuf, PathBuf) {
    let stem = format!(
        "fn64-static-closure-{}-{rom_start:08x}-{byte_len:08x}",
        &rom_sha256[..12]
    );
    let root = std::env::temp_dir();
    (
        root.join(format!("{stem}.rs")),
        root.join(format!("{stem}.rmeta")),
    )
}

fn dependency_directory() -> Result<PathBuf, String> {
    let executable_dir = std::env::current_exe()
        .map_err(|error| format!("finding gate executable: {error}"))?
        .parent()
        .ok_or("gate executable has no parent directory")?
        .to_path_buf();
    Ok(if executable_dir.ends_with("deps") {
        executable_dir
    } else {
        executable_dir.join("deps")
    })
}

fn current_recomp_rlib(deps: &Path) -> Result<PathBuf, String> {
    std::fs::read_dir(deps)
        .map_err(|error| format!("reading target dependency directory: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libfn64_cpu_runtime-") && name.ends_with(".rlib")
                })
        })
        .max_by_key(|path| {
            path.metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .ok_or_else(|| "fn64_cpu_runtime rlib is missing beside gate executable".into())
}
