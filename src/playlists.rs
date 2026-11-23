//! Playlists as extended M3U files in `<music>/Playlists/`, with paths relative to that folder so
//! the whole music folder can be moved or copied to another device.

use std::fs;
use std::io;
use std::path::Path;

use crate::api::Playlist;
use crate::fsutil;
use crate::layout::{self, Track};

pub const FOLDER: &str = "Playlists";
pub const EXTENSION: &str = "m3u";
const UNKNOWN_PLAYLIST: &str = "Unknown Playlist";
const TICKS_PER_SECOND: i64 = 10_000_000;

/// Gives each playlist its file name: the sanitised name plus `.m3u`, numbered like tracks when
/// two names collide. Returned sorted by name, then id.
pub fn name_files(playlists: Vec<Playlist>) -> Vec<(Playlist, String)> {
    let mut named: Vec<(String, Playlist)> = playlists
        .into_iter()
        .map(|playlist| {
            let name = layout::sanitize(playlist.name.as_deref().unwrap_or(""), UNKNOWN_PLAYLIST);
            (name, playlist)
        })
        .collect();
    named.sort_by(|a, b| (&a.0, &a.1.id).cmp(&(&b.0, &b.1.id)));
    let names: Vec<String> = named.iter().map(|(name, _)| name.clone()).collect();
    let file_names = layout::unique_file_names(&names, EXTENSION);
    named
        .into_iter()
        .zip(file_names)
        .map(|((_, playlist), file_name)| (playlist, file_name))
        .collect()
}

/// One line of text: control characters (line breaks) would split an M3U entry.
fn one_line(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Extended M3U: `#EXTM3U`, then `#EXTINF:<seconds>,<Artist> - <Title>` and the path relative to
/// the Playlists folder for each track. An unknown length is written as -1.
pub fn render(tracks: &[&Track]) -> String {
    let mut text = String::from("#EXTM3U\n");
    for track in tracks {
        let seconds = track.run_time_ticks.map_or(-1, |ticks| ticks / TICKS_PER_SECOND);
        text.push_str(&format!(
            "#EXTINF:{seconds},{} - {}\n../{}\n",
            one_line(&track.artist),
            one_line(&track.title),
            track.relative()
        ));
    }
    text
}

/// Whether `path` is missing or holds something other than `content`.
pub fn needs_writing(path: &Path, content: &str) -> bool {
    fs::read(path).map_or(true, |existing| existing != content.as_bytes())
}

/// Writes a playlist file atomically, creating the Playlists folder as needed.
pub fn write(path: &Path, content: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    fsutil::write_atomic(path, content.as_bytes(), 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::testdir::TestDir;

    fn track(artist: &str, title: &str, relative: (&str, &str, &str), ticks: Option<i64>) -> Track {
        Track {
            id: title.to_string(),
            artist: artist.to_string(),
            title: title.to_string(),
            album_id: None,
            album_has_image: false,
            has_image: false,
            run_time_ticks: ticks,
            size: None,
            artist_dir: relative.0.to_string(),
            album_dir: relative.1.to_string(),
            file_name: relative.2.to_string(),
            flat_name: relative.2.to_string(),
        }
    }

    fn playlist(id: &str, name: Option<&str>) -> Playlist {
        Playlist {
            id: id.to_string(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn renders_extended_m3u_with_relative_paths() {
        let first = track(
            "AC/DC",
            "Hells Bells",
            ("ACDC", "Back in Black", "Hells Bells.flac"),
            Some(3_123_456_789),
        );
        let second = track(
            "Unknown Artist",
            "Line\nBreak",
            ("Unknown Artist", "Unknown Album", "Line\nBreak.mp3"),
            None,
        );
        assert_eq!(
            render(&[&first, &second, &first]),
            "#EXTM3U\n\
             #EXTINF:312,AC/DC - Hells Bells\n../ACDC/Back in Black/Hells Bells.flac\n\
             #EXTINF:-1,Unknown Artist - Line Break\n../Unknown Artist/Unknown Album/Line\nBreak.mp3\n\
             #EXTINF:312,AC/DC - Hells Bells\n../ACDC/Back in Black/Hells Bells.flac\n"
        );
        assert_eq!(render(&[]), "#EXTM3U\n");
    }

    #[test]
    fn names_playlist_files_uniquely() {
        let named = name_files(vec![
            playlist("b", Some("Road: Trip")),
            playlist("a", Some("Road Trip")),
            playlist("c", None),
            playlist("d", Some("Chill")),
        ]);
        let files: Vec<(&str, &str)> = named.iter().map(|(p, f)| (p.id.as_str(), f.as_str())).collect();
        assert_eq!(
            files,
            [
                ("d", "Chill.m3u"),
                ("a", "Road Trip.m3u"),
                ("b", "Road Trip (2).m3u"),
                ("c", "Unknown Playlist.m3u")
            ]
        );
    }

    #[test]
    fn writes_only_changed_content() {
        let dir = TestDir::new("playlists");
        let path = dir.path().join(FOLDER).join("Mix.m3u");
        assert!(needs_writing(&path, "#EXTM3U\n"));
        write(&path, "#EXTM3U\n").expect("write");
        assert!(!needs_writing(&path, "#EXTM3U\n"));
        assert!(needs_writing(&path, "#EXTM3U\n#EXTINF:1,a - b\n../a\n"));
    }
}
