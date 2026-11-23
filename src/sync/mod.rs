//! The `sync` command: mirrors the server's music library into the music folder, in this order:
//! lock, credentials, reachability, library, moves from the flat layout, downloads, covers,
//! playlists, deletions. Every early stop leaves the music folder as it was.

mod covers_playlists;
mod downloads;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::account;
use crate::api::{self, ApiError, Client, Item};
use crate::config::Credentials;
use crate::layout::{self, Track};
use crate::lock::{self, Lock, LockError};
use crate::output::{Reporter, Summary};
use crate::prune::{self, Expected, Kind};
use crate::signal;
use crate::status::{Failure, Status};

const KEPT: &str = "partial downloads are kept for the next run and nothing was deleted";

pub struct Options {
    /// `None` when neither `--dir` nor a default could be found.
    pub dir: Option<PathBuf>,
    pub user: Option<String>,
    pub jobs: usize,
    pub overwrite: bool,
    pub dry_run: bool,
    pub covers: bool,
    pub playlists: bool,
    pub max_delete: u8,
}

/// What every step needs.
struct Context<'a> {
    client: Client,
    user_id: String,
    dir: PathBuf,
    options: &'a Options,
}

/// Runs a sync. `credentials` is the outcome of reading the flags, environment and config file.
pub fn run(credentials: Result<Option<Credentials>, String>, options: &Options, reporter: &mut Reporter) -> Summary {
    let mut summary = Summary::new(options.dry_run);
    if let Err(failure) = sync(credentials, options, reporter, &mut summary) {
        summary.status = failure.status;
        summary.message = Some(failure.message);
    }
    reporter.finish_progress();
    summary
}

fn sync(
    credentials: Result<Option<Credentials>, String>,
    options: &Options,
    reporter: &mut Reporter,
    summary: &mut Summary,
) -> Result<(), Failure> {
    let _lock = take_lock(options.dry_run)?;
    let credentials = credentials
        .map_err(Failure::error)?
        .ok_or_else(|| Failure::new(Status::NotConfigured, account::NOT_CONFIGURED))?;
    let dir = options
        .dir
        .clone()
        .ok_or_else(|| Failure::error("Cannot tell where the music folder is: pass --dir <path>"))?;
    let client = Client::new(&credentials);
    client.check_reachable().map_err(|error| {
        let failure = account::request_failure(&error, client.base_url());
        interrupted_or(failure)
    })?;
    let users = client
        .users()
        .map_err(|error| request_failure(error, "list the users"))?;
    let user = api::select_user(users, options.user.as_deref()).map_err(Failure::error)?;

    reporter.line(&format!("Fetching the music library of {} from Jellyfin...", user.name));
    let items = client
        .audio_items(&user.id)
        .map_err(|error| request_failure(error, "read the music library"))?;
    check_unique(&items)?;
    if items.is_empty() {
        return Err(Failure::new(
            Status::EmptyLibrary,
            "The server listed no tracks, so the music folder was left untouched",
        ));
    }
    stop_if_interrupted()?;
    let tracks = layout::plan(items);
    summary.tracks = tracks.len();
    reporter.line(&format!("Found {} tracks on the server", tracks.len()));

    let context = Context {
        client,
        user_id: user.id,
        dir,
        options,
    };
    if !options.dry_run {
        fs::create_dir_all(&context.dir)
            .map_err(|error| Failure::error(format!("Could not create {}: {error}", context.dir.display())))?;
    }
    let (jobs, mut synced, moved_away) = downloads::prepare(&context, &tracks, reporter, summary);
    downloads::download_all(&context, &tracks, &jobs, &mut synced, reporter, summary)?;
    if options.covers {
        covers_playlists::sync_covers(&context, &tracks, &synced, reporter, summary)?;
    }
    let playlist_files = if options.playlists {
        covers_playlists::sync_playlists(&context, &tracks, &synced, reporter, summary)?
    } else {
        None
    };
    remove_stale(&context, &tracks, playlist_files, moved_away, reporter, summary)
}

fn take_lock(dry_run: bool) -> Result<Option<Lock>, Failure> {
    // A dry run writes nothing, not even the lock file.
    if dry_run {
        return Ok(None);
    }
    let path = lock::default_path()
        .ok_or_else(|| Failure::error("Cannot tell where the lock file goes: set XDG_RUNTIME_DIR or HOME"))?;
    match lock::acquire(&path) {
        Ok(lock) => Ok(Some(lock)),
        Err(LockError::AlreadyRunning) => Err(Failure::new(
            Status::AlreadyRunning,
            "Another jellyfin-music-rs sync is already running",
        )),
        Err(LockError::Io(error)) => Err(Failure::error(format!(
            "Could not take the lock {}: {error}",
            path.display()
        ))),
    }
}

fn interrupted() -> Failure {
    Failure::new(Status::Interrupted, format!("Interrupted: {KEPT}"))
}

/// A signal also breaks off network reads; report the signal rather than the broken read.
fn interrupted_or(failure: Failure) -> Failure {
    if signal::interrupted() { interrupted() } else { failure }
}

fn stop_if_interrupted() -> Result<(), Failure> {
    if signal::interrupted() {
        Err(interrupted())
    } else {
        Ok(())
    }
}

/// A failed request after the reachability check: a broken connection is "connection lost",
/// anything else an error. Nothing has been changed at that point.
fn request_failure(error: ApiError, what: &str) -> Failure {
    interrupted_or(match error {
        ApiError::Transport(message) => Failure::new(
            Status::ConnectionLost,
            format!("Lost the connection while trying to {what} ({message}); nothing was changed"),
        ),
        other => Failure::error(format!("Could not {what}: {other}; nothing was changed")),
    })
}

fn connection_lost(message: &str, left: usize) -> Failure {
    interrupted_or(Failure::new(
        Status::ConnectionLost,
        format!("Lost the connection to the server ({message}) with {left} downloads not started: {KEPT}"),
    ))
}

/// Paging over a library that changes can list a track twice and skip another; a skipped track
/// would be deleted, so that stops the sync.
fn check_unique(items: &[Item]) -> Result<(), Failure> {
    let mut seen = HashSet::new();
    if items.iter().all(|item| seen.insert(item.id.as_str())) {
        Ok(())
    } else {
        Err(Failure::error(
            "The server listed a track twice (the library changed while it was read); nothing was changed",
        ))
    }
}

/// Deletes the managed files the server no longer has, unless that is too many, then the empty
/// folders. Reached only when the library was read completely and the connection held.
fn remove_stale(
    context: &Context,
    tracks: &[Track],
    playlist_files: Option<HashSet<String>>,
    moved_away: HashSet<PathBuf>,
    reporter: &Reporter,
    summary: &mut Summary,
) -> Result<(), Failure> {
    stop_if_interrupted()?;
    let dir = &context.dir;
    let expected = Expected {
        tracks: tracks.iter().map(|track| track.path(dir)).collect(),
        album_folders: tracks.iter().map(|track| track.album_folder(dir)).collect(),
        playlists: playlist_files,
        moved_away,
    };
    let plan = prune::plan(dir, &expected).map_err(|error| {
        Failure::error(format!(
            "Could not scan {}: {error}; nothing was deleted",
            dir.display()
        ))
    })?;
    let audio = plan.audio_removals();
    let max = context.options.max_delete;
    if !prune::guard_allows(audio, plan.local_audio, max) {
        return Err(Failure::new(
            Status::TooManyDeletions,
            format!(
                "Nothing was deleted: {audio} of the {} local tracks are no longer on the server, more than \
                 --max-delete {max}% allows. Check the server's library, or run again with --max-delete 100 \
                 to delete them",
                plan.local_audio
            ),
        ));
    }
    for (done, removal) in plan.removals.iter().enumerate() {
        if signal::interrupted() {
            return Err(Failure::new(
                Status::Interrupted,
                format!(
                    "Interrupted while deleting: {done} of {} files removed",
                    plan.removals.len()
                ),
            ));
        }
        let relative = removal.path.strip_prefix(dir).unwrap_or(&removal.path).display();
        if context.options.dry_run {
            reporter.line(&format!("Would remove (not on server): {relative}"));
        } else {
            reporter.line(&format!("Removing (not on server): {relative}"));
            if let Err(error) = fs::remove_file(&removal.path) {
                reporter.warn(&format!("Could not remove {relative}: {error}"));
                continue;
            }
        }
        if removal.kind == Kind::Audio {
            summary.removed += 1;
        }
    }
    if !context.options.dry_run {
        prune::remove_empty_folders(dir);
    }
    Ok(())
}
