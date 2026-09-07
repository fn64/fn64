//! Where a ROM's save file lives.
//!
//! Pure path derivation, split from `main.rs`'s `save_storage_for_rom`, which
//! keeps the I/O (creating the directory, opening the file, and falling back
//! to an in-memory store when either fails). Deciding the path and trying to
//! open it are different jobs, and only the first can be tested without a
//! filesystem.
//!
//! The path is `<data_dir>/fn64/saves/<rom-file-stem>.sav`. `dirs::data_dir()`
//! is the same platform-data-dir crate `InputConfig` already uses for its
//! config file (see input_map.rs); saves use `data_dir` rather than
//! `config_dir` because a save is user data, not configuration.
//!
//! This never fails. It only picks where the caller will *try* to open a
//! file; the caller is what actually falls further back if that path cannot
//! be opened.

/// The saves directory: `<data_dir>/fn64/saves`, or `.fn64/saves` under the
/// current directory when the platform reports no data dir (an unusual or
/// headless host).
///
/// `data_dir` is taken as an argument rather than read from `dirs` so the
/// fallback is reachable from a test: the real caller passes
/// `dirs::data_dir()`.
pub fn saves_dir(data_dir: Option<std::path::PathBuf>) -> std::path::PathBuf {
    data_dir
        .map(|dir| dir.join("fn64").join("saves"))
        .unwrap_or_else(|| std::path::PathBuf::from(".fn64").join("saves"))
}

/// Per-ROM save file path: `<saves_dir>/<rom-file-stem>.sav`.
///
/// A ROM path with no usable file stem (a bare `/`, or a path ending in
/// `..`) yields `rom.sav` rather than an empty or malformed name -- the point
/// is that this function always returns *some* openable-looking path, and
/// lets the caller's open decide the outcome.
pub fn save_path_for_rom(
    saves_dir: &std::path::Path,
    rom_path: &std::path::Path,
) -> std::path::PathBuf {
    let stem = rom_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rom".to_string());
    saves_dir.join(format!("{stem}.sav"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn saves_live_under_fn64_saves_in_the_platform_data_dir() {
        assert_eq!(
            saves_dir(Some(PathBuf::from("/home/p/.local/share"))),
            Path::new("/home/p/.local/share/fn64/saves")
        );
    }

    /// A host with no data dir still gets a usable relative location rather
    /// than a panic or an empty path.
    #[test]
    fn a_host_without_a_data_dir_falls_back_to_a_relative_dot_fn64_dir() {
        assert_eq!(saves_dir(None), Path::new(".fn64/saves"));
    }

    /// The stem, not the file name: the extension is replaced by `.sav`, so
    /// the same title in two container formats does not collide with itself
    /// under two names.
    #[test]
    fn the_rom_extension_is_replaced_by_sav() {
        let dir = saves_dir(Some(PathBuf::from("/data")));
        assert_eq!(
            save_path_for_rom(&dir, Path::new("/roms/oot-ntsc-1.0.z64")),
            Path::new("/data/fn64/saves/oot-ntsc-1.0.sav")
        );
    }

    /// Only the last extension goes. A dotted stem keeps its dots, so two
    /// revisions distinguished by a dotted suffix stay distinct files.
    #[test]
    fn a_dotted_stem_keeps_every_dot_but_the_last_extension() {
        let dir = PathBuf::from("/s");
        assert_eq!(
            save_path_for_rom(&dir, Path::new("/roms/wm2000.v1.2.z64")),
            Path::new("/s/wm2000.v1.2.sav")
        );
    }

    /// Different ROMs never share a save file -- the property that keeps one
    /// title from overwriting another's progress.
    #[test]
    fn distinct_rom_names_map_to_distinct_save_files() {
        let dir = PathBuf::from("/s");
        let a = save_path_for_rom(&dir, Path::new("/a/wm2000.z64"));
        let b = save_path_for_rom(&dir, Path::new("/b/nomercy.z64"));
        assert_ne!(a, b);
    }

    /// The ROM's own directory is irrelevant: the save lands in the saves
    /// dir, so a ROM on read-only media still saves somewhere writable.
    #[test]
    fn the_save_ignores_the_roms_own_directory() {
        let dir = PathBuf::from("/s");
        assert_eq!(
            save_path_for_rom(&dir, Path::new("/Volumes/readonly/g.z64")),
            save_path_for_rom(&dir, Path::new("/home/me/g.z64"))
        );
    }

    /// A path with no file stem still produces an openable-looking name. This
    /// function never fails; the caller's open is what decides the outcome.
    #[test]
    fn a_rom_path_without_a_stem_becomes_rom_sav() {
        let dir = PathBuf::from("/s");
        assert_eq!(
            save_path_for_rom(&dir, Path::new("/")),
            Path::new("/s/rom.sav")
        );
        assert_eq!(
            save_path_for_rom(&dir, Path::new("..")),
            Path::new("/s/rom.sav")
        );
    }

    /// A ROM with no extension keeps its whole name as the stem.
    #[test]
    fn an_extensionless_rom_keeps_its_whole_name() {
        assert_eq!(
            save_path_for_rom(Path::new("/s"), Path::new("/roms/wm2000")),
            Path::new("/s/wm2000.sav")
        );
    }
}
