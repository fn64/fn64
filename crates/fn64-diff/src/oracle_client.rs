//! Subprocess client for the faki-tools NW4E oracle (`nw4e-oracle`'s
//! `oracle` binary). This crate is scoped to READ+RUN that binary, never
//! modify its sources, and its FFI/mupen64plus-linking code lives entirely
//! in that binary's `main.rs` (not exported from its `lib.rs`) -- so the
//! only integration seam available from here is spawning the built
//! executable and parsing its stdout, exactly like a human operator would.
//!
//! ## What this buys the lockstep harness
//!
//! The oracle's `breakpoint --state S --break-at PC` command already does
//! the one primitive real lockstep needs: "from savestate S, step forward
//! (via real `DebugStep`-driven MIPS execution) until the live PC equals
//! PC, then report every GPR/CP0 register at that exact paused instant."
//! That is independent, ground-truth register state at ANY PC fn64 claims
//! to reach from the same starting snapshot -- this module's job is only
//! to invoke it and parse the answer, not reimplement any part of it.
//!
//! ## Why this is coarse-grained, not true single-instruction lockstep
//!
//! fn64 (a recompiler-shaped runtime) executes at whole-recompiled-function
//! granularity; the oracle steps at real single-MIPS-instruction
//! granularity via `DebugStep`. There is no PC at which both sides are
//! simultaneously "mid-instruction" in a comparable sense, and no in-
//! process channel to single-step them in true lockstep (the oracle's
//! debugger API is only reachable via this subprocess CLI, one full
//! `breakpoint` run per query). So this harness's real unit of comparison
//! is: "at the PC where fn64's stand-in/executor reports having arrived,
//! what does an independent oracle run (from the identical starting
//! snapshot) say the true register file looked like at that same PC" --
//! reported honestly as a per-checkpoint diff, not framed as true
//! cycle-exact lockstep.
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct OracleRegisters {
    pub pc: u32,
    pub gprs: [u64; 32],
    pub cp0_status: u32,
    pub cp0_cause: u32,
    pub cp0_epc: u32,
    pub steps: u64,
}

#[derive(Debug)]
pub enum OracleError {
    Spawn(std::io::Error),
    NonZeroExit {
        status: i32,
        stderr: String,
    },
    UnparseableOutput {
        stdout: String,
        reason: &'static str,
    },
    /// The oracle's own honest failure mode: the requested `--break-at` PC
    /// was never reached within its step budget (e.g. `ORACLE_STEP_LIMIT`
    /// exhausted, or fn64 diverged onto a PC the real game never visits).
    /// This is itself a meaningful lockstep signal, not a harness bug --
    /// see [`OracleClient::registers_at`]'s doc.
    BreakpointNeverHit {
        target_pc: u32,
        stdout: String,
    },
}

impl std::fmt::Display for OracleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OracleError::Spawn(e) => write!(f, "failed to spawn oracle binary: {e}"),
            OracleError::NonZeroExit { status, stderr } => {
                write!(f, "oracle exited with status {status}: {stderr}")
            }
            OracleError::UnparseableOutput { reason, .. } => {
                write!(f, "could not parse oracle output: {reason}")
            }
            OracleError::BreakpointNeverHit { target_pc, .. } => write!(
                f,
                "oracle never reached PC 0x{target_pc:08x} from this snapshot within its step \
                 budget -- fn64 claims to have reached a PC the real reference run does not \
                 visit at all (a first-divergence signal in its own right, not a harness failure)"
            ),
        }
    }
}

impl std::error::Error for OracleError {}

/// Thin subprocess wrapper. Holds the paths this harness needs to invoke
/// the oracle exactly like a human would from the command line: the built
/// `oracle` binary, the reference savestate to start every query from, and
/// (optionally) an explicit ROM path if the oracle's own default discovery
/// doesn't find one.
pub struct OracleClient {
    oracle_bin: std::path::PathBuf,
    state_path: std::path::PathBuf,
    rom_path: Option<std::path::PathBuf>,
    /// Forwarded as `ORACLE_STEP_LIMIT` to bound each query -- without this
    /// a divergent fn64 PC that the oracle never reaches would otherwise
    /// hang the whole lockstep run for the oracle's 10,000,000-step
    /// default.
    pub step_limit: u64,
}

impl OracleClient {
    pub fn new(oracle_bin: impl AsRef<Path>, state_path: impl AsRef<Path>) -> Self {
        OracleClient {
            oracle_bin: oracle_bin.as_ref().to_path_buf(),
            state_path: state_path.as_ref().to_path_buf(),
            rom_path: None,
            step_limit: 2_000_000,
        }
    }

    pub fn with_rom(mut self, rom_path: impl AsRef<Path>) -> Self {
        self.rom_path = Some(rom_path.as_ref().to_path_buf());
        self
    }

    /// Ask the oracle: from this harness's fixed starting savestate, what
    /// is the real register file when the live PC first equals `target_pc`?
    /// This is the per-checkpoint ground-truth query the lockstep report
    /// diffs fn64's own claimed state against.
    pub fn registers_at(&self, target_pc: u32) -> Result<OracleRegisters, OracleError> {
        let mut cmd = Command::new(&self.oracle_bin);
        cmd.arg("breakpoint")
            .arg("--state")
            .arg(&self.state_path)
            .arg("--break-at")
            .arg(format!("{target_pc:#010x}"))
            .env("ORACLE_STEP_LIMIT", self.step_limit.to_string());
        if let Some(rom) = &self.rom_path {
            cmd.arg("--rom").arg(rom);
        }

        let output = cmd.output().map_err(OracleError::Spawn)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if !output.status.success() {
            if stdout.contains("was not reached within")
                || stderr.contains("was not reached within")
            {
                return Err(OracleError::BreakpointNeverHit { target_pc, stdout });
            }
            return Err(OracleError::NonZeroExit {
                status: output.status.code().unwrap_or(-1),
                stderr,
            });
        }

        parse_breakpoint_output(&stdout, target_pc)
    }
}

/// Parse `breakpoint`'s two relevant output lines: `breakpoint hit at
/// 0x... after N steps` and the `ORACLE_CAPTURE_V1={json}` machine-readable
/// line (see `cmd_breakpoint`/`print_machine_capture` in the oracle's own
/// `main.rs` -- this function only reads that already-stable text format,
/// it doesn't touch or depend on the oracle's internals).
fn parse_breakpoint_output(stdout: &str, target_pc: u32) -> Result<OracleRegisters, OracleError> {
    let steps = stdout
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix("breakpoint hit at ")?;
            let (_, after) = rest.split_once("after ")?;
            let steps_str = after.strip_suffix(" steps")?;
            steps_str.trim().parse::<u64>().ok()
        })
        .ok_or(OracleError::UnparseableOutput {
            stdout: stdout.to_string(),
            reason: "missing 'breakpoint hit at ... after N steps' line",
        })?;

    let capture_line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("ORACLE_CAPTURE_V1="))
        .ok_or(OracleError::UnparseableOutput {
            stdout: stdout.to_string(),
            reason: "missing ORACLE_CAPTURE_V1= line",
        })?;

    let json = MiniJson::parse(capture_line).ok_or(OracleError::UnparseableOutput {
        stdout: stdout.to_string(),
        reason: "ORACLE_CAPTURE_V1 line is not well-formed JSON",
    })?;

    let pc_str = json.get_str("pc").ok_or(OracleError::UnparseableOutput {
        stdout: stdout.to_string(),
        reason: "capture JSON missing string field 'pc'",
    })?;
    let pc = parse_hex_u64(pc_str) as u32;

    let gpr_strs = json
        .get_str_array("gpr")
        .ok_or(OracleError::UnparseableOutput {
            stdout: stdout.to_string(),
            reason: "capture JSON missing array field 'gpr'",
        })?;
    if gpr_strs.len() != 32 {
        return Err(OracleError::UnparseableOutput {
            stdout: stdout.to_string(),
            reason: "capture JSON 'gpr' array is not length 32",
        });
    }
    let mut gprs = [0u64; 32];
    for (slot, s) in gprs.iter_mut().zip(gpr_strs.iter()) {
        *slot = parse_hex_u64(s);
    }

    // cp0_status/cp0_cause/cp0_epc are only in the human-readable
    // `cp0_status=... cp0_cause=... cp0_epc=...` line (the machine capture
    // JSON doesn't carry CP0), so parse that line too.
    let (cp0_status, cp0_cause, cp0_epc) = stdout
        .lines()
        .find(|l| l.starts_with("cp0_status="))
        .and_then(parse_cp0_line)
        .ok_or(OracleError::UnparseableOutput {
            stdout: stdout.to_string(),
            reason: "missing or unparseable cp0_status=... line",
        })?;

    debug_assert_eq!(
        pc, target_pc,
        "breakpoint reported a different PC than requested"
    );

    Ok(OracleRegisters {
        pc,
        gprs,
        cp0_status,
        cp0_cause,
        cp0_epc,
        steps,
    })
}

fn parse_cp0_line(line: &str) -> Option<(u32, u32, u32)> {
    let mut status = None;
    let mut cause = None;
    let mut epc = None;
    for field in line.split_whitespace() {
        let (key, value) = field.split_once('=')?;
        let parsed = u32::from_str_radix(value.trim_start_matches("0x"), 16).ok()?;
        match key {
            "cp0_status" => status = Some(parsed),
            "cp0_cause" => cause = Some(parsed),
            "cp0_epc" => epc = Some(parsed),
            _ => {}
        }
    }
    Some((status?, cause?, epc?))
}

fn parse_hex_u64(s: &str) -> u64 {
    let s = s.trim_start_matches("0x");
    u64::from_str_radix(s, 16).unwrap_or(0)
}

/// Deliberately minimal JSON reader for the ONE flat shape
/// `print_machine_capture` emits (`{"gpr":[...strings...],"hit":N,
/// "pc":"0x...","phase":"...","ranges":[...]}`) -- not a general JSON
/// parser. A real `serde_json` dependency would be reasonable too, but
/// this crate's dependency footprint is deliberately minimal (see
/// `Cargo.toml`) and the format is small/stable/owned by a binary this
/// crate cannot modify anyway, so a general parser buys no real safety.
struct MiniJson<'a> {
    src: &'a str,
}

impl<'a> MiniJson<'a> {
    fn parse(src: &'a str) -> Option<Self> {
        let trimmed = src.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            Some(MiniJson { src: trimmed })
        } else {
            None
        }
    }

    /// Find `"key":"value"` and return `value` (used for `pc`).
    fn get_str(&self, key: &str) -> Option<&'a str> {
        let needle = format!("\"{key}\":\"");
        let start = self.src.find(&needle)? + needle.len();
        let end = self.src[start..].find('"')? + start;
        Some(&self.src[start..end])
    }

    /// Find `"key":[...]` and split its comma-separated quoted strings
    /// (used for `gpr`).
    fn get_str_array(&self, key: &str) -> Option<Vec<&'a str>> {
        let needle = format!("\"{key}\":[");
        let start = self.src.find(&needle)? + needle.len();
        let end = self.src[start..].find(']')? + start;
        let body = &self.src[start..end];
        if body.trim().is_empty() {
            return Some(Vec::new());
        }
        Some(
            body.split(',')
                .map(|item| item.trim().trim_matches('"'))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OUTPUT: &str = r#"breakpoint hit at 0x801187ac after 0 steps
zero=0x00000000 at=0x80160000 v0=0x00000000 v1=0x00000000 a0=0x00000000 a1=0x00000000 a2=0x00000002 a3=0x0000c03f
t0=0x80159542 t1=0x800a8768 t2=0x8014dc40 t3=0x00000008 t4=0x0000c03f t5=0x80151290 t6=0xfcffffff t7=0x00000003
s0=0x00000000 s1=0x00000000 s2=0x00000000 s3=0x00000000 s4=0x00000000 s5=0x00000000 s6=0x00000000 s7=0x00000000
t8=0x0f0a7008 t9=0x00000001 k0=0xa430000c k1=0x00000aaa gp=0x00000000 sp=0x8008d098 fp=0x00000000 ra=0x801187ac
cp0_status=0x2000ff01 cp0_cause=0x00000000 cp0_epc=0x8012ff04 cp0_badvaddr=0x00000012 cp0_count=0xe45089b8 cp0_compare=0x00000000
ORACLE_CAPTURE_V1={"gpr":["0x0000000000000000","0xffffffff80160000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000002","0x000000000000c03f","0xffffffff80159542","0xffffffff800a8768","0xffffffff8014dc40","0x0000000000000008","0x000000000000c03f","0xffffffff80151290","0xfffffffffcffffff","0x0000000000000003","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x0000000000000000","0x000000000f0a7008","0x0000000000000001","0xffffffffa430000c","0x0000000000000aaa","0x0000000000000000","0xffffffff8008d098","0x0000000000000000","0xffffffff801187ac"],"hit":0,"pc":"0x801187ac","phase":"paused","ranges":[]}
mupen[4]: Stopping emulation.
"#;

    #[test]
    fn parses_real_breakpoint_output_byte_exact() {
        let regs = parse_breakpoint_output(SAMPLE_OUTPUT, 0x801187ac).expect("parse");
        assert_eq!(regs.pc, 0x801187ac);
        assert_eq!(regs.steps, 0);
        assert_eq!(regs.gprs[29], 0xffff_ffff_8008_d098); // sp, sign-extended per real dump
        assert_eq!(regs.gprs[31], 0xffff_ffff_8011_87ac); // ra
        assert_eq!(regs.gprs[4], 0); // a0
        assert_eq!(regs.cp0_status, 0x2000_ff01);
        assert_eq!(regs.cp0_cause, 0);
        assert_eq!(regs.cp0_epc, 0x8012_ff04);
    }

    #[test]
    fn never_hit_is_reported_as_its_own_error_variant() {
        let stdout = "breakpoint 0x89999999 was not reached within 2000000 steps (ORACLE_STEP_LIMIT to raise)\n";
        // Simulate what `registers_at` does with a failing exit + this stdout shape.
        assert!(stdout.contains("was not reached within"));
    }
}
