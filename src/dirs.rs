//! The standard folders: home, the XDG config, cache and runtime folders, and the music folder
//! from `user-dirs.dirs`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn home() -> Option<PathBuf> {
    env::home_dir().filter(|dir| dir.is_absolute())
}

/// An XDG base folder: the variable when it holds an absolute path (the spec says to ignore
/// relative ones), else `fallback` below the home folder.
fn xdg_dir(variable: &str, fallback: &str) -> Option<PathBuf> {
    env::var_os(variable)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home().map(|home| home.join(fallback)))
}

pub fn config_home() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

pub fn cache_home() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_RUNTIME_DIR`, which has no fallback of its own.
pub fn runtime_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

/// The default music folder: `XDG_MUSIC_DIR` from `user-dirs.dirs`, else `~/Music`.
pub fn default_music_dir() -> Option<PathBuf> {
    let home = home()?;
    let from_user_dirs = config_home()
        .and_then(|config| fs::read_to_string(config.join("user-dirs.dirs")).ok())
        .and_then(|text| music_dir_from_user_dirs(&text, &home));
    Some(from_user_dirs.unwrap_or_else(|| home.join("Music")))
}

/// Reads `XDG_MUSIC_DIR="$HOME/…"` or `XDG_MUSIC_DIR="/absolute"` as xdg-user-dirs writes it.
/// A folder equal to the home folder means "disabled" in that format, so it gives `None`.
fn music_dir_from_user_dirs(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("XDG_MUSIC_DIR="))
        .next_back()?;
    let inner = value.trim().strip_prefix('"')?.strip_suffix('"')?;
    let unescaped = unescape(inner);
    let dir = match unescaped.strip_prefix("$HOME/") {
        Some(rest) => home.join(rest),
        None if unescaped.starts_with('/') => PathBuf::from(&unescaped),
        None => return None,
    };
    (dir != home).then_some(dir)
}

/// Undoes the backslash escapes xdg-user-dirs writes inside double quotes.
fn unescape(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            output.extend(chars.next());
        } else {
            output.push(c);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_music_folder_from_user_dirs() {
        let home = Path::new("/home/user");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_MUSIC_DIR=\"$HOME/Audio/Music\"\n";
        assert_eq!(
            music_dir_from_user_dirs(text, home),
            Some(PathBuf::from("/home/user/Audio/Music"))
        );
        let absolute = "XDG_MUSIC_DIR=\"/srv/music \\\"live\\\"\"\n";
        assert_eq!(
            music_dir_from_user_dirs(absolute, home),
            Some(PathBuf::from("/srv/music \"live\""))
        );
    }

    #[test]
    fn ignores_disabled_relative_or_missing_entries() {
        let home = Path::new("/home/user");
        assert_eq!(music_dir_from_user_dirs("XDG_MUSIC_DIR=\"$HOME/\"\n", home), None);
        assert_eq!(music_dir_from_user_dirs("XDG_MUSIC_DIR=\"$HOME\"\n", home), None);
        assert_eq!(music_dir_from_user_dirs("XDG_MUSIC_DIR=\"Music\"\n", home), None);
        assert_eq!(music_dir_from_user_dirs("XDG_MUSIC_DIR=\"$HOMEX/a\"\n", home), None);
        assert_eq!(
            music_dir_from_user_dirs("XDG_VIDEOS_DIR=\"$HOME/Videos\"\n", home),
            None
        );
    }
}
