//! Getting the tracks in place: moves from the other layout, then the downloads, run in
//! parallel.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{Context, connection_lost, stop_if_interrupted};
use crate::decide::{self, Action};
use crate::download::{self, DownloadError};
use crate::fsutil;
use crate::layout::Track;
use crate::output::{FailedTrack, Reporter, Summary};
use crate::pool;
use crate::signal;
use crate::status::Failure;

/// A download to run.
pub(super) struct Job {
    /// Index into the tracks.
    track: usize,
    action: Action,
    dest: PathBuf,
}

/// Moves files from the other layout into place (the album layout, or the flat one, which the
/// bash script also used) and decides what to download. Returns the downloads, whether each track
/// has a local file, and (in a dry run) the files that would have been moved.
pub(super) fn prepare(
    context: &Context,
    tracks: &[Track],
    reporter: &Reporter,
    summary: &mut Summary,
) -> (Vec<Job>, Vec<bool>, HashSet<PathBuf>) {
    let options = context.options;
    let mut jobs = Vec::new();
    let mut synced = vec![false; tracks.len()];
    let mut claimed = HashSet::new();
    for (index, track) in tracks.iter().enumerate() {
        let dest = track.path(&context.dir);
        let mut local = fsutil::file_size(&dest);
        let mut moved = false;
        if local.is_none() {
            let other = track.other_path(&context.dir);
            if let Some(size) = fsutil::file_size(&other)
                && claimed.insert(other.clone())
            {
                match move_into_place(&other, &dest, options.dry_run) {
                    Ok(()) => {
                        let verb = if options.dry_run { "Would move" } else { "Moved" };
                        reporter.line(&format!("{verb}: {}", track.label()));
                        local = Some(size);
                        moved = true;
                    }
                    Err(error) => reporter.warn(&format!("Could not move {}: {error}", other.display())),
                }
            }
        }
        match decide::decide(local, track.size, options.overwrite) {
            Action::UpToDate if moved => summary.moved += 1,
            Action::UpToDate => summary.up_to_date += 1,
            action => {
                jobs.push(Job {
                    track: index,
                    action,
                    dest,
                });
                synced[index] = local.is_some();
                continue;
            }
        }
        synced[index] = true;
    }
    let moved_away = if options.dry_run { claimed } else { HashSet::new() };
    (jobs, synced, moved_away)
}

fn move_into_place(from: &Path, dest: &Path, dry_run: bool) -> std::io::Result<()> {
    if dry_run {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, dest)
}

pub(super) fn download_all(
    context: &Context,
    tracks: &[Track],
    jobs: &[Job],
    synced: &mut [bool],
    reporter: &mut Reporter,
    summary: &mut Summary,
) -> Result<(), Failure> {
    let total_bytes: u64 = jobs.iter().filter_map(|job| tracks[job.track].size).sum();
    if context.options.dry_run {
        for job in jobs {
            let (verb, count) = match job.action {
                Action::Update => ("Would update", &mut summary.updated),
                _ => ("Would download", &mut summary.downloaded),
            };
            *count += 1;
            synced[job.track] = true;
            reporter.line(&format!("{verb}: {}", tracks[job.track].label()));
        }
        summary.bytes = total_bytes;
        return Ok(());
    }

    reporter.start_progress(tracks.len(), tracks.len() - jobs.len(), total_bytes);
    let bytes_bar = reporter.bytes_bar();
    let received = AtomicU64::new(0);
    let lost = AtomicBool::new(false);
    let on_bytes = |count: u64| {
        received.fetch_add(count, Ordering::Relaxed);
        if let Some(bar) = &bytes_bar {
            bar.inc(count);
        }
    };
    let work = |job: &Job| {
        let track = &tracks[job.track];
        let result = download::download(
            &context.client,
            &context.user_id,
            &track.id,
            &job.dest,
            track.size,
            &on_bytes,
        );
        if matches!(result, Err(DownloadError::ConnectionLost(_))) {
            lost.store(true, Ordering::SeqCst);
        }
        result
    };
    let stop = || lost.load(Ordering::SeqCst) || signal::interrupted();

    let mut finished = 0;
    let mut lost_reason = None;
    pool::run(jobs, context.options.jobs, &stop, &work, |index, result| {
        finished += 1;
        let job = &jobs[index];
        let track = &tracks[job.track];
        reporter.track_done();
        match result {
            Ok(()) => {
                let (verb, count) = match job.action {
                    Action::Update => ("Updated", &mut summary.updated),
                    _ => ("Downloaded", &mut summary.downloaded),
                };
                *count += 1;
                synced[job.track] = true;
                reporter.line(&format!("{verb}: {}", track.label()));
            }
            Err(error) => {
                reporter.line(&format!("Failed [{}]: {}", error.label(), track.title));
                if let DownloadError::ConnectionLost(reason) = &error {
                    lost_reason.get_or_insert_with(|| reason.clone());
                }
                summary.failed.push(FailedTrack {
                    track: track.relative(),
                    error: error.detail(),
                });
            }
        }
    });
    summary.failed.sort_by(|a, b| a.track.cmp(&b.track));
    summary.bytes = received.load(Ordering::SeqCst);
    reporter.finish_progress();

    let left = jobs.len() - finished;
    stop_if_interrupted()?;
    match lost_reason {
        Some(reason) => Err(connection_lost(&reason, left)),
        None => Ok(()),
    }
}
