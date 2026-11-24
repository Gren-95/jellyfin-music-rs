//! `cover.jpg` in each album folder. The first choice is the primary image of the album entity
//! (`AlbumId`): Jellyfin only sets `AlbumId` when it grouped the tracks into an album, and
//! `AlbumPrimaryImageTag` only when that album has an image. Without one, the folder gets the
//! primary image of its first track (in layout order) that has one (`ImageTags.Primary`), which is
//! usually the cover art embedded in the file. Only images the server lists are asked for.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::api::{self, ApiError, Client};
use crate::fsutil;
use crate::layout::Track;
use crate::prune::COVER_NAME;

/// Images are read into memory up to this size.
const IMAGE_LIMIT: u64 = 64 << 20;

/// Whose primary image becomes `cover.jpg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The album entity, by id.
    Album(String),
    /// A track of the folder, by id: the fallback when the album has no image.
    Track(String),
}

impl Source {
    fn item_id(&self) -> &str {
        match self {
            Source::Album(id) | Source::Track(id) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverJob {
    /// Where `cover.jpg` goes.
    pub path: PathBuf,
    /// `Artist/Album`, for messages.
    pub folder: String,
    pub source: Source,
}

/// What one album folder offers.
#[derive(Default)]
struct Folder<'a> {
    any_synced: bool,
    album_id: Option<&'a str>,
    track_id: Option<&'a str>,
}

/// The covers to fetch: one per album folder that has a synced track and an image (the album's,
/// else its first track's that has one), and no `cover.jpg` yet (or `--overwrite`). `tracks` are
/// in layout order, as [`crate::layout::plan`] returns them; `synced[i]` tells whether
/// `tracks[i]` is synced.
pub fn plan(tracks: &[Track], synced: &[bool], music_dir: &Path, overwrite: bool) -> Vec<CoverJob> {
    let mut folders: BTreeMap<(&str, &str), Folder> = BTreeMap::new();
    for (track, &is_synced) in tracks.iter().zip(synced) {
        let folder = folders.entry((&track.artist_dir, &track.album_dir)).or_default();
        folder.any_synced |= is_synced;
        if folder.album_id.is_none() && track.album_has_image {
            folder.album_id = track.album_id.as_deref();
        }
        if folder.track_id.is_none() && track.has_image {
            folder.track_id = Some(&track.id);
        }
    }
    folders
        .into_iter()
        .filter(|(_, folder)| folder.any_synced)
        .filter_map(|((artist, album), folder)| {
            let source = folder
                .album_id
                .map(|id| Source::Album(id.to_string()))
                .or_else(|| folder.track_id.map(|id| Source::Track(id.to_string())))?;
            let path = music_dir.join(artist).join(album).join(COVER_NAME);
            (overwrite || !path.exists()).then(|| CoverJob {
                path,
                folder: format!("{artist}/{album}"),
                source,
            })
        })
        .collect()
}

/// Downloads one image to `job.path`. `Ok(false)`: the server has no such image (404).
pub fn fetch(client: &Client, job: &CoverJob) -> Result<bool, ApiError> {
    let mut response = client.get(&Client::primary_image_path(job.source.item_id()), None)?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(false),
        code => return Err(ApiError::Status(code)),
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(IMAGE_LIMIT)
        .read_to_vec()
        .map_err(api::classify)?;
    if let Some(folder) = job.path.parent() {
        fs::create_dir_all(folder).map_err(|error| ApiError::Other(error.to_string()))?;
    }
    fsutil::write_atomic(&job.path, &bytes, 0o644).map_err(|error| ApiError::Other(error.to_string()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::testdir::TestDir;
    use crate::layout::Layout;

    /// A track; `album` is `(album id, album has an image)`, `image` whether the track has one.
    fn track(folder: (&str, &str), file: &str, album: Option<(&str, bool)>, image: bool) -> Track {
        Track {
            id: format!("{}/{}/{file}", folder.0, folder.1),
            artist: folder.0.to_string(),
            title: file.to_string(),
            album_id: album.map(|(id, _)| id.to_string()),
            album_has_image: album.is_some_and(|(_, has_image)| has_image),
            has_image: image,
            run_time_ticks: None,
            size: None,
            artist_dir: folder.0.to_string(),
            album_dir: folder.1.to_string(),
            layout: Layout::Albums,
            album_file_name: file.to_string(),
            flat_file_name: file.to_string(),
        }
    }

    fn sources(jobs: &[CoverJob]) -> Vec<(&str, &Source)> {
        jobs.iter().map(|job| (job.folder.as_str(), &job.source)).collect()
    }

    #[test]
    fn one_cover_per_synced_album_folder_with_an_image() {
        let dir = TestDir::new("covers");
        let root = dir.path();
        dir.write("A/Has cover/cover.jpg", b"jpg");
        let tracks = [
            track(("A", "One"), "1.flac", Some(("album-1", true)), false),
            track(("A", "One"), "2.flac", Some(("album-1", true)), false),
            track(("A", "No image"), "1.flac", Some(("album-2", false)), false),
            track(("A", "No album"), "1.flac", None, false),
            track(("A", "Failed"), "1.flac", Some(("album-3", true)), false),
            track(("A", "Has cover"), "1.flac", Some(("album-4", true)), false),
            track(("B", "Mixed"), "1.flac", None, false),
            track(("B", "Mixed"), "2.flac", Some(("album-5", true)), false),
        ];
        let synced = [true, false, true, true, false, true, true, false];
        let jobs = plan(&tracks, &synced, root, false);
        let album = |id: &str| Source::Album(id.to_string());
        assert_eq!(
            sources(&jobs),
            [("A/One", &album("album-1")), ("B/Mixed", &album("album-5"))]
        );
        assert_eq!(jobs[0].path, root.join("A/One/cover.jpg"));
        let overwrite = plan(&tracks, &synced, root, true);
        assert_eq!(overwrite.len(), 3, "an existing cover.jpg is replaced with --overwrite");
    }

    #[test]
    fn falls_back_to_the_first_track_image_in_layout_order() {
        let dir = TestDir::new("covers-fallback");
        let tracks = [
            // The album image wins over the tracks' own images.
            track(("A", "Album image"), "1.mp3", Some(("album-1", true)), true),
            track(("A", "Album image"), "2.mp3", Some(("album-1", true)), true),
            // No album image: the first track (in layout order) that has an image.
            track(("A", "Album without image"), "1.mp3", Some(("album-2", false)), false),
            track(("A", "Album without image"), "2.mp3", Some(("album-2", false)), true),
            track(("A", "Album without image"), "3.mp3", Some(("album-2", false)), true),
            // No album entity at all, as on a library Jellyfin did not group into albums.
            track(("A", "No album"), "1.mp3", None, true),
            // No image anywhere: no cover.
            track(("A", "Nothing"), "1.mp3", None, false),
            // A track image from a track that failed still serves its synced neighbours.
            track(("B", "Partly failed"), "1.mp3", None, true),
            track(("B", "Partly failed"), "2.mp3", None, false),
        ];
        let synced = [true, true, true, true, true, true, true, false, true];
        let jobs = plan(&tracks, &synced, dir.path(), false);
        let track_source = |id: &str| Source::Track(id.to_string());
        assert_eq!(
            sources(&jobs),
            [
                ("A/Album image", &Source::Album("album-1".to_string())),
                ("A/Album without image", &track_source("A/Album without image/2.mp3")),
                ("A/No album", &track_source("A/No album/1.mp3")),
                ("B/Partly failed", &track_source("B/Partly failed/1.mp3")),
            ]
        );
        let all_failed = [true, true, true, true, true, true, true, false, false];
        let jobs = plan(&tracks, &all_failed, dir.path(), false);
        assert!(
            jobs.iter().all(|job| job.folder != "B/Partly failed"),
            "no synced track, no cover"
        );
    }
}
