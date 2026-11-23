//! The files beside the tracks: `cover.jpg` per album folder and the playlist files.

use std::collections::{HashMap, HashSet};

use super::{Context, connection_lost, stop_if_interrupted};
use crate::api::ApiError;
use crate::covers;
use crate::layout::Track;
use crate::output::{Reporter, Summary};
use crate::playlists;
use crate::status::Failure;

pub(super) fn sync_covers(
    context: &Context,
    tracks: &[Track],
    synced: &[bool],
    reporter: &Reporter,
    summary: &mut Summary,
) -> Result<(), Failure> {
    let options = context.options;
    for job in covers::plan(tracks, synced, &context.dir, options.overwrite) {
        stop_if_interrupted()?;
        if options.dry_run {
            reporter.line(&format!("Would download cover: {}", job.folder));
            summary.covers += 1;
            continue;
        }
        match covers::fetch(&context.client, &job) {
            Ok(true) => {
                reporter.line(&format!("Cover: {}", job.folder));
                summary.covers += 1;
            }
            Ok(false) => {}
            Err(ApiError::Transport(reason)) => return Err(connection_lost(&reason, 0)),
            Err(error) => reporter.warn(&format!("Cover failed [{error}]: {}", job.folder)),
        }
    }
    Ok(())
}

/// Writes the playlist files that changed. Returns the file names of all the server's
/// playlists, or `None` when they could not be listed (then no playlist file is deleted).
pub(super) fn sync_playlists(
    context: &Context,
    tracks: &[Track],
    synced: &[bool],
    reporter: &Reporter,
    summary: &mut Summary,
) -> Result<Option<HashSet<String>>, Failure> {
    let listed = match context.client.playlists(&context.user_id) {
        Ok(listed) => listed,
        Err(ApiError::Transport(reason)) => return Err(connection_lost(&reason, 0)),
        Err(error) => {
            reporter.warn(&format!(
                "Could not list the playlists ({error}); the playlist files are left as they are"
            ));
            return Ok(None);
        }
    };
    let by_id: HashMap<&str, &Track> = tracks
        .iter()
        .zip(synced)
        .filter(|(_, synced)| **synced)
        .map(|(track, _)| (track.id.as_str(), track))
        .collect();
    let folder = context.dir.join(playlists::FOLDER);
    let mut files = HashSet::new();
    for (playlist, file_name) in playlists::name_files(listed) {
        files.insert(file_name.clone());
        stop_if_interrupted()?;
        let entries = match context.client.playlist_entries(&playlist.id, &context.user_id) {
            Ok(entries) => entries,
            Err(ApiError::Transport(reason)) => return Err(connection_lost(&reason, 0)),
            Err(error) => {
                reporter.warn(&format!("Playlist failed [{error}]: {file_name}"));
                continue;
            }
        };
        let members: Vec<&Track> = entries
            .iter()
            .filter_map(|entry| by_id.get(entry.id.as_str()).copied())
            .collect();
        let content = playlists::render(&members);
        let path = folder.join(&file_name);
        if !playlists::needs_writing(&path, &content) {
            continue;
        }
        if context.options.dry_run {
            reporter.line(&format!("Would write playlist: {file_name}"));
        } else if let Err(error) = playlists::write(&path, &content) {
            reporter.warn(&format!("Could not write {}: {error}", path.display()));
            continue;
        } else {
            reporter.line(&format!("Playlist: {file_name} ({} tracks)", members.len()));
        }
        summary.playlists += 1;
    }
    Ok(Some(files))
}
