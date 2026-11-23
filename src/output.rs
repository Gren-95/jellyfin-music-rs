//! What a sync shows: progress bars in a terminal, one plain line per action otherwise (for log
//! files), the final count line and the `--json` summary.
//!
//! With `--json`, stdout carries only the JSON object; every human message goes to stderr.

use std::io::{self, IsTerminal, Write};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde::Serialize;

use crate::status::Status;

/// A track that could not be synced, as listed in the JSON output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedTrack {
    /// `Artist/Album/Title.ext`.
    pub track: String,
    pub error: String,
}

/// The result of a sync. Also what `--json` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    #[serde(serialize_with = "serialize_status")]
    pub status: Status,
    pub dry_run: bool,
    /// Tracks on the server.
    pub tracks: usize,
    /// New tracks downloaded.
    pub downloaded: usize,
    /// Tracks downloaded again: changed size or `--overwrite`.
    pub updated: usize,
    pub up_to_date: usize,
    /// Tracks moved from the old flat layout that needed nothing else.
    pub moved: usize,
    /// Audio files deleted because the server no longer has them.
    pub removed: usize,
    /// Bytes of audio received (with `--dry-run`: the size of what would be downloaded).
    pub bytes: u64,
    /// `cover.jpg` files written.
    pub covers: usize,
    /// Playlist files written (new or changed).
    pub playlists: usize,
    pub failed: Vec<FailedTrack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn serialize_status<S: serde::Serializer>(status: &Status, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(status.name())
}

impl Summary {
    pub fn new(dry_run: bool) -> Self {
        Summary {
            status: Status::Ok,
            dry_run,
            tracks: 0,
            downloaded: 0,
            updated: 0,
            up_to_date: 0,
            moved: 0,
            removed: 0,
            bytes: 0,
            covers: 0,
            playlists: 0,
            failed: Vec::new(),
            message: None,
        }
    }

    /// The bash script's last line, with the new counts added. A sync that stopped early (the
    /// message says why) is not called complete.
    pub fn line(&self) -> String {
        let heading = match (self.dry_run, self.status) {
            (false, Status::Ok) => "Sync complete",
            (false, _) => "Sync stopped",
            (true, Status::Ok) => "Dry run complete, nothing was changed",
            (true, _) => "Dry run stopped, nothing was changed",
        };
        format!(
            "{heading}: {} downloaded, {} updated, {} up to date, {} moved, {} removed, {} failed",
            self.downloaded,
            self.updated,
            self.up_to_date,
            self.moved,
            self.removed,
            self.failed.len()
        )
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| String::from("{}"))
    }
}

/// The overall bars: tracks handled, and bytes received of the bytes to download.
struct Bars {
    multi: MultiProgress,
    tracks: ProgressBar,
    bytes: ProgressBar,
}

pub struct Reporter {
    /// `--json`: human lines go to stderr.
    json: bool,
    progress_allowed: bool,
    bars: Option<Bars>,
}

impl Reporter {
    /// Progress bars are shown when stderr is a terminal and `progress` is set.
    pub fn new(json: bool, progress: bool) -> Self {
        Reporter {
            json,
            progress_allowed: progress && io::stderr().is_terminal(),
            bars: None,
        }
    }

    /// An action line (`Downloaded: …`): above the bars when they are shown, else on stdout (on
    /// stderr with `--json`).
    pub fn line(&self, text: &str) {
        if let Some(bars) = &self.bars {
            let _ = bars.multi.println(text);
        } else if self.json {
            eprintln!("{text}");
        } else {
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout, "{text}");
        }
    }

    /// A warning or error, always on stderr.
    pub fn warn(&self, text: &str) {
        if let Some(bars) = &self.bars {
            bars.multi.suspend(|| eprintln!("{text}"));
        } else {
            eprintln!("{text}");
        }
    }

    /// Shows the bars, if allowed: `done` of `tracks` are already handled, `bytes` are to come.
    pub fn start_progress(&mut self, tracks: usize, done: usize, bytes: u64) {
        if !self.progress_allowed {
            return;
        }
        let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());
        let tracks_bar = multi.add(
            ProgressBar::new(tracks as u64).with_style(style("{prefix:>6} [{bar:40.cyan/blue}] {pos}/{len} tracks")),
        );
        tracks_bar.set_prefix("Tracks");
        tracks_bar.set_position(done as u64);
        let bytes_bar = multi.add(ProgressBar::new(bytes).with_style(style(
            "{prefix:>6} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta}",
        )));
        bytes_bar.set_prefix("Data");
        self.bars = Some(Bars {
            multi,
            tracks: tracks_bar,
            bytes: bytes_bar,
        });
    }

    pub fn track_done(&self) {
        if let Some(bars) = &self.bars {
            bars.tracks.inc(1);
        }
    }

    /// The bytes bar, for the download threads to advance.
    pub fn bytes_bar(&self) -> Option<ProgressBar> {
        self.bars.as_ref().map(|bars| bars.bytes.clone())
    }

    pub fn finish_progress(&mut self) {
        if let Some(bars) = self.bars.take() {
            bars.tracks.finish_and_clear();
            bars.bytes.finish_and_clear();
            let _ = bars.multi.clear();
        }
    }
}

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template)
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=> ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_line_matches_the_bash_script_with_new_counts() {
        let mut summary = Summary::new(false);
        summary.downloaded = 3;
        summary.up_to_date = 10;
        summary.removed = 1;
        assert_eq!(
            summary.line(),
            "Sync complete: 3 downloaded, 0 updated, 10 up to date, 0 moved, 1 removed, 0 failed"
        );
        summary.dry_run = true;
        assert!(
            summary
                .line()
                .starts_with("Dry run complete, nothing was changed: 3 downloaded")
        );
    }

    #[test]
    fn json_has_the_documented_shape() {
        let mut summary = Summary::new(false);
        summary.tracks = 5;
        summary.downloaded = 2;
        summary.bytes = 2048;
        summary.covers = 1;
        summary.failed.push(FailedTrack {
            track: "Artist/Album/Song.flac".to_string(),
            error: "HTTP 404".to_string(),
        });
        let value: serde_json::Value = serde_json::from_str(&summary.to_json()).expect("valid JSON");
        assert_eq!(
            value,
            serde_json::json!({
                "status": "ok", "dry_run": false, "tracks": 5, "downloaded": 2, "updated": 0,
                "up_to_date": 0, "moved": 0, "removed": 0, "bytes": 2048, "covers": 1, "playlists": 0,
                "failed": [{"track": "Artist/Album/Song.flac", "error": "HTTP 404"}]
            })
        );
    }

    #[test]
    fn json_carries_a_message_for_other_statuses() {
        let mut summary = Summary::new(true);
        summary.status = Status::Unreachable;
        summary.message = Some("Server unreachable".to_string());
        let value: serde_json::Value = serde_json::from_str(&summary.to_json()).expect("valid JSON");
        assert_eq!(value["status"], "unreachable");
        assert_eq!(value["message"], "Server unreachable");
        assert_eq!(value["dry_run"], true);
    }
}
