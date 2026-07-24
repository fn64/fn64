//! Load an N64Recomp `oot.toml` (`[input]` + `[patches]`) plus its
//! `symbols_file_path` symbol table (`dump.toml`) into a typed
//! [`RecompConfig`]. This is the READ side of the N64Recomp config format:
//! it parses the exact on-disk shape real AKI/OoT configs use and produces
//! our own typed representation, so a caller can drive
//! `Recompiler::recompile` over a whole real ROM's symbol dump.
//!
//! # Format (observed, `aki-recomp/games/OOTU/{oot.toml,syms/dump.toml}`)
//!
//! The top-level config:
//! ```toml
//! [input]
//! entrypoint = 0x80000400
//! rom_file_path = "oot-ntsc-1.0.z64"
//! bss_section_suffix = "_bss"
//! symbols_file_path = "syms/dump.toml"
//! output_func_path = "RecompiledFuncs"
//! trace_mode = false
//!
//! [patches]
//! stubs = [ "func_...", ... ]
//! ```
//!
//! The companion symbol table (`symbols_file_path`, resolved relative to the
//! config file's own directory):
//! ```toml
//! [[section]]
//! name = "boot"
//! rom  = 0x00001060
//! vram = 0x80000460
//! size = 0x5dd0
//! functions = [ { name = "bootproc", vram = 0x80000498, size = 0x108 }, ... ]
//! ```
//!
//! TOML natively parses `0x...` hex integers, so `rom`/`vram`/`size` and
//! `entrypoint` deserialize directly to `u32`. Overlay sections are NOT a
//! special case here: in the OoT dump every section — base and overlay —
//! already carries its own `rom`/`vram`/`size`, so this symbol table is
//! self-contained (overlays.json is informational only; see its own
//! `comment`: "each overlay is position-independent code loaded into its own
//! RAM chunk … 0 vram collisions"). No overlay merge is needed.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::{Function, Patches, RecompConfig, Section};

/// Everything that can go wrong loading a config off disk. Loud/named per the
/// crate's "no silent failure" rule (mirrors [`crate::RecompError`]).
#[derive(Debug)]
pub enum LoadError {
    /// A file (the config or its symbol table) could not be read.
    Io { path: PathBuf, reason: String },
    /// A file's TOML did not parse or was missing a required key.
    Parse { path: PathBuf, reason: String },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, reason } => {
                write!(f, "could not read {}: {reason}", path.display())
            }
            LoadError::Parse { path, reason } => {
                write!(f, "could not parse {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for LoadError {}

// ---- serde mirrors of the two on-disk documents ----

#[derive(Debug, Deserialize)]
struct ConfigDoc {
    input: InputDoc,
    #[serde(default)]
    patches: PatchesDoc,
}

#[derive(Debug, Deserialize)]
struct InputDoc {
    entrypoint: u32,
    rom_file_path: String,
    #[serde(default = "default_bss_suffix")]
    bss_section_suffix: String,
    symbols_file_path: String,
    #[serde(default = "default_output_func_path")]
    output_func_path: String,
    #[serde(default)]
    trace_mode: bool,
}

fn default_bss_suffix() -> String {
    "_bss".to_string()
}

fn default_output_func_path() -> String {
    "RecompiledFuncs".to_string()
}

#[derive(Debug, Default, Deserialize)]
struct PatchesDoc {
    #[serde(default)]
    stubs: Vec<String>,
    #[serde(default)]
    ignored: Vec<String>,
    // Instruction/hook patches (`[[patches.instruction]]`/`[[patches.hook]]`)
    // are accepted-and-parsed so a config that has them loads cleanly, and are
    // carried into the typed `Patches`.
    #[serde(default)]
    instruction: Vec<InstructionPatchDoc>,
    #[serde(default)]
    hook: Vec<HookDoc>,
}

#[derive(Debug, Deserialize)]
struct InstructionPatchDoc {
    func: String,
    /// N64Recomp writes these as hex STRINGS (`vram = "0x..."`). Parsed
    /// leniently via [`parse_u32_flexible`].
    vram: StringOrInt,
    value: StringOrInt,
}

#[derive(Debug, Deserialize)]
struct HookDoc {
    func: String,
    before_vram: StringOrInt,
    text: String,
}

/// A field that may be a bare TOML integer (`0x1234`) or a quoted hex string
/// (`"0x1234"`). N64Recomp's own writer emits the string form for patch vram/
/// value; the symbol table uses bare ints. Accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrInt {
    Int(i64),
    Str(String),
}

impl StringOrInt {
    fn as_u32(&self, path: &Path, field: &str) -> Result<u32, LoadError> {
        match self {
            StringOrInt::Int(v) => u32::try_from(*v).map_err(|_| LoadError::Parse {
                path: path.to_path_buf(),
                reason: format!("{field} value {v} out of u32 range"),
            }),
            StringOrInt::Str(s) => parse_u32_flexible(s).ok_or_else(|| LoadError::Parse {
                path: path.to_path_buf(),
                reason: format!("{field} value {s:?} is not a valid u32"),
            }),
        }
    }
}

/// Parse `"0x1234"`, `"0X1234"`, or a plain decimal string into a `u32`.
fn parse_u32_flexible(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u32>().ok()
    }
}

#[derive(Debug, Deserialize)]
struct SymbolsDoc {
    #[serde(default)]
    section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
struct SectionDoc {
    name: String,
    rom: u32,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
struct FunctionDoc {
    name: String,
    vram: u32,
    size: u32,
}

/// Load an N64Recomp config file (`oot.toml`) and its referenced symbol table
/// into a [`RecompConfig`].
///
/// `config_path` points at the `[input]`/`[patches]` TOML. Its
/// `symbols_file_path` is resolved **relative to the config file's own
/// directory** (matching how N64Recomp itself resolves it). The `[patches]`
/// `stubs`/`ignored` are carried straight through — the caller
/// (`Recompiler::recompile`) is what actually skips stubbed/ignored functions.
///
/// If `rom_path_override` is `Some`, it replaces the config's `rom_file_path`
/// (the ROM lives out-of-tree; a driver passes its real absolute path here).
/// Otherwise the config's `rom_file_path` is resolved relative to the config
/// directory, matching N64Recomp.
pub fn load_config(
    config_path: impl AsRef<Path>,
    rom_path_override: Option<PathBuf>,
) -> Result<RecompConfig, LoadError> {
    let config_path = config_path.as_ref();
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

    let config_text = std::fs::read_to_string(config_path).map_err(|e| LoadError::Io {
        path: config_path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let doc: ConfigDoc = toml::from_str(&config_text).map_err(|e| LoadError::Parse {
        path: config_path.to_path_buf(),
        reason: e.to_string(),
    })?;

    let symbols_path = resolve_relative(config_dir, &doc.input.symbols_file_path);
    let sections = load_symbols(&symbols_path)?;

    let rom_file_path = match rom_path_override {
        Some(p) => p,
        None => resolve_relative(config_dir, &doc.input.rom_file_path),
    };

    let patches = build_patches(&doc.patches, config_path)?;

    Ok(RecompConfig {
        entrypoint: doc.input.entrypoint,
        rom_file_path,
        bss_section_suffix: doc.input.bss_section_suffix,
        output_func_path: PathBuf::from(doc.input.output_func_path),
        trace_mode: doc.input.trace_mode,
        sections,
        patches,
    })
}

/// Load just the symbol table (`dump.toml` shape) into typed [`Section`]s.
/// Exposed so a caller can load a symbol dump independently of a full config.
pub fn load_symbols(symbols_path: impl AsRef<Path>) -> Result<Vec<Section>, LoadError> {
    let symbols_path = symbols_path.as_ref();
    let symbols_text = std::fs::read_to_string(symbols_path).map_err(|e| LoadError::Io {
        path: symbols_path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let symbols: SymbolsDoc = toml::from_str(&symbols_text).map_err(|e| LoadError::Parse {
        path: symbols_path.to_path_buf(),
        reason: e.to_string(),
    })?;

    Ok(symbols
        .section
        .into_iter()
        .map(|s| Section {
            name: s.name,
            rom: s.rom,
            vram: s.vram,
            size: s.size,
            functions: s
                .functions
                .into_iter()
                .map(|f| Function {
                    name: f.name,
                    vram: f.vram,
                    size: f.size,
                })
                .collect(),
        })
        .collect())
}

fn build_patches(doc: &PatchesDoc, config_path: &Path) -> Result<Patches, LoadError> {
    let instructions = doc
        .instruction
        .iter()
        .map(|p| {
            Ok(crate::config::InstructionPatch {
                func: p.func.clone(),
                vram: p.vram.as_u32(config_path, "instruction.vram")?,
                value: p.value.as_u32(config_path, "instruction.value")?,
            })
        })
        .collect::<Result<Vec<_>, LoadError>>()?;
    let hooks = doc
        .hook
        .iter()
        .map(|h| {
            Ok(crate::config::Hook {
                func: h.func.clone(),
                before_vram: h.before_vram.as_u32(config_path, "hook.before_vram")?,
                text: h.text.clone(),
            })
        })
        .collect::<Result<Vec<_>, LoadError>>()?;
    Ok(Patches {
        stubs: doc.stubs.clone(),
        ignored: doc.ignored.clone(),
        instructions,
        hooks,
    })
}

/// Resolve `path` relative to `base` unless it is already absolute.
fn resolve_relative(base: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TMPDIR: AtomicU64 = AtomicU64::new(0);

    /// The `OOT_*` spellings of these knobs are gone; an unset var means "off",
    /// so a silent rename would let a stale `OOT_CONFIG=…` skip this test
    /// instead of running it.
    fn reject_legacy_env() {
        for (old, new) in [("OOT_CONFIG", "FN64_CONFIG")] {
            if std::env::var_os(old).is_some() {
                panic!("{old} was renamed to {new}; unset {old} and set {new} instead");
            }
        }
    }

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    fn tmpdir() -> PathBuf {
        // Parallel tests can observe the same coarse SystemTime tick: one test
        // then removes the shared directory while another is still writing its
        // config. The process-local sequence makes ownership unambiguous.
        let sequence = NEXT_TMPDIR.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "fn64-load-test-{}-{:?}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&d).unwrap();
        d
    }

    #[test]
    fn loads_small_handbuilt_config() {
        let dir = tmpdir();
        write(
            &dir,
            "dump.toml",
            r#"
[[section]]
name = "boot"
rom = 0x00001060
vram = 0x80000460
size = 0x200

functions = [
    { name = "bootproc", vram = 0x80000498, size = 0x108 },
    { name = "func_800005A0", vram = 0x800005a0, size = 0x9c },
]

[[section]]
name = "ovl_title"
rom = 0x00b9da40
vram = 0x80800000
size = 0x910

functions = [
    { name = "Title_Init", vram = 0x80800000, size = 0x40 },
]
"#,
        );
        let config = write(
            &dir,
            "oot.toml",
            r#"
[input]
entrypoint = 0x80000400
rom_file_path = "oot-ntsc-1.0.z64"
bss_section_suffix = "_bss"
symbols_file_path = "dump.toml"
output_func_path = "RecompiledFuncs"
trace_mode = false

[patches]
stubs = [ "bootproc", "Title_Init" ]
"#,
        );

        let cfg = load_config(&config, None).expect("must load");
        assert_eq!(cfg.entrypoint, 0x8000_0400);
        assert_eq!(cfg.bss_section_suffix, "_bss");
        assert!(!cfg.trace_mode);
        // rom_file_path resolves relative to the config dir.
        assert_eq!(cfg.rom_file_path, dir.join("oot-ntsc-1.0.z64"));
        assert_eq!(cfg.sections.len(), 2);
        assert_eq!(cfg.sections[0].name, "boot");
        assert_eq!(cfg.sections[0].rom, 0x1060);
        assert_eq!(cfg.sections[0].vram, 0x8000_0460);
        assert_eq!(cfg.sections[0].functions.len(), 2);
        assert_eq!(cfg.sections[0].functions[1].name, "func_800005A0");
        assert_eq!(cfg.sections[0].functions[1].vram, 0x8000_05a0);
        // Overlay section carries its own rom/vram — no special handling.
        assert_eq!(cfg.sections[1].name, "ovl_title");
        assert_eq!(cfg.sections[1].rom, 0x00b9_da40);
        assert_eq!(cfg.sections[1].vram, 0x8080_0000);
        // Stubs carried through verbatim.
        assert_eq!(cfg.patches.stubs, vec!["bootproc", "Title_Init"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rom_override_replaces_config_rom_path() {
        let dir = tmpdir();
        write(
            &dir,
            "dump.toml",
            r#"
[[section]]
name = "boot"
rom = 0x1000
vram = 0x80000400
size = 0x10
functions = [ { name = "entrypoint", vram = 0x80000400, size = 0x10 } ]
"#,
        );
        let config = write(
            &dir,
            "oot.toml",
            r#"
[input]
entrypoint = 0x80000400
rom_file_path = "relative.z64"
symbols_file_path = "dump.toml"
"#,
        );
        let override_path = PathBuf::from("/somewhere/absolute/oot.z64");
        let cfg = load_config(&config, Some(override_path.clone())).unwrap();
        assert_eq!(cfg.rom_file_path, override_path);
        // Defaults kicked in for omitted keys.
        assert_eq!(cfg.output_func_path, PathBuf::from("RecompiledFuncs"));
        assert_eq!(cfg.bss_section_suffix, "_bss");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn instruction_and_hook_patches_parse_string_hex() {
        let dir = tmpdir();
        write(
            &dir,
            "dump.toml",
            r#"
[[section]]
name = "boot"
rom = 0x1000
vram = 0x80000400
size = 0x10
functions = [ { name = "entrypoint", vram = 0x80000400, size = 0x10 } ]
"#,
        );
        // The N64Recomp config format writes vram/value as quoted hex strings.
        let config = write(
            &dir,
            "oot.toml",
            r#"
[input]
entrypoint = 0x80000400
rom_file_path = "r.z64"
symbols_file_path = "dump.toml"

[patches]
stubs = []
ignored = [ "func_ignored" ]

[[patches.instruction]]
func = "func_800004D0"
vram = "0x800005ac"
value = "0x1000ffff"

[[patches.hook]]
func = "func_80015250"
before_vram = "0x80015324"
text = "{ ctx.r4 = 0; }"
"#,
        );
        let cfg = load_config(&config, None).unwrap();
        assert_eq!(cfg.patches.ignored, vec!["func_ignored"]);
        assert_eq!(cfg.patches.instructions.len(), 1);
        assert_eq!(cfg.patches.instructions[0].vram, 0x8000_05ac);
        assert_eq!(cfg.patches.instructions[0].value, 0x1000_ffff);
        assert_eq!(cfg.patches.hooks.len(), 1);
        assert_eq!(cfg.patches.hooks[0].before_vram, 0x8001_5324);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Integration: load the REAL OoT config + symbol dump if present on disk
    /// (out-of-tree, `aki-recomp/games/OOTU`). Asserts the documented counts:
    /// 472 sections, 13,358 functions. Skips (does not fail) when the
    /// out-of-tree game content isn't checked out, so CI without the ROM tree
    /// stays green.
    #[test]
    fn loads_real_oot_symbols_when_present() {
        reject_legacy_env();

        let candidates = [
            std::env::var("FN64_CONFIG").ok().map(PathBuf::from),
            Some(PathBuf::from(
                "/Users/jer/Code/aki-recomp/games/OOTU/oot.toml",
            )),
        ];
        let config_path = candidates.into_iter().flatten().find(|p| p.exists());
        let Some(config_path) = config_path else {
            eprintln!("skipping real-OoT load test: oot.toml not found (out-of-tree game content)");
            return;
        };

        // Use a dummy ROM override so a missing ROM file doesn't matter — the
        // loader never reads the ROM, only records its path.
        let cfg = load_config(&config_path, Some(PathBuf::from("/dev/null")))
            .expect("real OoT config must load");

        assert_eq!(cfg.sections.len(), 472, "OoT dump.toml has 472 sections");
        let func_count: usize = cfg.sections.iter().map(|s| s.functions.len()).sum();
        assert_eq!(func_count, 13_358, "OoT dump.toml has 13,358 functions");
        assert_eq!(cfg.entrypoint, 0x8000_0400);
        // The [patches].stubs from oot.toml must be present.
        assert!(
            cfg.patches.stubs.contains(&"func_80026230".to_string()),
            "expected the known first stub to be loaded"
        );
    }
}
