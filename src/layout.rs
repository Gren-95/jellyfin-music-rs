//! Where each server track goes: `<Artist>/<Album>/<Title>.<ext>` below the music folder, or
//! `<Title>.<ext>` straight in it in the flat layout, with names sanitised exactly like the bash
//! script did for ordinary names. Tracks that would land on the same file are numbered, so every
//! track keeps a file of its own.

use std::collections::HashSet;
use std::hash::Hash;
use std::path::{Path, PathBuf};

use crate::api::Item;

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";
pub const UNKNOWN_ALBUM: &str = "Unknown Album";
pub const UNKNOWN_TRACK: &str = "Unknown Track";
const DEFAULT_EXTENSION: &str = "mp3";
/// Longest name part in bytes, to stay below the usual 255-byte file name limit.
const MAX_PART_BYTES: usize = 200;
/// Characters the bash script removed (`tr -d '/:*?"<>|\\'`).
const REMOVED: [char; 9] = ['/', ':', '*', '?', '"', '<', '>', '|', '\\'];

/// Makes a name safe as one path part, like the bash script's `sanitize`: drops `/ : * ? " < >
/// | \`, trims spaces at both ends and cuts to 200 bytes. Unlike the script's `cut`, the cut
/// never splits a UTF-8 character. A part that ends up empty, `.` or `..` becomes `fallback`.
pub fn sanitize(name: &str, fallback: &str) -> String {
    let kept: String = name.chars().filter(|c| !REMOVED.contains(c)).collect();
    let part = truncate(kept.trim_matches(' '), MAX_PART_BYTES);
    if part.is_empty() || part == "." || part == ".." {
        fallback.to_string()
    } else {
        part.to_string()
    }
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let end = (0..=max).rev().find(|&index| text.is_char_boundary(index)).unwrap_or(0);
    &text[..end]
}

/// The extension of the server file: what follows the last `.` of its file name, else `mp3`.
fn extension(server_path: Option<&str>) -> String {
    let file_name = server_path.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    match file_name.rsplit_once('.') {
        Some((_, extension)) if !extension.is_empty() => extension.to_string(),
        _ => DEFAULT_EXTENSION.to_string(),
    }
}

/// How the tracks are arranged in the music folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `Artist/Album/Title.ext`, with covers and playlists beside the tracks.
    Albums,
    /// `Title.ext` straight in the music folder, without covers or playlists.
    Flat,
}

impl Layout {
    fn other(self) -> Layout {
        match self {
            Layout::Albums => Layout::Flat,
            Layout::Flat => Layout::Albums,
        }
    }
}

/// A server track and its place in the local folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: String,
    /// The album artist as the server gives it, for messages and playlists.
    pub artist: String,
    /// The track name as the server gives it.
    pub title: String,
    pub album_id: Option<String>,
    /// Whether the album has a primary image to use as `cover.jpg`.
    pub album_has_image: bool,
    /// Whether the track has a primary image of its own, the fallback for `cover.jpg`.
    pub has_image: bool,
    pub run_time_ticks: Option<i64>,
    /// The size the server lists, when known.
    pub size: Option<u64>,
    /// The layout the sync uses.
    pub layout: Layout,
    pub artist_dir: String,
    pub album_dir: String,
    /// `Title.ext` in the album folder, or `Title (2).ext` when another track of the album has
    /// the same name.
    pub album_file_name: String,
    /// `Title.ext` in the music folder, or `Title (2).ext` when any other track has the same name.
    pub flat_file_name: String,
}

impl Track {
    fn relative_in(&self, layout: Layout) -> String {
        match layout {
            Layout::Albums => format!("{}/{}/{}", self.artist_dir, self.album_dir, self.album_file_name),
            Layout::Flat => self.flat_file_name.clone(),
        }
    }

    /// The path below the music folder, with `/` as the separator: `Artist/Album/Title.ext`, or
    /// `Title.ext` in the flat layout.
    pub fn relative(&self) -> String {
        self.relative_in(self.layout)
    }

    pub fn album_folder(&self, music_dir: &Path) -> PathBuf {
        music_dir.join(&self.artist_dir).join(&self.album_dir)
    }

    pub fn path(&self, music_dir: &Path) -> PathBuf {
        music_dir.join(self.relative())
    }

    /// Where the other layout puts the track. A file found there is moved into place instead of
    /// downloaded again, so switching layouts costs no downloads.
    pub fn other_path(&self, music_dir: &Path) -> PathBuf {
        music_dir.join(self.relative_in(self.layout.other()))
    }

    /// `Artist — Title`, as the bash script printed tracks.
    pub fn label(&self) -> String {
        format!("{} — {}", self.artist, self.title)
    }
}

/// A track before collisions are resolved, with the keys that order it.
struct Draft {
    track: Track,
    stem: String,
    extension: String,
    server_path: String,
    index_number: Option<i64>,
}

impl Draft {
    fn new(item: Item, layout: Layout) -> Self {
        let size = item.size();
        let artist = item.album_artist.unwrap_or_else(|| UNKNOWN_ARTIST.to_string());
        let album = item.album.unwrap_or_else(|| UNKNOWN_ALBUM.to_string());
        let title = item.name.unwrap_or_else(|| UNKNOWN_TRACK.to_string());
        let stem = sanitize(&title, UNKNOWN_TRACK);
        Draft {
            track: Track {
                id: item.id,
                artist_dir: sanitize(&artist, UNKNOWN_ARTIST),
                album_dir: sanitize(&album, UNKNOWN_ALBUM),
                artist,
                album_has_image: item.album_id.is_some() && item.album_primary_image_tag.is_some(),
                has_image: item.image_tags.is_some_and(|tags| tags.primary.is_some()),
                album_id: item.album_id,
                run_time_ticks: item.run_time_ticks,
                size,
                layout,
                album_file_name: String::new(),
                flat_file_name: String::new(),
                title,
            },
            stem,
            extension: extension(item.path.as_deref()),
            server_path: item.path.unwrap_or_default(),
            index_number: item.index_number,
        }
    }

    /// `Title.ext`, before any numbering.
    fn natural_name(&self) -> String {
        format!("{}.{}", self.stem, self.extension)
    }

    fn numbered_name(&self, number: usize) -> String {
        format!("{} ({number}).{}", self.stem, self.extension)
    }

    /// The server path, track number and id: what orders tracks that share a local path, so the
    /// numbering is the same on every run.
    fn tie_break(&self) -> (&str, Option<i64>, &str) {
        (&self.server_path, self.index_number, &self.track.id)
    }
}

/// Gives each draft a file name that is unique within its slot (`slot` turns a draft and a file
/// name into what may not repeat). Drafts are visited in `order`: the first keeps `Title.ext`,
/// later ones get ` (2)`, ` (3)`, … before the extension, skipping names another draft already
/// has.
fn number_collisions<K: Eq + Hash>(drafts: &[Draft], order: &[usize], slot: impl Fn(&Draft, &str) -> K) -> Vec<String> {
    let mut names = vec![String::new(); drafts.len()];
    let mut taken = HashSet::new();
    let mut duplicates = Vec::new();
    for &index in order {
        let name = drafts[index].natural_name();
        if taken.insert(slot(&drafts[index], &name)) {
            names[index] = name;
        } else {
            duplicates.push(index);
        }
    }
    for index in duplicates {
        let draft = &drafts[index];
        let name = (2..)
            .map(|number| draft.numbered_name(number))
            .find(|candidate| !taken.contains(&slot(draft, candidate)))
            .unwrap_or_default();
        taken.insert(slot(draft, &name));
        names[index] = name;
    }
    names
}

/// Maps server tracks to local files in `layout`, sorted by local path. Each track gets its name
/// in both layouts, so a switch can find the files of the other one. Of the tracks that map to
/// the same file, the first (by server path, then track number, then id) keeps the name and the
/// others are numbered (see [`number_collisions`]): within the album folder in the album layout,
/// across the whole folder in the flat one.
pub fn plan(items: Vec<Item>, layout: Layout) -> Vec<Track> {
    let mut drafts: Vec<Draft> = items.into_iter().map(|item| Draft::new(item, layout)).collect();

    let mut by_album: Vec<usize> = (0..drafts.len()).collect();
    by_album.sort_by(|&a, &b| {
        let (a, b) = (&drafts[a], &drafts[b]);
        (&a.track.artist_dir, &a.track.album_dir, a.natural_name(), a.tie_break()).cmp(&(
            &b.track.artist_dir,
            &b.track.album_dir,
            b.natural_name(),
            b.tie_break(),
        ))
    });
    let album_names = number_collisions(&drafts, &by_album, |draft, name| {
        (
            draft.track.artist_dir.clone(),
            draft.track.album_dir.clone(),
            name.to_string(),
        )
    });

    let mut by_name: Vec<usize> = (0..drafts.len()).collect();
    by_name.sort_by(|&a, &b| {
        let (a, b) = (&drafts[a], &drafts[b]);
        (a.natural_name(), &a.track.artist_dir, &a.track.album_dir, a.tie_break()).cmp(&(
            b.natural_name(),
            &b.track.artist_dir,
            &b.track.album_dir,
            b.tie_break(),
        ))
    });
    let flat_names = number_collisions(&drafts, &by_name, |_, name| name.to_string());

    for ((draft, album_name), flat_name) in drafts.iter_mut().zip(album_names).zip(flat_names) {
        draft.track.album_file_name = album_name;
        draft.track.flat_file_name = flat_name;
    }
    let mut tracks: Vec<Track> = drafts.into_iter().map(|draft| draft.track).collect();
    match layout {
        Layout::Albums => tracks.sort_by(|a, b| {
            (&a.artist_dir, &a.album_dir, &a.album_file_name).cmp(&(&b.artist_dir, &b.album_dir, &b.album_file_name))
        }),
        Layout::Flat => tracks.sort_by(|a, b| a.flat_file_name.cmp(&b.flat_file_name)),
    }
    tracks
}

/// Gives each name in `names` (in priority order) a unique `<name>.<extension>` file name,
/// numbering later duplicates like [`plan`] does.
pub fn unique_file_names(names: &[String], extension: &str) -> Vec<String> {
    let mut taken: HashSet<String> = names.iter().map(|name| format!("{name}.{extension}")).collect();
    let mut seen = HashSet::new();
    names
        .iter()
        .map(|name| {
            let natural = format!("{name}.{extension}");
            if seen.insert(natural.clone()) {
                return natural;
            }
            let numbered = (2..)
                .map(|number| format!("{name} ({number}).{extension}"))
                .find(|candidate| !taken.contains(candidate))
                .unwrap_or_default();
            taken.insert(numbered.clone());
            numbered
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MediaSource;

    fn item(id: &str, artist: Option<&str>, album: Option<&str>, name: Option<&str>, path: &str) -> Item {
        Item {
            id: id.to_string(),
            name: name.map(str::to_string),
            album: album.map(str::to_string),
            album_artist: artist.map(str::to_string),
            path: Some(path.to_string()),
            media_sources: Some(vec![MediaSource {
                size: Some(1000),
                direct_stream_url: None,
            }]),
            ..Item::default()
        }
    }

    #[test]
    fn sanitizes_like_the_bash_script() {
        assert_eq!(sanitize("AC/DC", UNKNOWN_ARTIST), "ACDC");
        assert_eq!(sanitize("What? Why: *Now* \"<>|\\", UNKNOWN_TRACK), "What Why Now");
        assert_eq!(sanitize("  padded name  ", UNKNOWN_TRACK), "padded name");
        assert_eq!(sanitize("\ttab kept\t", UNKNOWN_TRACK), "\ttab kept\t");
        assert_eq!(
            sanitize("Sigur Rós — ( ) [ ] ' & ; $", UNKNOWN_ARTIST),
            "Sigur Rós — ( ) [ ] ' & ; $"
        );
        assert_eq!(sanitize(&"a".repeat(250), UNKNOWN_TRACK), "a".repeat(200));
    }

    #[test]
    fn empty_and_dot_parts_get_a_fallback() {
        assert_eq!(sanitize("", UNKNOWN_ARTIST), UNKNOWN_ARTIST);
        assert_eq!(sanitize(" ?/: ", UNKNOWN_ALBUM), UNKNOWN_ALBUM);
        assert_eq!(sanitize("..", UNKNOWN_ARTIST), UNKNOWN_ARTIST);
        assert_eq!(sanitize(" . ", UNKNOWN_ALBUM), UNKNOWN_ALBUM);
        assert_eq!(sanitize("...", UNKNOWN_TRACK), "...");
    }

    #[test]
    fn cuts_at_a_character_boundary() {
        // 199 ASCII bytes and a two-byte character: cutting at 200 bytes would split it.
        let name = format!("{}é", "a".repeat(199));
        assert_eq!(sanitize(&name, UNKNOWN_TRACK), "a".repeat(199));
        let emoji = "🎵".repeat(60);
        let cut = sanitize(&emoji, UNKNOWN_TRACK);
        assert_eq!(cut.len(), 200);
        assert_eq!(cut, "🎵".repeat(50));
    }

    #[test]
    fn takes_the_extension_of_the_server_file() {
        assert_eq!(extension(Some("/media/music/Song.flac")), "flac");
        assert_eq!(extension(Some("/media/music/Song.Name.Opus")), "Opus");
        assert_eq!(extension(Some("D:\\Music\\Song.m4a")), "m4a");
        assert_eq!(extension(Some("/media/my.band/track")), "mp3");
        assert_eq!(extension(Some("/media/music/Song.")), "mp3");
        assert_eq!(extension(Some("")), "mp3");
        assert_eq!(extension(None), "mp3");
    }

    #[test]
    fn builds_artist_album_title_paths() {
        let tracks = plan(
            vec![
                item(
                    "1",
                    Some("AC/DC"),
                    Some("Back in Black"),
                    Some("Hells Bells"),
                    "/m/1.flac",
                ),
                item("2", None, None, None, "/m/2"),
                item("3", Some(""), Some("  "), Some("???"), "/m/3.ogg"),
            ],
            Layout::Albums,
        );
        let paths: Vec<String> = tracks.iter().map(Track::relative).collect();
        assert_eq!(
            paths,
            [
                "ACDC/Back in Black/Hells Bells.flac",
                "Unknown Artist/Unknown Album/Unknown Track.mp3",
                "Unknown Artist/Unknown Album/Unknown Track.ogg",
            ]
        );
        assert_eq!(tracks[0].label(), "AC/DC — Hells Bells");
        assert_eq!(
            tracks[0].path(Path::new("/home/user/Music")),
            PathBuf::from("/home/user/Music/ACDC/Back in Black/Hells Bells.flac")
        );
        assert_eq!(tracks[1].label(), "Unknown Artist — Unknown Track");
    }

    #[test]
    fn numbers_collisions_in_a_stable_order() {
        let items = vec![
            item("c", Some("A"), Some("B"), Some("Song"), "/m/disc2/song.flac"),
            item("a", Some("A"), Some("B"), Some("Song"), "/m/disc1/song.flac"),
            item("b", Some("A"), Some("B"), Some("Song?"), "/m/disc1/song.flac"),
            item("d", Some("A"), Some("B"), Some("Song (2)"), "/m/other.flac"),
        ];
        let mut reversed = items.clone();
        reversed.reverse();
        for input in [items, reversed] {
            let tracks = plan(input, Layout::Albums);
            let names: Vec<(&str, &str)> = tracks
                .iter()
                .map(|t| (t.id.as_str(), t.album_file_name.as_str()))
                .collect();
            // "a" and "b" share the server path; the id decides. "d" keeps its own name
            // "Song (2)", so the third "Song" becomes "Song (3)".
            assert_eq!(
                names,
                [
                    ("d", "Song (2).flac"),
                    ("b", "Song (3).flac"),
                    ("c", "Song (4).flac"),
                    ("a", "Song.flac")
                ]
            );
            // One album, so the flat layout numbers the same way.
            assert!(tracks.iter().all(|t| t.flat_file_name == t.album_file_name));
        }
    }

    #[test]
    fn same_title_on_different_albums_is_no_collision() {
        let tracks = plan(
            vec![
                item("1", Some("A"), Some("One"), Some("Intro"), "/m/1.mp3"),
                item("2", Some("A"), Some("Two"), Some("Intro"), "/m/2.mp3"),
                item("3", Some("A"), Some("Two"), Some("Intro"), "/m/2.flac"),
            ],
            Layout::Albums,
        );
        let paths: Vec<String> = tracks.iter().map(Track::relative).collect();
        assert_eq!(paths, ["A/One/Intro.mp3", "A/Two/Intro.flac", "A/Two/Intro.mp3"]);
    }

    fn library() -> Vec<Item> {
        vec![
            item("1", Some("B"), Some("Two"), Some("Intro"), "/m/b.mp3"),
            item("2", Some("A"), Some("One"), Some("Intro"), "/m/a.mp3"),
            item("3", Some("A"), Some("One"), Some("Outro"), "/m/c.flac"),
            item("4", Some("C"), Some("Three"), Some("Intro (2)"), "/m/d.mp3"),
        ]
    }

    #[test]
    fn the_flat_layout_puts_tracks_in_the_music_folder_numbered_across_albums() {
        let tracks = plan(library(), Layout::Flat);
        let paths: Vec<(&str, String)> = tracks.iter().map(|t| (t.id.as_str(), t.relative())).collect();
        // Artist A sorts first, so its "Intro" keeps the name; "Intro (2)" is a real title.
        assert_eq!(
            paths,
            [
                ("4", "Intro (2).mp3".to_string()),
                ("1", "Intro (3).mp3".to_string()),
                ("2", "Intro.mp3".to_string()),
                ("3", "Outro.flac".to_string()),
            ]
        );
        let mut shuffled = library();
        shuffled.reverse();
        assert_eq!(plan(shuffled, Layout::Flat), tracks);
        assert_eq!(
            tracks[0].path(Path::new("/home/user/Music")),
            PathBuf::from("/home/user/Music/Intro (2).mp3")
        );
    }

    #[test]
    fn each_layout_knows_where_the_other_puts_a_track() {
        let music = Path::new("/home/user/Music");
        let flat = plan(library(), Layout::Flat);
        let albums = plan(library(), Layout::Albums);
        let by_id = |tracks: &[Track], id: &str| tracks.iter().find(|t| t.id == id).cloned().expect("track");
        for id in ["1", "2", "3", "4"] {
            let (flat, albums) = (by_id(&flat, id), by_id(&albums, id));
            assert_eq!(flat.other_path(music), albums.path(music), "{id}");
            assert_eq!(albums.other_path(music), flat.path(music), "{id}");
        }
        assert_eq!(by_id(&albums, "1").relative(), "B/Two/Intro.mp3");
        assert_eq!(by_id(&albums, "1").other_path(music), music.join("Intro (3).mp3"));
    }

    #[test]
    fn numbers_duplicate_playlist_names() {
        let names = ["Mix", "Mix", "Mix (2)", "Road"].map(str::to_string);
        assert_eq!(
            unique_file_names(&names, "m3u"),
            ["Mix.m3u", "Mix (3).m3u", "Mix (2).m3u", "Road.m3u"]
        );
    }
}
