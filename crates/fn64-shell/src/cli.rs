//! fn64's ONE configuration surface: a clap CLI, an optional `fn64.toml`, and
//! a typed [`Knobs`] struct resolved exactly once in `main`.
//!
//! Before this module the shell had no argument parser at all -- its only flag
//! was a hand-rolled `--demo` scan of `std::env::args()` -- and every setting
//! arrived as a bare `std::env::var("FN64_...")` read scattered across the
//! boot path. A player had no way to discover what was settable (there was no
//! `--help`), no way to persist a choice (there was no config file), and no
//! way to see what a run had actually resolved to. `docs/knobs.toml` counted
//! 18 `user`-class names; not one of them was reachable except by exporting an
//! environment variable someone had to already know the name of.
//!
//! ## Precedence
//!
//! Highest wins:
//!
//! 1. **CLI flag** -- `--render reference`.
//! 2. **`fn64.toml`** -- `--config <path>` if given, else the first that
//!    exists of: `fn64.toml` next to the shard root, `$XDG_CONFIG_HOME/fn64/
//!    fn64.toml`, `dirs::config_dir()/fn64/fn64.toml` (the same platform
//!    config directory `input_map.rs` and `video_config.rs` already use).
//! 3. **`FN64_*` environment variable** -- a COMPATIBILITY layer, kept for one
//!    release so existing scripts and gate harnesses keep working. New code
//!    must not add names here.
//! 4. **The struct default** -- what [`Knobs::default`] says.
//!
//! ## Why the env lookup is a closure
//!
//! [`Knobs::resolve`] takes `env: impl Fn(&str) -> Option<String>` rather than
//! calling `std::env::var` itself. The process environment is global mutable
//! state: a test that sets `FN64_RENDER` to check precedence races every other
//! test in the same binary (see `stack.rs`'s
//! `the_hud_opens_at_startup_only_when_explicitly_asked`, which has to
//! serialize every case into one test and restore the old value by hand to
//! stay sound). With the lookup injected, the precedence tests below assert
//! the real resolution order while touching nothing outside their own stack
//! frame. `main` passes [`process_env`], which is the only site in this crate
//! permitted to read the environment.

use std::path::PathBuf;

use clap::Parser;

/// The one thing in this crate allowed to read the process environment.
///
/// Everything else takes a resolved [`Knobs`]. `scripts/lint-hot-path-env.py`
/// is what keeps that true as the crate grows.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Which recompiler lane executes guest CPU code.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecompLane {
    /// `fn64-cpu-runtime`: the pure-Rust whole-ROM recompiler.
    Rs,
    /// N64Recomp-emitted C bodies compiled by `build.rs`.
    #[default]
    C,
}

impl RecompLane {
    /// The spelling this lane answers to on the command line, in `fn64.toml`,
    /// and in `FN64_RECOMP`. One function so the three surfaces cannot drift.
    pub fn as_str(self) -> &'static str {
        match self {
            RecompLane::Rs => "rs",
            RecompLane::C => "c",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rs" => Some(RecompLane::Rs),
            "c" => Some(RecompLane::C),
            _ => None,
        }
    }
}

/// Which render backend draws the frame.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderBackendKind {
    /// The pure-Rust wgpu backend (the all-Rust stack's GPU half).
    Wgpu,
    /// The software `ReferenceBackend` -- fn64's CI oracle, and the default.
    #[default]
    Reference,
    /// The RT64 static backend. Requires the `rt64` Cargo feature.
    Rt64,
}

impl RenderBackendKind {
    /// The spelling this backend answers to everywhere. `boot()` compares the
    /// RESOLVED backend against this, and `stack.rs` prints it verbatim.
    pub fn as_str(self) -> &'static str {
        match self {
            RenderBackendKind::Wgpu => "wgpu",
            RenderBackendKind::Reference => "reference",
            RenderBackendKind::Rt64 => "rt64",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "wgpu" => Some(RenderBackendKind::Wgpu),
            "reference" => Some(RenderBackendKind::Reference),
            "rt64" => Some(RenderBackendKind::Rt64),
            _ => None,
        }
    }
}

/// fn64: play a recompiled Nintendo 64 game in a window.
///
/// Settings resolve in this order, highest first: a flag below, then
/// `fn64.toml`, then the matching `FN64_*` environment variable (kept for one
/// release for existing scripts), then the built-in default. `--print-config`
/// dumps what a run actually resolved to, in `fn64.toml` form.
#[derive(Debug, Default, Parser)]
#[command(name = "fn64", version, about, long_about = None)]
pub struct Cli {
    /// ROM image to boot. Overrides the `ROM` environment variable.
    #[arg(long, value_name = "PATH")]
    pub rom: Option<PathBuf>,

    /// Extracted game-package shard root. Also where `fn64.toml` is looked for
    /// first. (`FN64_SHARD_ROOT`)
    #[arg(long, value_name = "PATH")]
    pub shard_root: Option<PathBuf>,

    /// Identity-checked IPL3 boot context for the rs lane. Required by the rs
    /// lane; ignored by the C lane. (`FN64_BOOT_CONTEXT`)
    #[arg(long, value_name = "PATH")]
    pub boot_context: Option<PathBuf>,

    /// Recompiler lane this binary was BUILT with -- `rs` (fn64-cpu-runtime)
    /// or `c` (N64Recomp bodies). Assert-only: the lane is fixed at compile
    /// time by `FN64_RECOMP` in build.rs, so passing a lane this binary was
    /// not built with is a hard error rather than a silent no-op. Use it in a
    /// script to prove you launched the binary you meant to.
    #[arg(long, value_name = "LANE")]
    pub recomp: Option<RecompLane>,

    /// Render backend. `reference` is the software oracle and the default;
    /// `rt64` needs the `rt64` Cargo feature. (`FN64_RENDER`)
    #[arg(long, value_name = "BACKEND")]
    pub render: Option<RenderBackendKind>,

    /// Re-present the previous field instead of blocking guest time (and audio
    /// production) on an unfinished renderer join. On by default.
    /// (`FN64_AUDIO_PRIORITY`)
    #[arg(long, value_name = "BOOL")]
    pub audio_priority: Option<bool>,

    /// Milliseconds the audio-priority bounded VI join may spend waiting.
    /// (`FN64_AUDIO_PRIORITY_JOIN_BUDGET_MS`)
    #[arg(long, value_name = "MS")]
    pub audio_priority_join_budget_ms: Option<u32>,

    /// Disable audio output entirely. (`FN64_NO_AUDIO`)
    #[arg(long)]
    pub no_audio: bool,

    /// Bring the stack/framerate HUD up with the window, without synthesizing
    /// an F3 keypress. (`FN64_HUD`)
    #[arg(long)]
    pub hud: bool,

    /// Overscan crop in pixels applied to the presented field. Overrides the
    /// persisted video config for this session only. (`FN64_OVERSCAN`)
    #[arg(long, value_name = "PIXELS")]
    pub overscan: Option<u32>,

    /// Where F2 screenshots land. (`FN64_SCREENSHOT_DIR`)
    #[arg(long, value_name = "DIR")]
    pub screenshot_dir: Option<PathBuf>,

    /// The game's aligned `__CartRomHandle` BSS address, as hex. Titles the
    /// default probe does not cover need this. (`FN64_CART_HANDLE_VRAM`)
    #[arg(long, value_name = "HEX")]
    pub cart_handle_vram: Option<String>,

    /// How many linked sections stay always-resident. OoT keeps 3; titles
    /// whose section 2 is an overlay bank keep 2. (`FN64_RESIDENT_SECTIONS`)
    #[arg(long, value_name = "COUNT")]
    pub resident_sections: Option<usize>,

    /// Content-free UI demo: the real presentation path driven by a synthetic
    /// RDRAM field. No ROM, no recompilation.
    #[arg(long)]
    pub demo: bool,

    /// Exit the demo after this many frames. (`FN64_DEMO_FRAMES`)
    #[arg(long, value_name = "N")]
    pub demo_frames: Option<u64>,

    /// Start the demo on the custom fullscreen presenter, so shader creation
    /// is exercised without UI automation. (`FN64_DEMO_ZOOM_FILL`)
    #[arg(long)]
    pub demo_zoom_fill: bool,

    /// Presented-frame cache mode. (`FN64_PRESENT_CACHE`)
    #[arg(long, value_name = "MODE")]
    pub present_cache: Option<String>,

    /// Write every presented field to this directory as a PNG.
    /// (`FN64_FRAME_DUMP`)
    #[arg(long, value_name = "DIR")]
    pub frame_dump: Option<PathBuf>,

    /// Headless input-seam self-test: drive ONE press of this key through the
    /// real PadState path, assert the game-facing state is non-neutral, and
    /// exit. Proves keyboard -> controller wiring without a keyboard.
    /// (`FN64_INPUT_PROBE`)
    #[arg(long, value_name = "KEY")]
    pub input_probe: Option<String>,

    /// Read settings from this `fn64.toml` instead of searching the shard root
    /// and the platform config directory.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Print the fully resolved configuration as TOML and exit. Save the
    /// output as `fn64.toml` to make this run's settings the default.
    #[arg(long)]
    pub print_config: bool,
}

/// The on-disk `fn64.toml`. Every field is optional: an absent field falls
/// through to the env-compat layer and then to the struct default, so a
/// two-line config file is a legal config file.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileConfig {
    #[serde(default)]
    pub rom: Option<PathBuf>,
    #[serde(default)]
    pub shard_root: Option<PathBuf>,
    #[serde(default)]
    pub boot_context: Option<PathBuf>,
    #[serde(default)]
    pub recomp: Option<RecompLane>,
    #[serde(default)]
    pub render: Option<RenderBackendKind>,
    #[serde(default)]
    pub audio: FileAudio,
    #[serde(default)]
    pub video: FileVideo,
    #[serde(default)]
    pub boot: FileBoot,
    #[serde(default)]
    pub diagnostics: FileDiagnostics,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileAudio {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub priority: Option<bool>,
    #[serde(default)]
    pub priority_join_budget_ms: Option<u32>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileVideo {
    #[serde(default)]
    pub hud: Option<bool>,
    #[serde(default)]
    pub overscan: Option<u32>,
    #[serde(default)]
    pub screenshot_dir: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileBoot {
    #[serde(default)]
    pub cart_handle_vram: Option<String>,
    #[serde(default)]
    pub resident_sections: Option<usize>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileDiagnostics {
    #[serde(default)]
    pub present_cache: Option<String>,
    #[serde(default)]
    pub frame_dump: Option<PathBuf>,
    #[serde(default)]
    pub demo_frames: Option<u64>,
    #[serde(default)]
    pub demo_zoom_fill: Option<bool>,
    #[serde(default)]
    pub input_probe: Option<String>,
}

/// Everything the shell's boot path used to read out of the environment,
/// resolved exactly once.
///
/// Sub-structs group by the subsystem that consumes them, so a call site takes
/// `&knobs.video` rather than the whole struct and the compiler says which
/// settings a function actually depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Knobs {
    /// ROM image to boot. `None` means "the `ROM` environment variable", which
    /// is what the build-time intake contract has always used.
    pub rom: Option<PathBuf>,
    /// Extracted game-package shard root.
    pub shard_root: Option<PathBuf>,
    /// Identity-checked IPL3 boot context. Required by the rs lane.
    pub boot_context: Option<PathBuf>,
    pub recomp: RecompLane,
    pub render: RenderKnobs,
    pub audio: AudioKnobs,
    pub video: VideoKnobs,
    pub boot: BootKnobs,
    pub diagnostics: DiagnosticKnobs,
    /// Content-free UI demo instead of a game boot.
    pub demo: bool,
    /// Print the resolved configuration as TOML and exit.
    pub print_config: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderKnobs {
    /// The REQUESTED backend. `boot()` may still fall back to `reference` if
    /// construction fails; `stack.rs` names that outcome separately.
    pub backend: RenderBackendKind,
    /// The wgpu backend's launch-time probe policy.
    ///
    /// All seven of its knobs are `diagnostic`-class, so none has a flag and
    /// this is always the documented default today. It is carried on `Knobs`
    /// anyway, and passed to `WgpuBackend::try_new_with_knobs`, so that the
    /// shell -> backend configuration path EXISTS: before task 2.2b the
    /// backend read those seven variables itself at construction, and there
    /// was no way for the host to state a policy at all. Giving one of them a
    /// flag is now a two-line change here rather than a new seam.
    pub wgpu: fn64_render_wgpu::WgpuKnobs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioKnobs {
    /// `false` is `FN64_NO_AUDIO`: no host output stream is opened.
    pub enabled: bool,
    /// Re-present the previous field rather than block on an unfinished join.
    pub priority: bool,
    /// Bound on the audio-priority VI join, in milliseconds. `None` leaves the
    /// ABI's own default in place.
    pub priority_join_budget_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoKnobs {
    /// HUD is already up when the window opens.
    pub hud: bool,
    /// Session-only overscan override; `None` keeps the persisted video config.
    pub overscan: Option<u32>,
    /// `None` means `screenshot::resolve_dir`'s own default.
    pub screenshot_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootKnobs {
    /// `__CartRomHandle` BSS address. Defaults to OoT NTSC 1.0's.
    pub cart_handle_vram: u32,
    /// Always-resident linked section count.
    pub resident_sections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticKnobs {
    /// Raw presented-frame cache mode string; `framebuffer` owns the parse.
    pub present_cache: Option<String>,
    pub frame_dump: Option<PathBuf>,
    pub demo_frames: Option<u64>,
    pub demo_zoom_fill: bool,
    /// The key the headless input-seam self-test presses, if any.
    pub input_probe: Option<String>,
    /// Raw values for the five trace/census sinks, which own their own
    /// validation (an absolute-path rule, a "this requires that" rule, a
    /// bounded parse). They are carried as strings rather than parsed here
    /// **on purpose**: each sink already has a `from_values` seam that its own
    /// tests drive, and re-implementing those rules in `resolve` would put the
    /// same contract in two places for the compiler to let drift. `resolve`
    /// decides WHERE the value comes from; the sink still decides whether it
    /// is legal.
    pub sinks: SinkKnobs,
}

/// Raw, unvalidated inputs for the diagnostic sinks. Every field is the string
/// the corresponding `FN64_*` variable used to be read from directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinkKnobs {
    pub device_timing_trace: Option<String>,
    pub device_timing_trace_id: Option<String>,
    pub device_trace_scope: Option<String>,
    pub presentation_trace: Option<String>,
    pub presentation_trace_id: Option<String>,
    pub av_sync_cue_id: Option<String>,
    pub av_sync_probe: Option<String>,
    pub av_sync_video_hash: Option<String>,
    pub av_sync_video_occurrence: Option<String>,
    pub av_sync_frame_dump: Option<PathBuf>,
    pub frame_trip: Option<String>,
    pub frame_trip_frames: Option<String>,
    pub pump_census: Option<String>,
    pub pump_census_pumps: Option<String>,
    pub pump_census_sequence: Option<String>,
    pub pump_census_warmup: Option<String>,
}

/// OoT NTSC 1.0's aligned `__CartRomHandle`. Titles that differ (WM2000/NWXE's
/// `D_800839A0`) override it; a wrong value aborts boot inside
/// `osCartRomInit`, so this default is a documented starting point, not a
/// guess the shell can make on the player's behalf.
pub const DEFAULT_CART_HANDLE_VRAM: u32 = 0x8000_9EA0;

/// OoT keeps `makerom.ent`/`boot`/`code` resident.
pub const DEFAULT_RESIDENT_SECTIONS: usize = 3;

/// The lane this binary was actually compiled on.
///
/// Selected by `cfg!(fn64_cpu_runtime)` -- the cfg `build.rs` sets from
/// `FN64_RECOMP` -- so it cannot drift from the bodies that were linked. Same
/// discipline as `stack.rs`'s `RECOMPILER_LANE`, and for the same reason: a
/// runtime read would report what the LAST build was ASKED for, which the
/// environment can change without changing a single instruction in the binary.
pub const COMPILED_RECOMP_LANE: RecompLane = if cfg!(fn64_cpu_runtime) {
    RecompLane::Rs
} else {
    RecompLane::C
};

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            rom: None,
            shard_root: None,
            boot_context: None,
            // Not `RecompLane::default()`: the lane is whatever this binary
            // was compiled with, and there is no such thing as a "default"
            // that differs from it.
            recomp: COMPILED_RECOMP_LANE,
            render: RenderKnobs {
                backend: RenderBackendKind::default(),
                wgpu: fn64_render_wgpu::WgpuKnobs::default(),
            },
            audio: AudioKnobs {
                enabled: true,
                priority: true,
                priority_join_budget_ms: None,
            },
            video: VideoKnobs {
                hud: false,
                overscan: None,
                screenshot_dir: None,
            },
            boot: BootKnobs {
                cart_handle_vram: DEFAULT_CART_HANDLE_VRAM,
                resident_sections: DEFAULT_RESIDENT_SECTIONS,
            },
            diagnostics: DiagnosticKnobs {
                present_cache: None,
                frame_dump: None,
                demo_frames: None,
                demo_zoom_fill: false,
                input_probe: None,
                sinks: SinkKnobs::default(),
            },
            demo: false,
            print_config: false,
        }
    }
}

/// The spellings `FN64_HUD`, `FN64_DEMO_ZOOM_FILL` and friends have always
/// accepted. Preserved EXACTLY: this is a compatibility layer, and a script
/// that said `FN64_HUD=on` must keep meaning what it meant.
fn truthy(raw: &str) -> bool {
    matches!(raw.trim(), "1" | "true" | "on")
}

impl Knobs {
    /// Resolve one configuration from the three sources plus the defaults.
    ///
    /// `env` is injected rather than read directly so tests never mutate the
    /// process environment -- see the module docs. `main` passes
    /// [`process_env`].
    pub fn resolve(
        cli: Cli,
        file: Option<FileConfig>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let file = file.unwrap_or_default();
        let default = Knobs::default();

        // One helper per shape. Each reads exactly the precedence the module
        // docs promise: flag, file, env, default -- and each `?`-chain stops
        // at the first source that supplied a value, so an env var set to a
        // value the parser rejects falls through to the default rather than
        // aborting a run that never asked for it.
        let env_str = |name: &str| env(name).filter(|value| !value.is_empty());

        // The bare `ROM` is the LAST rung, after `FN64_ROM`: it is what the
        // build-time intake contract has always used (build.rs,
        // examples/oot-boot, every runner script), so dropping it would break
        // every existing invocation, but it is also an unprefixed name in a
        // shared namespace and so should lose to the prefixed one.
        let rom = cli
            .rom
            .or(file.rom)
            .or_else(|| env_str("FN64_ROM").map(PathBuf::from))
            .or_else(|| env_str("ROM").map(PathBuf::from))
            .or(default.rom);

        let shard_root = cli
            .shard_root
            .or(file.shard_root)
            .or_else(|| env_str("FN64_SHARD_ROOT").map(PathBuf::from))
            .or(default.shard_root);

        let boot_context = cli
            .boot_context
            .or(file.boot_context)
            .or_else(|| env_str("FN64_BOOT_CONTEXT").map(PathBuf::from))
            .or(default.boot_context);

        // NOT resolved by precedence, deliberately: the lane is a COMPILE-TIME
        // fact. `build.rs` reads `FN64_RECOMP` and sets `cfg(fn64_cpu_runtime)`
        // from it, which decides which bodies are linked into this binary --
        // by the time any flag could be parsed, the answer is already baked in
        // and unchangeable. So `--recomp` is an ASSERTION, and a mismatch is a
        // loud failure. Reporting a request here would let `--print-config`
        // claim `rs` for a binary running the C lane, which is precisely the
        // "a session running the C lane looked exactly like one running the
        // Rust lane" confusion stack.rs exists to have ended.
        let recomp = COMPILED_RECOMP_LANE;
        // An EXPLICIT claim -- a flag someone typed, or a key someone wrote
        // into fn64.toml -- is a hard error when it disagrees: the user stated
        // something about this binary that is false, and continuing would run
        // the other lane while they believe otherwise.
        if let Some(claimed) = cli.recomp.or(file.recomp) {
            assert_eq!(
                claimed,
                recomp,
                "fn64: --recomp {} was asked for, but this binary was BUILT on the {} lane. \
                 The lane is fixed at compile time (build.rs reads FN64_RECOMP); rebuild with \
                 FN64_RECOMP={} to change it.",
                claimed.as_str(),
                recomp.as_str(),
                claimed.as_str(),
            );
        } else if let Some(inherited) =
            env_str("FN64_RECOMP").and_then(|v| RecompLane::parse(&v))
        {
            // An INHERITED value is ambient state, not a claim. `FN64_RECOMP`
            // is exported by documented workflows (docs/FAST-LOOP.md) and by
            // the build itself, so a shell launched from that same session
            // legitimately sees a value that no longer describes the binary
            // it is running. Panicking on it would break those workflows to
            // punish the user for an environment they were told to set. Warn
            // once, naming the lane that actually applies, and continue.
            if inherited != recomp {
                eprintln!(
                    "[fn64-shell] WARNING: FN64_RECOMP={} is set, but this binary was BUILT on \
                     the {} lane -- the inherited value is ignored (the lane is fixed at compile \
                     time). Pass --recomp {} to assert the lane, or rebuild with FN64_RECOMP={}.",
                    inherited.as_str(),
                    recomp.as_str(),
                    recomp.as_str(),
                    inherited.as_str(),
                );
            }
        }

        let backend = cli
            .render
            .or(file.render)
            .or_else(|| env_str("FN64_RENDER").and_then(|v| RenderBackendKind::parse(&v)))
            .unwrap_or(default.render.backend);

        // `--no-audio` and `--hud` are `bool` flags, not `Option<bool>`: clap
        // cannot express "absent" for a bare flag. Absent therefore means
        // "defer", and the flag can only turn the setting ON. That is the
        // right shape for both -- neither has a plausible "force it off past
        // an fn64.toml that turned it on" use, and adding `--no-hud` for
        // symmetry would be an abstraction nothing asked for.
        let audio_enabled = if cli.no_audio {
            false
        } else {
            file.audio
                .enabled
                .or_else(|| env("FN64_NO_AUDIO").map(|_| false))
                .unwrap_or(default.audio.enabled)
        };

        let audio_priority = cli
            .audio_priority
            .or(file.audio.priority)
            // Historic spelling: any value other than "0" enables it.
            .or_else(|| env_str("FN64_AUDIO_PRIORITY").map(|value| value != "0"))
            .unwrap_or(default.audio.priority);

        let audio_priority_join_budget_ms = cli
            .audio_priority_join_budget_ms
            .or(file.audio.priority_join_budget_ms)
            .or_else(|| {
                env_str("FN64_AUDIO_PRIORITY_JOIN_BUDGET_MS").and_then(|v| v.trim().parse().ok())
            })
            .or(default.audio.priority_join_budget_ms);

        let hud = if cli.hud {
            true
        } else {
            file.video
                .hud
                .or_else(|| env_str("FN64_HUD").map(|v| truthy(&v)))
                .unwrap_or(default.video.hud)
        };

        let overscan = cli
            .overscan
            .or(file.video.overscan)
            .or_else(|| env_str("FN64_OVERSCAN").and_then(|v| v.trim().parse().ok()))
            .or(default.video.overscan);

        let screenshot_dir = cli
            .screenshot_dir
            .or(file.video.screenshot_dir)
            .or_else(|| env_str("FN64_SCREENSHOT_DIR").map(PathBuf::from))
            .or(default.video.screenshot_dir);

        // Hex, with or without an `0x` prefix. A malformed value is a LOUD
        // failure everywhere it can be: the old code panicked, and a silently
        // defaulted cart handle aborts boot much later inside `osCartRomInit`
        // with a symptom that names nothing.
        let cart_handle_vram = cli
            .cart_handle_vram
            .or(file.boot.cart_handle_vram)
            .or_else(|| env_str("FN64_CART_HANDLE_VRAM"))
            .map(|raw| {
                u32::from_str_radix(raw.trim().trim_start_matches("0x"), 16).unwrap_or_else(|_| {
                    panic!("cart-handle-vram must be a hex vram address, got {raw:?}")
                })
            })
            .unwrap_or(default.boot.cart_handle_vram);

        let resident_sections = cli
            .resident_sections
            .or(file.boot.resident_sections)
            .or_else(|| {
                env_str("FN64_RESIDENT_SECTIONS").map(|raw| {
                    raw.trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("resident-sections must be a count, got {raw:?}"))
                })
            })
            .unwrap_or(default.boot.resident_sections);

        let present_cache = cli
            .present_cache
            .or(file.diagnostics.present_cache)
            .or_else(|| env_str("FN64_PRESENT_CACHE"))
            .or(default.diagnostics.present_cache);

        let frame_dump = cli
            .frame_dump
            .or(file.diagnostics.frame_dump)
            .or_else(|| env_str("FN64_FRAME_DUMP").map(PathBuf::from))
            .or(default.diagnostics.frame_dump);

        let demo_frames = cli
            .demo_frames
            .or(file.diagnostics.demo_frames)
            .or_else(|| env_str("FN64_DEMO_FRAMES").and_then(|v| v.trim().parse().ok()))
            .or(default.diagnostics.demo_frames);

        let demo_zoom_fill = if cli.demo_zoom_fill {
            true
        } else {
            file.diagnostics
                .demo_zoom_fill
                // Deliberately NOT `truthy`: this variable has always rejected
                // anything but exactly "0" or "1" with a panic (demo.rs's old
                // match arms), and loosening it here would silently accept a
                // typo that used to be caught. Preserved verbatim.
                .or_else(|| {
                    env_str("FN64_DEMO_ZOOM_FILL").map(|value| match value.as_str() {
                        "0" => false,
                        "1" => true,
                        other => {
                            panic!("FN64_DEMO_ZOOM_FILL must be exactly 0 or 1, got {other:?}")
                        }
                    })
                })
                .unwrap_or(default.diagnostics.demo_zoom_fill)
        };

        let input_probe = cli
            .input_probe
            .or(file.diagnostics.input_probe)
            .or_else(|| env_str("FN64_INPUT_PROBE"))
            .or(default.diagnostics.input_probe);

        // The trace/census sinks. No flags and no `fn64.toml` keys: these are
        // set by gate harnesses and capture scripts that already spell them as
        // environment variables, and inventing sixteen flags nobody asked for
        // would be the speculative surface the plan forbids. What matters for
        // this task is that the READS move here -- the shell's own modules no
        // longer touch the process environment, and part b can add flags to
        // any of these without hunting for the read site.
        let sinks = SinkKnobs {
            device_timing_trace: env("FN64_DEVICE_TIMING_TRACE"),
            device_timing_trace_id: env("FN64_DEVICE_TIMING_TRACE_ID"),
            device_trace_scope: env("FN64_DEVICE_TRACE_SCOPE"),
            presentation_trace: env("FN64_PRESENTATION_TRACE"),
            presentation_trace_id: env("FN64_PRESENTATION_TRACE_ID"),
            av_sync_cue_id: env("FN64_AV_SYNC_CUE_ID"),
            av_sync_probe: env("FN64_AV_SYNC_PROBE"),
            av_sync_video_hash: env("FN64_AV_SYNC_VIDEO_HASH"),
            av_sync_video_occurrence: env("FN64_AV_SYNC_VIDEO_OCCURRENCE"),
            av_sync_frame_dump: env("FN64_AV_SYNC_FRAME_DUMP").map(PathBuf::from),
            frame_trip: env("FN64_FRAME_TRIP"),
            frame_trip_frames: env("FN64_FRAME_TRIP_FRAMES"),
            pump_census: env("FN64_PUMP_CENSUS"),
            pump_census_pumps: env("FN64_PUMP_CENSUS_PUMPS"),
            pump_census_sequence: env("FN64_PUMP_CENSUS_SEQUENCE"),
            pump_census_warmup: env("FN64_PUMP_CENSUS_WARMUP"),
        };

        Knobs {
            rom,
            shard_root,
            boot_context,
            recomp,
            render: RenderKnobs {
                backend,
                // No flag or file key resolves any of the seven: they are all
                // `diagnostic`-class. The default is what the backend used to
                // compute from the environment itself.
                wgpu: fn64_render_wgpu::WgpuKnobs::default(),
            },
            audio: AudioKnobs {
                enabled: audio_enabled,
                priority: audio_priority,
                priority_join_budget_ms: audio_priority_join_budget_ms,
            },
            video: VideoKnobs {
                hud,
                overscan,
                screenshot_dir,
            },
            boot: BootKnobs {
                cart_handle_vram,
                resident_sections,
            },
            diagnostics: DiagnosticKnobs {
                present_cache,
                frame_dump,
                demo_frames,
                demo_zoom_fill,
                input_probe,
                sinks,
            },
            demo: cli.demo,
            print_config: cli.print_config,
        }
    }

    /// Resolve from the real process: parse `argv`, find and read `fn64.toml`,
    /// and use the real environment as the compatibility layer.
    ///
    /// This is `main`'s entry point and the ONLY path that touches global
    /// state. Everything downstream takes the `Knobs` it returns.
    pub fn from_process() -> Self {
        let cli = Cli::parse();
        // The shard root the config search uses must come from the flag or the
        // env directly: it is what tells us WHERE to look, so it cannot itself
        // wait on the file we have not read yet.
        let shard_root_hint = cli
            .shard_root
            .clone()
            .or_else(|| process_env("FN64_SHARD_ROOT").map(PathBuf::from));
        let file = load_file_config(cli.config.as_deref(), shard_root_hint.as_deref());
        Knobs::resolve(cli, file, process_env)
    }

    /// The resolved configuration as a `fn64.toml` a user can save verbatim.
    ///
    /// Hand-rendered rather than `toml::to_string`-serialized so the output
    /// carries the section comments that make it a usable starting file, and
    /// so every setting appears even when it is at its default -- a dump whose
    /// defaults are invisible does not answer "what is this run doing?", which
    /// is the whole reason `--print-config` exists.
    pub fn to_toml(&self) -> String {
        fn path_line(key: &str, value: &Option<PathBuf>) -> String {
            match value {
                Some(path) => format!("{key} = {:?}\n", path.display().to_string()),
                None => format!("# {key} = \"...\"   (unset)\n"),
            }
        }

        let mut out = String::new();
        out.push_str("# fn64 configuration. Save as `fn64.toml` next to the shard root, or at\n");
        out.push_str("# `$XDG_CONFIG_HOME/fn64/fn64.toml`. A command-line flag still wins.\n\n");
        out.push_str(&path_line("rom", &self.rom));
        out.push_str(&path_line("shard-root", &self.shard_root));
        out.push_str(&path_line("boot-context", &self.boot_context));
        out.push_str(&format!("recomp = {:?}\n", self.recomp.as_str()));
        out.push_str(&format!("render = {:?}\n", self.render.backend.as_str()));

        out.push_str("\n[audio]\n");
        out.push_str(&format!("enabled = {}\n", self.audio.enabled));
        out.push_str(&format!("priority = {}\n", self.audio.priority));
        match self.audio.priority_join_budget_ms {
            Some(ms) => out.push_str(&format!("priority-join-budget-ms = {ms}\n")),
            None => out.push_str("# priority-join-budget-ms = 0   (unset: the ABI default)\n"),
        }

        out.push_str("\n[video]\n");
        out.push_str(&format!("hud = {}\n", self.video.hud));
        match self.video.overscan {
            Some(px) => out.push_str(&format!("overscan = {px}\n")),
            None => out.push_str("# overscan = 0   (unset: the persisted video config wins)\n"),
        }
        out.push_str(&path_line("screenshot-dir", &self.video.screenshot_dir));

        out.push_str("\n[boot]\n");
        out.push_str(&format!(
            "cart-handle-vram = \"0x{:08x}\"\n",
            self.boot.cart_handle_vram
        ));
        out.push_str(&format!(
            "resident-sections = {}\n",
            self.boot.resident_sections
        ));

        out.push_str("\n[diagnostics]\n");
        match &self.diagnostics.present_cache {
            Some(mode) => out.push_str(&format!("present-cache = {mode:?}\n")),
            None => out.push_str("# present-cache = \"...\"   (unset)\n"),
        }
        out.push_str(&path_line("frame-dump", &self.diagnostics.frame_dump));
        match self.diagnostics.demo_frames {
            Some(n) => out.push_str(&format!("demo-frames = {n}\n")),
            None => out.push_str("# demo-frames = 0   (unset: run until closed)\n"),
        }
        out.push_str(&format!(
            "demo-zoom-fill = {}\n",
            self.diagnostics.demo_zoom_fill
        ));
        match &self.diagnostics.input_probe {
            Some(key) => out.push_str(&format!("input-probe = {key:?}\n")),
            None => out.push_str("# input-probe = \"Enter\"   (unset)\n"),
        }
        out
    }
}

/// Where `fn64.toml` is looked for, in order. `--config` short-circuits the
/// search and makes a missing or malformed file FATAL: an explicit path that
/// silently did nothing is exactly the class of quiet failure AGENTS.md's
/// "loud traps, no silent shrugs" exists to stop. A file found by SEARCH, by
/// contrast, is optional -- not having one is the normal case.
pub fn config_search_paths(shard_root: Option<&std::path::Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(root) = shard_root {
        paths.push(root.join("fn64.toml"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        paths.push(PathBuf::from(xdg).join("fn64").join("fn64.toml"));
    }
    // The same platform config directory `input_map.rs` and `video_config.rs`
    // already write into, so all three of fn64's config files live together.
    if let Some(dir) = dirs::config_dir() {
        paths.push(dir.join("fn64").join("fn64.toml"));
    }
    paths
}

/// Read the first `fn64.toml` that exists, or the one `--config` named.
pub fn load_file_config(
    explicit: Option<&std::path::Path>,
    shard_root: Option<&std::path::Path>,
) -> Option<FileConfig> {
    if let Some(path) = explicit {
        let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("fn64: --config {} could not be read: {error}", path.display())
        });
        let config = toml::from_str(&text).unwrap_or_else(|error| {
            panic!("fn64: --config {} is malformed: {error}", path.display())
        });
        println!("[fn64-shell] config loaded from {}", path.display());
        return Some(config);
    }
    for path in config_search_paths(shard_root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str(&text) {
            Ok(config) => {
                println!("[fn64-shell] config loaded from {}", path.display());
                return Some(config);
            }
            Err(error) => {
                // Found but unusable: FATAL, exactly as for `--config`.
                //
                // Falling back to defaults here would be a silent shrug: the
                // user has a config file on disk, believes it is in effect,
                // and would get a run configured by something else entirely
                // -- with a warning that scrolls past in a log they are not
                // reading. A user who wants the defaults deletes or renames
                // the file, which is unambiguous and takes one command.
                panic!(
                    "fn64: config {} is malformed: {error}\n\
                     Fix it, or delete/rename it to run with the defaults.",
                    path.display()
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The precedence contract in one test: a flag beats an environment
    /// variable, and the same environment variable beats the default.
    ///
    /// Both halves use the SAME env closure, so the only difference between
    /// them is whether the flag was passed -- which is what makes this an
    /// assertion about precedence rather than two unrelated resolutions.
    #[test]
    fn env_var_loses_to_flag_and_wins_over_default() {
        let knobs = Knobs::resolve(
            Cli::parse_from(["fn64", "--render", "reference"]),
            None,
            |name| (name == "FN64_RENDER").then(|| "wgpu".to_string()),
        );
        assert_eq!(knobs.render.backend, RenderBackendKind::Reference);
        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
            (name == "FN64_RENDER").then(|| "wgpu".to_string())
        });
        assert_eq!(knobs.render.backend, RenderBackendKind::Wgpu);
    }

    /// The middle rung the previous test skips: `fn64.toml` outranks the
    /// environment, and a flag still outranks the file.
    #[test]
    fn file_config_sits_between_the_flag_and_the_environment() {
        let file = || FileConfig {
            render: Some(RenderBackendKind::Rt64),
            ..FileConfig::default()
        };
        let env = |name: &str| (name == "FN64_RENDER").then(|| "wgpu".to_string());

        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), Some(file()), env);
        assert_eq!(
            knobs.render.backend,
            RenderBackendKind::Rt64,
            "the file must beat the environment"
        );

        let knobs = Knobs::resolve(
            Cli::parse_from(["fn64", "--render", "reference"]),
            Some(file()),
            env,
        );
        assert_eq!(
            knobs.render.backend,
            RenderBackendKind::Reference,
            "the flag must beat the file"
        );
    }

    /// With nothing set anywhere, every knob is its documented default. This
    /// is what `--print-config` prints on a bare run, and what the shell boots
    /// with when a player has configured nothing at all.
    #[test]
    fn nothing_set_anywhere_resolves_to_the_documented_defaults() {
        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |_| None);
        assert_eq!(knobs, Knobs::default());
        assert_eq!(knobs.render.backend, RenderBackendKind::Reference);
        assert_eq!(knobs.recomp, COMPILED_RECOMP_LANE);
        assert!(knobs.audio.enabled);
        assert!(knobs.audio.priority);
        assert!(!knobs.video.hud);
        assert_eq!(knobs.boot.cart_handle_vram, DEFAULT_CART_HANDLE_VRAM);
        assert_eq!(knobs.boot.resident_sections, DEFAULT_RESIDENT_SECTIONS);
    }

    /// The compatibility layer has to preserve the EXACT spellings the old
    /// `std::env::var` sites accepted, or a script that worked yesterday
    /// silently changes meaning today. These are the three that were not a
    /// plain parse.
    #[test]
    fn the_env_compat_layer_preserves_the_historic_spellings() {
        // FN64_AUDIO_PRIORITY: anything but "0" is ON (main.rs:676).
        for (value, expected) in [("0", false), ("1", true), ("", true)] {
            let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
                (name == "FN64_AUDIO_PRIORITY").then(|| value.to_string())
            });
            // An EMPTY value is filtered out by `env_str` and so falls through
            // to the default, which is also `true` -- the same outcome the old
            // `.map(|value| value != "0")` produced.
            assert_eq!(knobs.audio.priority, expected, "FN64_AUDIO_PRIORITY={value:?}");
        }

        // FN64_HUD: 1/true/on, and nothing else (stack.rs:144).
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("on", true),
            ("0", false),
            ("yes", false),
        ] {
            let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
                (name == "FN64_HUD").then(|| value.to_string())
            });
            assert_eq!(knobs.video.hud, expected, "FN64_HUD={value:?}");
        }

        // FN64_NO_AUDIO: PRESENCE disables, whatever the value (main.rs:2527).
        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
            (name == "FN64_NO_AUDIO").then(String::new)
        });
        assert!(!knobs.audio.enabled, "FN64_NO_AUDIO set means no audio");
    }

    /// `--hud` and `--no-audio` are bare flags, so they can only turn their
    /// setting ON. Absent means "defer to the file, then the environment".
    #[test]
    fn bare_flags_turn_a_setting_on_and_absence_defers() {
        let knobs = Knobs::resolve(Cli::parse_from(["fn64", "--hud", "--no-audio"]), None, |_| {
            None
        });
        assert!(knobs.video.hud);
        assert!(!knobs.audio.enabled);

        // Absent, with the file asking for the HUD: the file wins.
        let knobs = Knobs::resolve(
            Cli::parse_from(["fn64"]),
            Some(FileConfig {
                video: FileVideo {
                    hud: Some(true),
                    ..FileVideo::default()
                },
                ..FileConfig::default()
            }),
            |_| None,
        );
        assert!(knobs.video.hud);
    }

    /// The cart handle is hex with an optional `0x`, and it is the one knob
    /// whose default is a specific title's address.
    #[test]
    fn cart_handle_vram_parses_hex_with_or_without_the_prefix() {
        for spelling in ["0x800839A0", "800839a0", " 0x800839a0 "] {
            let knobs = Knobs::resolve(
                Cli::parse_from(["fn64", "--cart-handle-vram", spelling]),
                None,
                |_| None,
            );
            assert_eq!(knobs.boot.cart_handle_vram, 0x8008_39A0, "{spelling:?}");
        }
    }

    /// `--print-config`'s output has to round-trip: a user saves it as
    /// `fn64.toml`, and the next run must resolve to the same thing. If this
    /// breaks, `--print-config` is emitting a file its own parser rejects.
    #[test]
    fn print_config_output_parses_back_to_the_same_knobs() {
        let original = Knobs::resolve(
            Cli::parse_from([
                "fn64",
                "--render",
                "wgpu",
                // The lane must be the compiled one -- `--recomp` asserts
                // rather than selects (see the two tests below).
                "--recomp",
                COMPILED_RECOMP_LANE.as_str(),
                "--hud",
                "--overscan",
                "8",
                "--resident-sections",
                "2",
                "--cart-handle-vram",
                "0x800839a0",
                "--audio-priority-join-budget-ms",
                "4",
            ]),
            None,
            |_| None,
        );
        let text = original.to_toml();
        let parsed: FileConfig = toml::from_str(&text)
            .unwrap_or_else(|error| panic!("--print-config emitted unparseable TOML: {error}\n{text}"));
        let round_tripped = Knobs::resolve(Cli::parse_from(["fn64"]), Some(parsed), |_| None);
        assert_eq!(round_tripped, original);
    }

    /// clap's own derive contract: every flag has a doc comment, so `--help`
    /// is the discoverability surface this task exists to create. A missing
    /// `about` would silently produce a blank help line.
    #[test]
    fn help_names_every_user_facing_knob() {
        use clap::CommandFactory;
        let help = Cli::command().render_long_help().to_string();
        for flag in [
            "--rom",
            "--shard-root",
            "--boot-context",
            "--recomp",
            "--render",
            "--audio-priority",
            "--audio-priority-join-budget-ms",
            "--no-audio",
            "--hud",
            "--overscan",
            "--screenshot-dir",
            "--cart-handle-vram",
            "--resident-sections",
            "--demo",
            "--demo-frames",
            "--demo-zoom-fill",
            "--present-cache",
            "--frame-dump",
            "--config",
            "--print-config",
        ] {
            assert!(help.contains(flag), "--help does not mention {flag}");
        }
        // Each flag names the environment variable it replaces, so a script
        // author reading --help can find the migration without a second doc.
        for env_name in [
            "FN64_RENDER",
            "FN64_RECOMP",
            "FN64_HUD",
            "FN64_NO_AUDIO",
            "FN64_BOOT_CONTEXT",
        ] {
            assert!(help.contains(env_name), "--help does not name {env_name}");
        }
    }

    /// clap rejects a value that is not one of the lane/backend spellings,
    /// rather than falling back to a default the user did not ask for.
    #[test]
    fn an_unknown_backend_is_rejected_not_defaulted() {
        assert!(Cli::try_parse_from(["fn64", "--render", "vulkan"]).is_err());
        assert!(Cli::try_parse_from(["fn64", "--recomp", "cpp"]).is_err());
    }

    /// `--recomp` reports the COMPILED lane, never a request. Precedence does
    /// not apply to it: `build.rs` already decided, and a `Knobs` that claimed
    /// otherwise would let `--print-config` say `rs` for a C-lane binary.
    #[test]
    fn the_recomp_lane_is_the_compiled_one_whatever_was_asked_for() {
        let knobs = Knobs::resolve(
            Cli::parse_from(["fn64", "--recomp", COMPILED_RECOMP_LANE.as_str()]),
            None,
            // An env var naming the OTHER lane must not move it either.
            |name| (name == "FN64_RECOMP").then(|| other_lane().as_str().to_string()),
        );
        assert_eq!(knobs.recomp, COMPILED_RECOMP_LANE);
    }

    /// An EXPLICIT `--recomp` naming the lane this binary is NOT is a loud
    /// failure: the flag cannot change the linked bodies, so accepting it
    /// would be a knob that looks live and does nothing.
    #[test]
    #[should_panic(expected = "was BUILT on the")]
    fn asking_for_the_other_lane_fails_loudly() {
        Knobs::resolve(
            Cli::parse_from(["fn64", "--recomp", other_lane().as_str()]),
            None,
            |_| None,
        );
    }

    /// A `fn64.toml` key is an explicit claim too -- someone wrote it down --
    /// so it asserts on the same terms as the flag.
    #[test]
    #[should_panic(expected = "was BUILT on the")]
    fn a_config_file_naming_the_other_lane_fails_loudly() {
        Knobs::resolve(
            Cli::parse_from(["fn64"]),
            Some(FileConfig {
                recomp: Some(other_lane()),
                ..FileConfig::default()
            }),
            |_| None,
        );
    }

    /// An INHERITED `FN64_RECOMP` is ambient state, not a claim. It is
    /// exported by documented workflows (docs/FAST-LOOP.md) and by the build
    /// itself, so a shell launched from that session legitimately sees a value
    /// that no longer describes the binary. It must WARN and continue --
    /// panicking would break those workflows to punish the user for an
    /// environment they were told to set.
    #[test]
    fn an_inherited_recomp_naming_the_other_lane_warns_and_continues() {
        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
            (name == "FN64_RECOMP").then(|| other_lane().as_str().to_string())
        });
        assert_eq!(
            knobs.recomp, COMPILED_RECOMP_LANE,
            "the compiled lane still wins"
        );
    }

    /// The env compat layer must not be able to launder a disagreeing value
    /// into a passing assertion either: an inherited value that AGREES is
    /// simply silent.
    #[test]
    fn an_inherited_recomp_naming_the_compiled_lane_is_silent() {
        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
            (name == "FN64_RECOMP").then(|| COMPILED_RECOMP_LANE.as_str().to_string())
        });
        assert_eq!(knobs.recomp, COMPILED_RECOMP_LANE);
    }

    /// The bare `ROM` variable is the LAST rung of the ROM chain -- the
    /// build-time intake contract has always used it, so it must keep working
    /// -- but the prefixed `FN64_ROM` outranks it, and a flag outranks both.
    #[test]
    fn the_bare_rom_variable_is_the_last_rung() {
        let env = |name: &str| match name {
            "FN64_ROM" => Some("/prefixed.z64".to_string()),
            "ROM" => Some("/bare.z64".to_string()),
            _ => None,
        };

        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, |name| {
            (name == "ROM").then(|| "/bare.z64".to_string())
        });
        assert_eq!(
            knobs.rom.unwrap(),
            PathBuf::from("/bare.z64"),
            "bare ROM alone must still work -- every runner script uses it"
        );

        let knobs = Knobs::resolve(Cli::parse_from(["fn64"]), None, env);
        assert_eq!(
            knobs.rom.unwrap(),
            PathBuf::from("/prefixed.z64"),
            "FN64_ROM outranks the unprefixed name"
        );

        let knobs = Knobs::resolve(Cli::parse_from(["fn64", "--rom", "/flag.z64"]), None, env);
        assert_eq!(knobs.rom.unwrap(), PathBuf::from("/flag.z64"));
    }

    /// Whichever lane this test binary was not compiled on.
    fn other_lane() -> RecompLane {
        match COMPILED_RECOMP_LANE {
            RecompLane::Rs => RecompLane::C,
            RecompLane::C => RecompLane::Rs,
        }
    }

    /// The shard root is searched FIRST, so a game package can ship its own
    /// `fn64.toml` and have it beat the user's global one.
    #[test]
    fn the_shard_root_config_is_searched_before_the_global_one() {
        let paths = config_search_paths(Some(std::path::Path::new("/shard")));
        assert_eq!(paths.first().unwrap(), std::path::Path::new("/shard/fn64.toml"));
        assert!(paths.len() > 1, "the global config directory is still searched");
    }

    /// A malformed config is FATAL whether it was named by `--config` or found
    /// by the search. Falling back to defaults would be a silent shrug: the
    /// user has a config file on disk, believes it is in effect, and would get
    /// a run configured by something else -- behind a warning that scrolls
    /// past in a log nobody reads.
    #[test]
    #[should_panic(expected = "is malformed")]
    fn a_malformed_searched_config_is_fatal_not_a_silent_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("fn64.toml"), "render = [not toml\n").expect("write");
        load_file_config(None, Some(dir.path()));
    }

    /// Same rule, reached the other way.
    #[test]
    #[should_panic(expected = "is malformed")]
    fn a_malformed_explicit_config_is_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("custom.toml");
        std::fs::write(&path, "render = [not toml\n").expect("write");
        load_file_config(Some(&path), None);
    }

    /// An unknown KEY is malformed too (`deny_unknown_fields`): a typo like
    /// `renderer = "wgpu"` must not be silently ignored, which would leave the
    /// user on the default wondering why their setting did nothing.
    #[test]
    #[should_panic(expected = "is malformed")]
    fn an_unknown_config_key_is_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("fn64.toml"), "renderer = \"wgpu\"\n").expect("write");
        load_file_config(None, Some(dir.path()));
    }

    /// The searched file is still OPTIONAL: not having one is the normal case
    /// and must resolve to the defaults without complaint. Only a file that
    /// EXISTS and is broken is fatal.
    ///
    /// Asserted through the search-path list rather than by calling
    /// `load_file_config` on an empty directory: that would fall through to
    /// the real `dirs::config_dir()`, so the test would pass or fail on
    /// whether the developer running it happens to have a personal
    /// `fn64.toml`. What is actually under test is that an absent file is not
    /// an error, and the fatal cases above already prove a present-and-broken
    /// one is.
    #[test]
    fn a_missing_searched_config_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let candidate = &config_search_paths(Some(dir.path()))[0];
        assert_eq!(candidate, &dir.path().join("fn64.toml"));
        assert!(
            !candidate.exists(),
            "the shard-root candidate is absent, which must not be an error"
        );
        assert!(std::fs::read_to_string(candidate).is_err());
    }
}
