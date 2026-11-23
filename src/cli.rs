//! The command line.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::{KEY_VARIABLE, URL_VARIABLE};

#[derive(Parser, Debug)]
#[command(
    name = "jellyfin-music-rs",
    version,
    about = "Mirror the music library of a Jellyfin server into a local folder"
)]
pub struct Cli {
    /// Config file with the server URL and API key
    /// [default: $XDG_CONFIG_HOME/jellyfin-music-rs/config]
    #[arg(long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Check an API key against the server and save it with the server URL
    Login {
        /// Server URL, e.g. https://jellyfin.example.com
        url: String,
        /// API key (Jellyfin: Dashboard, API Keys)
        key: String,
    },
    /// Remove the saved URL and API key
    Logout,
    /// Show the server, its version and the user whose library is synced
    ServerInfo(ServerArgs),
    /// Mirror the music library into the local folder
    Sync(SyncArgs),
}

#[derive(Args, Debug)]
pub struct ServerArgs {
    /// Server URL (overrides the config file)
    #[arg(long, value_name = "URL", env = URL_VARIABLE, hide_env_values = true)]
    pub url: Option<String>,

    /// API key (overrides the config file)
    #[arg(long, value_name = "KEY", env = KEY_VARIABLE, hide_env_values = true)]
    pub key: Option<String>,

    /// The user whose library is synced [default: the first user the server lists]
    #[arg(long, value_name = "NAME")]
    pub user: Option<String>,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    #[command(flatten)]
    pub server: ServerArgs,

    /// Music folder [default: XDG_MUSIC_DIR from user-dirs.dirs, else ~/Music]
    #[arg(long, value_name = "PATH", env = "JELLYFIN_MUSIC_DIR", hide_env_values = true)]
    pub dir: Option<PathBuf>,

    /// Number of downloads at the same time
    #[arg(long, value_name = "N", default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=32))]
    pub jobs: u16,

    /// Download every track again, and every cover
    #[arg(long, visible_alias = "force")]
    pub overwrite: bool,

    /// Show what would happen without changing anything
    #[arg(long)]
    pub dry_run: bool,

    /// Do not download album covers
    #[arg(long)]
    pub no_covers: bool,

    /// Do not write playlists
    #[arg(long)]
    pub no_playlists: bool,

    /// Refuse to delete more than this percentage of the local tracks (from 10 files on)
    #[arg(long, value_name = "PERCENT", default_value_t = 50, value_parser = clap::value_parser!(u8).range(0..=100))]
    pub max_delete: u8,

    /// Print one JSON summary on stdout; human messages go to stderr
    #[arg(long)]
    pub json: bool,

    /// Do not show progress bars
    #[arg(long)]
    pub no_progress: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_sync_options() {
        let cli = Cli::try_parse_from([
            "jellyfin-music-rs",
            "sync",
            "--dir",
            "/home/user/Music",
            "--jobs",
            "8",
            "--force",
            "--max-delete",
            "100",
            "--json",
            "--config",
            "/home/user/conf",
        ])
        .expect("valid");
        assert_eq!(cli.config, Some(PathBuf::from("/home/user/conf")));
        let Command::Sync(args) = cli.command else {
            panic!("not sync")
        };
        assert_eq!(
            (args.jobs, args.overwrite, args.max_delete, args.json),
            (8, true, 100, true)
        );
        assert!(!args.dry_run && !args.no_covers && !args.no_playlists);
    }

    #[test]
    fn rejects_out_of_range_values() {
        assert!(Cli::try_parse_from(["jellyfin-music-rs", "sync", "--jobs", "0"]).is_err());
        assert!(Cli::try_parse_from(["jellyfin-music-rs", "sync", "--max-delete", "101"]).is_err());
    }

    #[test]
    fn help_never_shows_the_key_from_the_environment() {
        let command = Cli::command();
        for name in ["sync", "server-info"] {
            let subcommand = command.find_subcommand(name).expect("subcommand");
            let key = subcommand
                .get_arguments()
                .find(|arg| arg.get_id() == "key")
                .expect("--key");
            assert!(key.is_hide_env_values_set(), "{name}");
        }
    }
}
