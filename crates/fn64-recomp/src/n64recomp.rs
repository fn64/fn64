//! The N64Recomp adapter. Per `docs/DECOUPLING.md`: "**This is the only
//! crate that knows N64Recomp exists.**" — and within this crate, this is
//! the only module that names it. It (a) serializes a [`RecompConfig`]/
//! [`RspConfig`] into N64Recomp's/RSPRecomp's real TOML shape, (b) shells
//! out to the pinned fork's binaries, (c) collects the generated C.
//!
//! The TOML shape mirrored here (`[input]`, `[patches]` with `stubs`/
//! `ignored`/`[[patches.instruction]]`/`[[patches.hook]]`, and the separate
//! symbol-table TOML's `[[section]]`/`functions`) is taken directly from
//! real configs already driving AKI-title recompiles: `aki-recomp/games/
//! {NW4E,NWXE}/*.toml` and `refs/WCWnWoRevengeRecomp/{revenge.toml,rsp/
//! revenge_audio.toml}`. This is a faithful mirror of an observed format,
//! not a guess.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{RecompConfig, RspConfig};
use crate::{AbiVersion, RecompError, RecompOutput, Recompiler};

/// Where the pinned N64Recomp/RSPRecomp fork's build lives, e.g.
/// `refs/N64RecompSource/build`. Both binaries (`N64Recomp`, `RSPRecomp`)
/// are expected directly inside it, matching the layout every AKI-title
/// profile already assumes (`n64recomp_bin` in `aki_profile`'s
/// `GameProfile`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N64RecompPaths {
    pub build_dir: PathBuf,
}

impl N64RecompPaths {
    pub fn new(build_dir: impl Into<PathBuf>) -> Self {
        N64RecompPaths {
            build_dir: build_dir.into(),
        }
    }

    fn recomp_bin(&self) -> PathBuf {
        self.build_dir.join("N64Recomp")
    }

    fn rsp_recomp_bin(&self) -> PathBuf {
        self.build_dir.join("RSPRecomp")
    }
}

/// The `Recompiler` adapter over N64Recomp/RSPRecomp. Holds only the paths
/// to the pinned fork's binaries — no other state, since every actual
/// recompile is a one-shot subprocess invocation over a config file.
pub struct N64RecompAdapter {
    paths: N64RecompPaths,
    /// ABI version this adapter's pinned fork build targets. Not derived
    /// from the binary itself (N64Recomp has no `--abi-version` self-report
    /// today) — set at construction so `abi_version()` can still fail
    /// loudly against a caller's expectation per the trait's contract.
    abi_version: AbiVersion,
}

impl N64RecompAdapter {
    pub fn new(paths: N64RecompPaths, abi_version: AbiVersion) -> Self {
        N64RecompAdapter { paths, abi_version }
    }
}

// ---- N64Recomp's own TOML shape (private to this module) ----
//
// Built directly as `toml::Value` rather than via `#[derive(Serialize)]`:
// N64Recomp's real file layout has `[patches]` scalar keys (`stubs`/
// `ignored`) alongside `[[patches.instruction]]`/`[[patches.hook]]`
// array-of-tables under that SAME `patches` key, which needs the
// dotted-header form serde's derive doesn't have a direct knob for.

/// Top-level recompile config document. N64Recomp's real file layout has
/// `[patches]` scalar keys (`stubs`/`ignored`) alongside `[[patches.
/// instruction]]`/`[[patches.hook]]` array-of-tables under the SAME
/// `patches` key — `toml::Value` (rather than a flat derive) is used for
/// final serialization so the array-of-tables come out as `[[patches.
/// instruction]]` headers, matching every real config on disk, instead of
/// nested-inline-table syntax a naive derive would produce.
fn build_input_document(cfg: &RecompConfig, symbols_file_path: &str) -> toml::Value {
    let mut patches = toml::Table::new();
    patches.insert(
        "stubs".into(),
        toml::Value::Array(
            cfg.patches
                .stubs
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    patches.insert(
        "ignored".into(),
        toml::Value::Array(
            cfg.patches
                .ignored
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    if !cfg.patches.instructions.is_empty() {
        let arr = cfg
            .patches
            .instructions
            .iter()
            .map(|p| {
                let mut t = toml::Table::new();
                t.insert("func".into(), toml::Value::String(p.func.clone()));
                t.insert("vram".into(), toml::Value::String(hex(p.vram)));
                t.insert("value".into(), toml::Value::String(hex(p.value)));
                toml::Value::Table(t)
            })
            .collect();
        patches.insert("instruction".into(), toml::Value::Array(arr));
    }
    if !cfg.patches.hooks.is_empty() {
        let arr = cfg
            .patches
            .hooks
            .iter()
            .map(|h| {
                let mut t = toml::Table::new();
                t.insert("func".into(), toml::Value::String(h.func.clone()));
                t.insert(
                    "before_vram".into(),
                    toml::Value::String(hex(h.before_vram)),
                );
                t.insert("text".into(), toml::Value::String(h.text.clone()));
                toml::Value::Table(t)
            })
            .collect();
        patches.insert("hook".into(), toml::Value::Array(arr));
    }

    let mut input = toml::Table::new();
    input.insert(
        "entrypoint".into(),
        toml::Value::String(hex(cfg.entrypoint)),
    );
    input.insert(
        "rom_file_path".into(),
        toml::Value::String(cfg.rom_file_path.to_string_lossy().into_owned()),
    );
    input.insert(
        "bss_section_suffix".into(),
        toml::Value::String(cfg.bss_section_suffix.clone()),
    );
    input.insert(
        "symbols_file_path".into(),
        toml::Value::String(symbols_file_path.to_string()),
    );
    input.insert(
        "output_func_path".into(),
        toml::Value::String(cfg.output_func_path.to_string_lossy().into_owned()),
    );
    input.insert("trace_mode".into(), toml::Value::Boolean(cfg.trace_mode));

    let mut root = toml::Table::new();
    root.insert("input".into(), toml::Value::Table(input));
    root.insert("patches".into(), toml::Value::Table(patches));
    toml::Value::Table(root)
}

/// The companion symbol-table document (`dump.toml`'s real shape):
/// `[[section]]` blocks each carrying an inline `functions` array.
fn build_symbols_document(cfg: &RecompConfig) -> toml::Value {
    let sections = cfg
        .sections
        .iter()
        .map(|s| {
            let funcs = s
                .functions
                .iter()
                .map(|f| {
                    let mut t = toml::Table::new();
                    t.insert("name".into(), toml::Value::String(f.name.clone()));
                    t.insert("vram".into(), toml::Value::String(hex(f.vram)));
                    t.insert("size".into(), toml::Value::String(hex(f.size)));
                    toml::Value::Table(t)
                })
                .collect();
            let mut t = toml::Table::new();
            t.insert("name".into(), toml::Value::String(s.name.clone()));
            t.insert("rom".into(), toml::Value::String(hex(s.rom)));
            t.insert("vram".into(), toml::Value::String(hex(s.vram)));
            t.insert("size".into(), toml::Value::String(hex(s.size)));
            t.insert("functions".into(), toml::Value::Array(funcs));
            toml::Value::Table(t)
        })
        .collect();
    let mut root = toml::Table::new();
    root.insert("section".into(), toml::Value::Array(sections));
    toml::Value::Table(root)
}

/// RSPRecomp's own config document shape (see `rsp/revenge_audio.toml`).
fn build_rsp_document(cfg: &RspConfig) -> toml::Value {
    let mut root = toml::Table::new();
    root.insert(
        "text_offset".into(),
        toml::Value::String(hex(cfg.text_offset)),
    );
    root.insert("text_size".into(), toml::Value::String(hex(cfg.text_size)));
    root.insert(
        "text_address".into(),
        toml::Value::String(hex(cfg.text_address)),
    );
    root.insert(
        "rom_file_path".into(),
        toml::Value::String(cfg.rom_file_path.to_string_lossy().into_owned()),
    );
    root.insert(
        "output_file_path".into(),
        toml::Value::String(format!("{}.cpp", cfg.output_function_name)),
    );
    root.insert(
        "output_function_name".into(),
        toml::Value::String(cfg.output_function_name.clone()),
    );
    if !cfg.extra_indirect_branch_targets.is_empty() {
        let arr = cfg
            .extra_indirect_branch_targets
            .iter()
            .map(|v| toml::Value::String(hex(*v)))
            .collect();
        root.insert(
            "extra_indirect_branch_targets".into(),
            toml::Value::Array(arr),
        );
    }
    toml::Value::Table(root)
}

fn hex(v: u32) -> String {
    format!("{v:#x}")
}

impl Recompiler for N64RecompAdapter {
    fn recompile(&self, cfg: &RecompConfig) -> Result<RecompOutput, RecompError> {
        let workdir = tempfile_dir()?;
        let symbols_path = workdir.join("dump.toml");
        write_toml(&symbols_path, &build_symbols_document(cfg))?;

        let config_path = workdir.join("recomp.toml");
        write_toml(&config_path, &build_input_document(cfg, "dump.toml"))?;

        let binary = self.paths.recomp_bin();
        let output = Command::new(&binary)
            .arg(&config_path)
            .current_dir(&workdir)
            .output()
            .map_err(|e| RecompError::Launch {
                binary: binary.to_string_lossy().into_owned(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(RecompError::RecompilerFailed {
                binary: binary.to_string_lossy().into_owned(),
                output: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        collect_generated_c(&workdir.join(&cfg.output_func_path), cfg)
    }

    fn recompile_rsp(&self, cfg: &RspConfig) -> Result<RecompOutput, RecompError> {
        let workdir = tempfile_dir()?;
        let config_path = workdir.join("rsp.toml");
        write_toml(&config_path, &build_rsp_document(cfg))?;

        let binary = self.paths.rsp_recomp_bin();
        let output = Command::new(&binary)
            .arg(&config_path)
            .current_dir(&workdir)
            .output()
            .map_err(|e| RecompError::Launch {
                binary: binary.to_string_lossy().into_owned(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(RecompError::RecompilerFailed {
                binary: binary.to_string_lossy().into_owned(),
                output: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        let generated = workdir.join(format!("{}.cpp", cfg.output_function_name));
        if !generated.exists() {
            return Err(RecompError::MissingOutput(generated));
        }
        let text = std::fs::read_to_string(&generated)
            .map_err(|_| RecompError::MissingOutput(generated.clone()))?;
        Ok(RecompOutput {
            generated_files: vec![(generated, text)],
            recompiled_functions: vec![cfg.output_function_name.clone()],
        })
    }

    fn abi_version(&self) -> AbiVersion {
        self.abi_version
    }
}

fn collect_generated_c(output_dir: &Path, cfg: &RecompConfig) -> Result<RecompOutput, RecompError> {
    if !output_dir.exists() {
        return Err(RecompError::MissingOutput(output_dir.to_path_buf()));
    }
    let mut generated_files = Vec::new();
    let entries = std::fs::read_dir(output_dir)
        .map_err(|_| RecompError::MissingOutput(output_dir.to_path_buf()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("c") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                generated_files.push((path, text));
            }
        }
    }
    let stubbed: std::collections::HashSet<&str> =
        cfg.patches.stubs.iter().map(String::as_str).collect();
    let recompiled_functions = cfg
        .sections
        .iter()
        .flat_map(|s| s.functions.iter())
        .map(|f| f.name.clone())
        .filter(|n| !stubbed.contains(n.as_str()))
        .collect();
    Ok(RecompOutput {
        generated_files,
        recompiled_functions,
    })
}

fn write_toml(path: &Path, value: &toml::Value) -> Result<(), RecompError> {
    let text =
        toml::to_string_pretty(value).map_err(|e| RecompError::InvalidConfig(e.to_string()))?;
    let mut f = std::fs::File::create(path).map_err(|e| RecompError::Launch {
        binary: path.to_string_lossy().into_owned(),
        reason: e.to_string(),
    })?;
    f.write_all(text.as_bytes())
        .map_err(|e| RecompError::Launch {
            binary: path.to_string_lossy().into_owned(),
            reason: e.to_string(),
        })
}

fn tempfile_dir() -> Result<PathBuf, RecompError> {
    let dir = std::env::temp_dir().join(format!("fn64-recomp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| RecompError::Launch {
        binary: dir.to_string_lossy().into_owned(),
        reason: e.to_string(),
    })?;
    Ok(dir)
}

/// Serializes a [`RecompConfig`] (and its companion symbol table) to
/// N64Recomp's real TOML shape, without shelling out — the pure format
/// round-trip this crate's golden test proves, and the same function the
/// adapter's `recompile` uses internally before invoking the binary.
pub fn to_input_toml(cfg: &RecompConfig, symbols_file_path: &str) -> String {
    toml::to_string_pretty(&build_input_document(cfg, symbols_file_path))
        .expect("RecompConfig always serializes")
}

/// Serializes just the symbol-table half (mirrors a real `dump.toml`).
pub fn to_symbols_toml(cfg: &RecompConfig) -> String {
    toml::to_string_pretty(&build_symbols_document(cfg)).expect("RecompConfig always serializes")
}

/// Serializes an [`RspConfig`] to RSPRecomp's real TOML shape.
pub fn to_rsp_toml(cfg: &RspConfig) -> String {
    toml::to_string_pretty(&build_rsp_document(cfg)).expect("RspConfig always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Function, Hook, InstructionPatch, Patches, Section};

    /// Golden test: a known `RecompConfig` -> the exact TOML shape real
    /// AKI-title configs use (`[input]`, `[patches]` with `stubs`/
    /// `ignored`/`[[patches.instruction]]`/`[[patches.hook]]`), modeled on
    /// `wm2000.toml`'s idle-thread + dispatch-table patches.
    fn sample_config() -> RecompConfig {
        RecompConfig {
            entrypoint: 0x8000_0400,
            rom_file_path: PathBuf::from("wm2000.z64"),
            bss_section_suffix: "_bss".to_string(),
            output_func_path: PathBuf::from("RecompiledFuncs"),
            trace_mode: false,
            sections: vec![Section {
                name: "entry".to_string(),
                rom: 0x1000,
                vram: 0x8000_0400,
                size: 0x50,
                functions: vec![Function {
                    name: "func_80000400".to_string(),
                    vram: 0x8000_0400,
                    size: 0x3C,
                }],
            }],
            patches: Patches {
                stubs: vec!["func_80000B30".to_string(), "func_80022540".to_string()],
                ignored: vec![],
                instructions: vec![InstructionPatch {
                    func: "func_800004D0".to_string(),
                    vram: 0x8000_05AC,
                    value: 0x1000_FFFF,
                }],
                hooks: vec![Hook {
                    func: "func_80015250".to_string(),
                    before_vram: 0x8001_5324,
                    text: "{ ctx->r4 = 0; }".to_string(),
                }],
            },
        }
    }

    #[test]
    fn golden_input_toml_round_trip() {
        let cfg = sample_config();
        let text = to_input_toml(&cfg, "syms/dump.toml");

        // Structural comparison (order/whitespace-independent) rather than a
        // brittle literal string match, since `toml::to_string_pretty`'s
        // exact formatting is a library-version detail; what must be
        // provably stable across regens is the parsed VALUE shape, matching
        // real configs' `[input]`/`[patches]`/`[[patches.instruction]]`/
        // `[[patches.hook]]` layout (see `wm2000.toml`).
        let parsed: toml::Value = toml::from_str(&text).expect("golden output must parse");

        let input = parsed.get("input").expect("has [input]");
        assert_eq!(
            input.get("entrypoint").unwrap().as_str().unwrap(),
            "0x80000400"
        );
        assert_eq!(
            input.get("rom_file_path").unwrap().as_str().unwrap(),
            "wm2000.z64"
        );
        assert_eq!(
            input.get("bss_section_suffix").unwrap().as_str().unwrap(),
            "_bss"
        );
        assert_eq!(
            input.get("symbols_file_path").unwrap().as_str().unwrap(),
            "syms/dump.toml"
        );
        assert_eq!(
            input.get("output_func_path").unwrap().as_str().unwrap(),
            "RecompiledFuncs"
        );
        assert!(!input.get("trace_mode").unwrap().as_bool().unwrap());

        let patches = parsed.get("patches").expect("has [patches]");
        let stubs = patches.get("stubs").unwrap().as_array().unwrap();
        assert_eq!(stubs.len(), 2);
        assert_eq!(stubs[0].as_str().unwrap(), "func_80000B30");
        assert_eq!(stubs[1].as_str().unwrap(), "func_80022540");
        assert!(patches
            .get("ignored")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());

        let instr = patches
            .get("instruction")
            .expect("has [[patches.instruction]]")
            .as_array()
            .unwrap();
        assert_eq!(instr.len(), 1);
        assert_eq!(
            instr[0].get("func").unwrap().as_str().unwrap(),
            "func_800004D0"
        );
        assert_eq!(
            instr[0].get("vram").unwrap().as_str().unwrap(),
            "0x800005ac"
        );
        assert_eq!(
            instr[0].get("value").unwrap().as_str().unwrap(),
            "0x1000ffff"
        );

        let hooks = patches
            .get("hook")
            .expect("has [[patches.hook]]")
            .as_array()
            .unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(
            hooks[0].get("func").unwrap().as_str().unwrap(),
            "func_80015250"
        );
        assert_eq!(
            hooks[0].get("before_vram").unwrap().as_str().unwrap(),
            "0x80015324"
        );
        assert_eq!(
            hooks[0].get("text").unwrap().as_str().unwrap(),
            "{ ctx->r4 = 0; }"
        );
    }

    #[test]
    fn golden_symbols_toml_round_trip() {
        let cfg = sample_config();
        let text = to_symbols_toml(&cfg);
        let parsed: toml::Value = toml::from_str(&text).expect("must parse");
        let sections = parsed.get("section").unwrap().as_array().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].get("name").unwrap().as_str().unwrap(), "entry");
        assert_eq!(sections[0].get("rom").unwrap().as_str().unwrap(), "0x1000");
        assert_eq!(
            sections[0].get("vram").unwrap().as_str().unwrap(),
            "0x80000400"
        );
        let funcs = sections[0].get("functions").unwrap().as_array().unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(
            funcs[0].get("name").unwrap().as_str().unwrap(),
            "func_80000400"
        );
    }

    #[test]
    fn golden_rsp_toml_round_trip() {
        let cfg = RspConfig {
            text_offset: 0x2C910,
            text_size: 0xC54,
            text_address: 0x0400_1080,
            rom_file_path: PathBuf::from("../revenge.z64"),
            output_function_name: "revenge_audio_ucode".to_string(),
            extra_indirect_branch_targets: vec![0x10EC, 0x139C, 0x12B0],
        };
        let text = to_rsp_toml(&cfg);
        let parsed: toml::Value = toml::from_str(&text).expect("must parse");
        assert_eq!(
            parsed.get("text_offset").unwrap().as_str().unwrap(),
            "0x2c910"
        );
        assert_eq!(parsed.get("text_size").unwrap().as_str().unwrap(), "0xc54");
        assert_eq!(
            parsed.get("text_address").unwrap().as_str().unwrap(),
            "0x4001080"
        );
        assert_eq!(
            parsed.get("output_file_path").unwrap().as_str().unwrap(),
            "revenge_audio_ucode.cpp"
        );
        let targets = parsed
            .get("extra_indirect_branch_targets")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].as_str().unwrap(), "0x10ec");
    }

    #[test]
    fn empty_patch_lists_still_serialize_as_present_stubs_and_ignored() {
        // N64Recomp's real configs always have `stubs = []`/`ignored = []`
        // present (never an absent key) even with zero entries -- a config
        // with no stubs at all must still round-trip that shape.
        let mut cfg = sample_config();
        cfg.patches.stubs.clear();
        cfg.patches.instructions.clear();
        cfg.patches.hooks.clear();
        let text = to_input_toml(&cfg, "syms/dump.toml");
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        let patches = parsed.get("patches").unwrap();
        assert!(patches.get("stubs").unwrap().as_array().unwrap().is_empty());
        assert!(patches.get("instruction").is_none());
        assert!(patches.get("hook").is_none());
    }
}
