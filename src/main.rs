//! jellyfin-music-rs: mirrors the music library of a Jellyfin server into a local folder.

mod account;
mod api;
mod cli;
mod config;
mod covers;
mod decide;
mod dirs;
mod download;
mod fsutil;
mod layout;
mod lock;
mod output;
mod playlists;
mod pool;
mod prune;
mod shell;
mod signal;
mod status;
mod sync;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command, SyncArgs};
use crate::output::Reporter;
use crate::status::Failure;

fn exit(result: Result<(), Failure>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("{}", failure.message);
            ExitCode::from(failure.status.exit_code())
        }
    }
}

/// `--dir` (or `JELLYFIN_MUSIC_DIR`) made absolute, else the default music folder.
fn music_dir(dir: Option<PathBuf>) -> Option<PathBuf> {
    match dir {
        Some(dir) => std::path::absolute(&dir).ok(),
        None => dirs::default_music_dir(),
    }
}

fn sync(args: SyncArgs, config_path: Option<PathBuf>) -> ExitCode {
    let credentials = config::resolve(args.server.url, args.server.key, config_path.as_deref());
    let options = sync::Options {
        dir: music_dir(args.dir),
        user: args.server.user,
        jobs: usize::from(args.jobs),
        overwrite: args.overwrite,
        dry_run: args.dry_run,
        covers: !args.no_covers,
        playlists: !args.no_playlists,
        flat: args.flat,
        max_delete: args.max_delete,
    };
    signal::install();
    let mut reporter = Reporter::new(args.json, !args.no_progress);
    let summary = sync::run(credentials, &options, &mut reporter);
    if summary.tracks > 0 {
        reporter.line(&summary.line());
    }
    if let Some(message) = &summary.message {
        reporter.warn(message);
    }
    if args.json {
        println!("{}", summary.to_json());
    }
    ExitCode::from(summary.status.exit_code())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config_path = cli.config.or_else(config::default_path);
    match cli.command {
        Command::Login { url, key } => exit(account::login(&url, &key, config_path.as_deref())),
        Command::Logout => exit(account::logout(config_path.as_deref())),
        Command::ServerInfo(args) => {
            let result = config::resolve(args.url, args.key, config_path.as_deref())
                .map_err(Failure::error)
                .and_then(|credentials| account::server_info(credentials, args.user.as_deref()));
            exit(result)
        }
        Command::Sync(args) => sync(args, config_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Status;

    #[test]
    fn relative_music_folders_become_absolute() {
        let dir = music_dir(Some(PathBuf::from("Music"))).expect("absolute");
        assert!(dir.is_absolute() && dir.ends_with("Music"));
        assert_eq!(
            music_dir(Some(PathBuf::from("/home/user/Music"))),
            Some(PathBuf::from("/home/user/Music"))
        );
    }

    #[test]
    fn statuses_are_reported_as_exit_codes() {
        assert_eq!(exit(Ok(())), ExitCode::SUCCESS);
        assert_eq!(exit(Err(Failure::new(Status::NotConfigured, "x"))), ExitCode::from(3));
    }
}
