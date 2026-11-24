//! Deleting what the server no longer has (Jellyfin is the master). Only files the sync manages
//! are candidates:
//!
//! - audio files (`mp3 flac ogg opus m4a wav aac`, any case) that no server track maps to
//! - `cover.jpg` in an `Artist/Album` folder without synced tracks
//! - `.m3u` files directly in the Playlists folder whose playlist is gone
//! - `.part` files of audio files no server track maps to
//!
//! Every other file is left alone. Symlinks are neither followed nor deleted.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::playlists;

pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "ogg", "opus", "m4a", "wav", "aac"];
pub const COVER_NAME: &str = "cover.jpg";
/// Deletions are refused only from this many audio files on; fewer can always go.
pub const GUARD_MIN_FILES: usize = 10;

/// What the server still has.
pub struct Expected {
    /// The local path of every server track.
    pub tracks: HashSet<PathBuf>,
    /// The album folders whose `cover.jpg` stays (none in the flat layout).
    pub album_folders: HashSet<PathBuf>,
    /// The playlist file names to keep, or `None` to leave every `.m3u` alone (playlists off, or
    /// the playlist list could not be read).
    pub playlists: Option<HashSet<String>>,
    /// Files of the other layout a dry run would have moved into place, which are no longer there
    /// then.
    pub moved_away: HashSet<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Cover,
    Playlist,
    Part,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Sorted by path.
    pub removals: Vec<Removal>,
    /// Audio files in the music folder before any deletion.
    pub local_audio: usize,
}

impl Plan {
    pub fn audio_removals(&self) -> usize {
        self.removals
            .iter()
            .filter(|removal| removal.kind == Kind::Audio)
            .count()
    }
}

fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|audio| audio.eq_ignore_ascii_case(extension))
        })
}

fn has_extension(path: &Path, wanted: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// Every regular file below `dir`, without following symlinks.
fn regular_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            regular_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Classifies a local file: `Some(kind)` when the sync manages it and the server no longer has it.
fn stale_kind(music_dir: &Path, path: &Path, expected: &Expected) -> Option<Kind> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    if has_audio_extension(path) {
        return (!expected.tracks.contains(path)).then_some(Kind::Audio);
    }
    if name == COVER_NAME && parent.parent().and_then(Path::parent) == Some(music_dir) {
        return (!expected.album_folders.contains(parent)).then_some(Kind::Cover);
    }
    if parent == music_dir.join(playlists::FOLDER) && has_extension(path, playlists::EXTENSION) {
        let keep = expected.playlists.as_ref()?;
        return (!keep.contains(name)).then_some(Kind::Playlist);
    }
    if let Some(target) = name.strip_suffix(".part") {
        let target = parent.join(target);
        return (has_audio_extension(&target) && !expected.tracks.contains(&target)).then_some(Kind::Part);
    }
    None
}

/// Lists what would be deleted. Reads the folder but changes nothing.
pub fn plan(music_dir: &Path, expected: &Expected) -> io::Result<Plan> {
    let mut files = Vec::new();
    if music_dir.is_dir() {
        regular_files(music_dir, &mut files)?;
    }
    files.retain(|path| !expected.moved_away.contains(path));
    files.sort();
    let local_audio = files.iter().filter(|path| has_audio_extension(path)).count();
    let removals = files
        .into_iter()
        .filter_map(|path| stale_kind(music_dir, &path, expected).map(|kind| Removal { path, kind }))
        .collect();
    Ok(Plan { removals, local_audio })
}

/// The mass-delete guard: deleting `removals` of `local` audio files is allowed unless it is at
/// least [`GUARD_MIN_FILES`] files and more than `max_percent` percent of them.
pub fn guard_allows(removals: usize, local: usize, max_percent: u8) -> bool {
    removals < GUARD_MIN_FILES || removals * 100 <= usize::from(max_percent) * local
}

/// Removes the empty folders below `music_dir`, deepest first, as far as it can (like the bash
/// script's `find -empty -delete 2>/dev/null`). `music_dir` itself always stays.
pub fn remove_empty_folders(music_dir: &Path) {
    let Ok(entries) = fs::read_dir(music_dir) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            let path = entry.path();
            remove_empty_folders(&path);
            // Fails, and so keeps the folder, unless it is empty.
            let _ = fs::remove_dir(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::testdir::TestDir;

    fn expected(dir: &Path, tracks: &[&str], playlists: Option<&[&str]>) -> Expected {
        let tracks: HashSet<PathBuf> = tracks.iter().map(|t| dir.join(t)).collect();
        Expected {
            album_folders: tracks
                .iter()
                .filter_map(|t| t.parent().map(Path::to_path_buf))
                .collect(),
            tracks,
            playlists: playlists.map(|names| names.iter().map(|n| n.to_string()).collect()),
            moved_away: HashSet::new(),
        }
    }

    fn relative(dir: &Path, plan: &Plan) -> Vec<(String, Kind)> {
        plan.removals
            .iter()
            .map(|r| (r.path.strip_prefix(dir).expect("below").display().to_string(), r.kind))
            .collect()
    }

    #[test]
    fn lists_only_managed_files_the_server_no_longer_has() {
        let dir = TestDir::new("prune");
        let root = dir.path();
        for file in [
            "A/One/Keep.flac",
            "A/One/cover.jpg",
            "A/One/notes.txt",
            "A/Gone/Old.MP3",
            "A/Gone/cover.jpg",
            "A/Gone/folder.jpg",
            "A/One/Keep.flac.part",
            "A/One/Gone.flac.part",
            "Flat.mp3",
            "cover.jpg",
            "Playlists/Mix.m3u",
            "Playlists/Old.M3U",
            "Playlists/notes.txt",
            "Deep/a/b/cover.jpg",
            "Other/Thing.flac.txt",
        ] {
            dir.write(file, b"x");
        }
        std::os::unix::fs::symlink(root.join("A/One/Keep.flac"), root.join("A/Link.flac")).expect("symlink");
        let plan = plan(root, &expected(root, &["A/One/Keep.flac"], Some(&["Mix.m3u"]))).expect("plan");
        assert_eq!(
            relative(root, &plan),
            [
                ("A/Gone/Old.MP3".to_string(), Kind::Audio),
                ("A/Gone/cover.jpg".to_string(), Kind::Cover),
                ("A/One/Gone.flac.part".to_string(), Kind::Part),
                ("Flat.mp3".to_string(), Kind::Audio),
                ("Playlists/Old.M3U".to_string(), Kind::Playlist),
            ]
        );
        assert_eq!(plan.local_audio, 3);
        assert_eq!(plan.audio_removals(), 2);
    }

    #[test]
    fn leaves_playlists_alone_when_unknown_and_skips_moved_files() {
        let dir = TestDir::new("prune-playlists");
        let root = dir.path();
        dir.write("Playlists/Old.m3u", b"x");
        dir.write("Song.mp3", b"x");
        let mut keep = expected(root, &["A/B/Song.mp3"], None);
        keep.moved_away.insert(root.join("Song.mp3"));
        let plan = plan(root, &keep).expect("plan");
        assert!(plan.removals.is_empty());
        assert_eq!(plan.local_audio, 0);
    }

    #[test]
    fn the_guard_refuses_mass_deletions() {
        assert!(guard_allows(9, 10, 50), "fewer than 10 files may always go");
        assert!(guard_allows(10, 20, 50), "exactly 50% is allowed");
        assert!(!guard_allows(11, 20, 50));
        assert!(!guard_allows(10, 10, 99));
        assert!(guard_allows(10, 10, 100));
        assert!(guard_allows(1000, 1000, 100));
        assert!(!guard_allows(10, 1000, 0));
        assert!(guard_allows(0, 0, 0));
    }

    #[test]
    fn removes_empty_folders_but_never_the_music_folder() {
        let dir = TestDir::new("empty");
        let root = dir.path();
        fs::create_dir_all(root.join("A/B/C")).expect("mkdir");
        fs::create_dir_all(root.join(".hidden")).expect("mkdir");
        dir.write("Keep/Album/song.mp3", b"x");
        fs::create_dir_all(root.join("Keep/Empty")).expect("mkdir");
        remove_empty_folders(root);
        let mut left: Vec<_> = fs::read_dir(root)
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["Keep"]);
        assert!(!root.join("Keep/Empty").exists());
        let empty = TestDir::new("empty-root");
        remove_empty_folders(empty.path());
        assert!(empty.path().is_dir());
    }
}
