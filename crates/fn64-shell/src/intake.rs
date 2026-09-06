//! Operator-facing message text the shell prints but never varies at runtime.
//!
//! The intake-contract notice and the hotkey hint are pure functions of their
//! arguments: no environment reads, no I/O, no clock. They were inline
//! `eprintln!`/`println!` literals in `main.rs`, where nothing could assert
//! their wording. The strings are what a user pastes into a symptom report and
//! what a content-free build owes an operator as an honest report, so the
//! tests below pin them exactly -- a silently reworded intake contract is a
//! changed contract.
//!
//! Every function here returns the message; the caller does the printing, and
//! the caller keeps the choice of stream (the intake notice goes to stderr and
//! exits 2; the hotkey hint goes to stdout).

/// The content-free build's intake contract: what was not linked, how to see
/// the UI anyway, and how to rebuild with a game.
///
/// Printed to **stderr** by a `#[cfg(not(fn64_game_linked))]` build that was
/// not asked for `--demo`, immediately before `exit(2)`.
pub fn contract_notice() -> String {
    // `\x20` (not a literal space) begins the indented lines: a leading space
    // after a `\` line-continuation in a Rust string literal is stripped, so
    // the escape is what keeps the command lines indented. Preserved verbatim
    // from main.rs.
    "fn64-shell: built WITHOUT a linked game (RECOMPILED_DIR was unset at build time).\n\
     \n\
     For a content-free UI demo (synthetic framebuffer, no ROM required):\n\
     \n\
     \x20 cargo run -p fn64-shell -- --demo\n\
     \n\
     To get a live, playable window, rebuild with the game intake env vars set (same\n\
     contract as examples/oot-boot), e.g. for OoT:\n\
     \n\
     \x20 RECOMPILED_DIR=.../OOTU/RecompiledFuncs \\\n\
     \x20 ROM=.../oot-ntsc-1.0.z64 \\\n\
     \x20 cargo run -p fn64-shell\n\
     \n\
     (Audio tasks execute live IMEM through fn64's clean-room LLE interpreter.)"
        .to_string()
}

/// The one place the shell chords are announced to a player who never opens a
/// source file.
///
/// `screenshot_dir` is interpolated because F2's destination is configurable;
/// the overlay's own hint line is shared with `--demo` (which has no
/// screenshot handler), so F2 is advertised from here rather than there -- a
/// hint that lies in one of two modes is worse than no hint.
pub fn hotkey_hint(screenshot_dir: &std::path::Path) -> String {
    format!(
        "[fn64-shell] hotkeys: F1 settings · F2 screenshot (PNG into {}/, override with \
         --screenshot-dir <dir>) · F3 stack/fps HUD (--hud starts it open) · F11 fullscreen · \
         Esc exit",
        screenshot_dir.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole message, byte for byte. A wording change that reaches a user
    /// must be a deliberate edit to this expectation, not a side effect of
    /// refactoring the module it lives in.
    #[test]
    fn contract_notice_is_pinned_verbatim() {
        assert_eq!(
            contract_notice(),
            concat!(
                "fn64-shell: built WITHOUT a linked game (RECOMPILED_DIR was unset at build time).\n",
                "\n",
                "For a content-free UI demo (synthetic framebuffer, no ROM required):\n",
                "\n",
                "  cargo run -p fn64-shell -- --demo\n",
                "\n",
                "To get a live, playable window, rebuild with the game intake env vars set (same\n",
                "contract as examples/oot-boot), e.g. for OoT:\n",
                "\n",
                "  RECOMPILED_DIR=.../OOTU/RecompiledFuncs \\\n",
                "  ROM=.../oot-ntsc-1.0.z64 \\\n",
                "  cargo run -p fn64-shell\n",
                "\n",
                "(Audio tasks execute live IMEM through fn64's clean-room LLE interpreter.)",
            )
        );
    }

    /// The indentation is content: it is what makes the three command lines
    /// copy-pasteable as a block. `\x20` exists precisely because the
    /// continuation would otherwise eat it, so assert the space survived.
    #[test]
    fn contract_notice_indents_every_command_line_by_two_spaces() {
        let notice = contract_notice();
        let indented: Vec<&str> = notice
            .lines()
            .filter(|line| line.starts_with("  "))
            .collect();
        assert_eq!(
            indented,
            vec![
                "  cargo run -p fn64-shell -- --demo",
                "  RECOMPILED_DIR=.../OOTU/RecompiledFuncs \\",
                "  ROM=.../oot-ntsc-1.0.z64 \\",
                "  cargo run -p fn64-shell",
            ]
        );
    }

    /// The notice's job is to name the way out. Both escape hatches -- the
    /// content-free demo and the rebuild-with-intake path -- must be present,
    /// and the variable that was unset must be named.
    #[test]
    fn contract_notice_names_the_missing_variable_and_both_escape_hatches() {
        let notice = contract_notice();
        assert!(notice.contains("RECOMPILED_DIR was unset at build time"));
        assert!(notice.contains("--demo"));
        assert!(notice.contains("ROM=.../oot-ntsc-1.0.z64"));
        // No trailing newline: `eprintln!` supplies the line break, and a
        // second one would print a blank line before the exit.
        assert!(!notice.ends_with('\n'));
    }

    #[test]
    fn hotkey_hint_is_pinned_and_interpolates_the_screenshot_dir() {
        assert_eq!(
            hotkey_hint(std::path::Path::new("/tmp/shots")),
            "[fn64-shell] hotkeys: F1 settings · F2 screenshot (PNG into /tmp/shots/, override \
             with --screenshot-dir <dir>) · F3 stack/fps HUD (--hud starts it open) · F11 \
             fullscreen · Esc exit"
        );
    }

    /// All five chords the shell handles before the keyboard reaches the game
    /// are advertised. A chord that exists but is unlisted is undiscoverable.
    #[test]
    fn hotkey_hint_lists_every_shell_chord() {
        let hint = hotkey_hint(std::path::Path::new("shots"));
        for chord in ["F1", "F2", "F3", "F11", "Esc"] {
            assert!(hint.contains(chord), "hotkey hint omits {chord}: {hint}");
        }
    }

    /// A relative directory renders relative -- the hint reports the
    /// configured path, it does not canonicalize or invent one.
    #[test]
    fn hotkey_hint_does_not_absolutize_a_relative_dir() {
        assert!(hotkey_hint(std::path::Path::new("shots")).contains("PNG into shots/,"));
    }
}
