# jellyfin-music-rs

[![CI](https://github.com/Gren-95/jellyfin-music-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Gren-95/jellyfin-music-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Gren-95/jellyfin-music-rs)](https://github.com/Gren-95/jellyfin-music-rs/releases/latest)
[![License: GPL-3.0](https://img.shields.io/github/license/Gren-95/jellyfin-music-rs)](LICENSE)

Mirror the music library of your [Jellyfin](https://jellyfin.org) server into a local folder, as
`Artist/Album/Title.ext`, from the command line or a timer:

- **Jellyfin is the master.** New tracks are downloaded, changed ones downloaded again, and tracks
  the server no longer has are deleted locally, behind a guard against mass deletions.
- **Safe on a flaky connection.** Downloads go to a `.part` file that only replaces the real file
  once complete, are resumed on the next run, and a stalled transfer fails after 30 seconds
  instead of hanging. A server that is down or answers with an empty library changes nothing.
- **Covers and playlists.** `cover.jpg` in each album folder, and the server's playlists as M3U
  files that work on any player the folder is copied to.
- **Made for scripts.** One static binary, parallel downloads, a JSON summary and distinct exit
  codes, so a wrapper can tell "away from home" from a real problem.

It replaces a bash script that did the same job (with `curl` and `jq`) and keeps its folder
layout and file names, so an existing music folder carries over without downloading anything
again.

This is not an official Jellyfin project.

## Install

### Download a release

Static builds for Linux on x86_64 and aarch64 (any distribution):

```bash
arch=$(uname -m)
base=https://github.com/Gren-95/jellyfin-music-rs/releases/latest/download
curl -fsSLO "$base/jellyfin-music-rs-$arch-linux.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "jellyfin-music-rs-$arch-linux.tar.gz" jellyfin-music-rs
install -Dm755 jellyfin-music-rs ~/.local/bin/jellyfin-music-rs
```

### Build from source

With Rust 1.89 or newer:

```bash
cargo install --locked --git https://github.com/Gren-95/jellyfin-music-rs
```

This installs `jellyfin-music-rs` into `~/.cargo/bin`. Only Linux is supported.

## Quick start

Create an API key in Jellyfin under Dashboard, API Keys. Then:

```bash
jellyfin-music-rs login https://jellyfin.example.com YOUR_API_KEY
jellyfin-music-rs sync --dry-run    # see what would happen
jellyfin-music-rs sync
```

The music goes to your music folder (`XDG_MUSIC_DIR` from `~/.config/user-dirs.dirs`, usually
`~/Music`), or wherever `--dir` points.

## Usage

```
Usage: jellyfin-music-rs [OPTIONS] <COMMAND>

Commands:
  login        Check an API key against the server and save it with the server URL
  logout       Remove the saved URL and API key
  server-info  Show the server, its version and the user whose library is synced
  sync         Mirror the music library into the local folder
  help         Print this message or the help of the given subcommand(s)

Options:
      --config <FILE>  Config file with the server URL and API key [default: $XDG_CONFIG_HOME/jellyfin-music-rs/config]
  -h, --help           Print help
  -V, --version        Print version
```

Nothing ever asks a question: anything missing is an error with an exit code (see below).

### Log in

```bash
jellyfin-music-rs login https://jellyfin.example.com YOUR_API_KEY
jellyfin-music-rs server-info
jellyfin-music-rs logout
```

`login` checks the key with an authenticated request (`/System/Info`), prints the server's name
and version, and only then saves the URL and key. A wrong key or an unreachable server leaves
the config file as it was.

The server URL and API key are taken from, highest priority first:

1. `--url` and `--key`
2. the environment variables `JELLYFIN_URL` and `JELLYFIN_API_KEY`
3. the config file: `--config <file>`, by default `$XDG_CONFIG_HOME/jellyfin-music-rs/config`
   (`~/.config/jellyfin-music-rs/config`)

The config file has one `NAME=VALUE` line per setting, in shell syntax, and is written with mode
0600 in a folder with mode 0700:

```bash
# Written by jellyfin-music-rs login.
JELLYFIN_URL=https://jellyfin.example.com
JELLYFIN_API_KEY=0123456789abcdef0123456789abcdef
```

Values may be quoted the way bash's `printf '%q'` writes them (backslash escapes, `'single'`,
`"double"` and `$'…'` quotes), so the old script's `~/.config/jellyfin/sync.conf` can be used
directly with `--config ~/.config/jellyfin/sync.conf`. Nothing in the file is expanded: a
`$VARIABLE` or `$(command)` is an error. Blank lines and `#` comments are ignored, and a trailing
slash on the URL is dropped. The key is never printed, logged or put in the JSON output.

API keys are not tied to a user, so the library synced is the first user's the server lists, or
the one named with `--user <name>` (on `server-info` and `sync`).

### Sync

```
Usage: jellyfin-music-rs sync [OPTIONS]

Options:
      --config <FILE>         Config file with the server URL and API key [default: $XDG_CONFIG_HOME/jellyfin-music-rs/config]
      --url <URL>             Server URL (overrides the config file) [env: JELLYFIN_URL]
      --key <KEY>             API key (overrides the config file) [env: JELLYFIN_API_KEY]
      --user <NAME>           The user whose library is synced [default: the first user the server lists]
      --dir <PATH>            Music folder [default: XDG_MUSIC_DIR from user-dirs.dirs, else ~/Music] [env: JELLYFIN_MUSIC_DIR]
      --jobs <N>              Number of downloads at the same time [default: 4]
      --overwrite             Download every track again, and every cover [alias: --force]
      --dry-run               Show what would happen without changing anything
      --no-covers             Do not download album covers
      --no-playlists          Do not write playlists
      --max-delete <PERCENT>  Refuse to delete more than this percentage of the local tracks (from 10 files on) [default: 50]
      --json                  Print one JSON summary on stdout; human messages go to stderr
      --no-progress           Do not show progress bars
  -h, --help                  Print help
```

Examples:

```bash
# Into another folder, eight downloads at a time
jellyfin-music-rs sync --dir /mnt/usb/Music --jobs 8

# Download everything again after fixing files on the server
jellyfin-music-rs sync --overwrite

# From a timer: a JSON summary for the wrapper, the plain log lines on stderr
jellyfin-music-rs sync --json 2>>~/.cache/jellyfin-sync.log
```

A sync runs these steps, in order:

1. **One at a time.** A lock in `$XDG_RUNTIME_DIR` (else the cache folder) makes a second sync
   exit right away with "already running". A dry run takes no lock, as it writes nothing.
2. **Reachable?** `/System/Info/Public` is asked first. When the server cannot be reached
   (connection refused, DNS failure, timeout), nothing is touched.
3. **Library.** All audio tracks are listed, 500 per request. If any request fails, or the
   server lists no tracks at all (as a server whose media drive is not mounted does), nothing is
   touched.
4. **Old flat layout.** A track whose file is missing, but which exists as `<music>/<Title>.<ext>`
   from the bash script's first layout, is moved into place instead of downloaded.
5. **Downloads.** Missing tracks are downloaded, and present ones again when their size differs
   from the server's or with `--overwrite`. Up to `--jobs` downloads run at the same time.
6. **Covers** and **playlists** (below).
7. **Deletions** (below), then empty folders below the music folder are removed.

Downloads first go to `<file>.part`, which replaces the file only once it is complete and has the
size the server lists, so a failed download never replaces a good copy. A `.part` file left by an
earlier run is continued with an HTTP `Range` request. If the stream answers 404, the track is
fetched from the `DirectStreamUrl` the server gives in its playback info. Any other error status
fails that one track and the sync goes on; failed tracks are listed at the end and do not change
the exit code.

A connection opens within 10 seconds or fails; a transfer that receives nothing for 30 seconds
fails. A broken connection (as opposed to an error status) stops the sync: no new downloads start,
the running ones finish or fail, `.part` files are kept for the next run, and nothing is deleted.
Ctrl-C (or SIGTERM) does the same: running downloads stop at once, and a second Ctrl-C quits
immediately.

In a terminal, progress bars show the tracks and bytes. Otherwise, for log files, each action is
one line, and the run ends with a summary:

```
Fetching the music library of alice from Jellyfin...
Found 1234 tracks on the server
Downloaded: Artist — Title
Updated: Artist — Title
Moved: Artist — Title
Failed [404]: Title
Removing (not on server): Artist/Album/Title.flac
Sync complete: 1 downloaded, 1 updated, 1229 up to date, 1 moved, 1 removed, 1 failed
```

`--dry-run` changes nothing at all (no folders, moves, downloads, covers, playlists or deletions)
and reports the same counts, with lines such as `Would download: Artist — Title`. It cannot know
in advance that a download would fail, so such a track counts as downloaded.

## Folder layout

```
Music/
├── Artist/
│   └── Album/
│       ├── Title.flac
│       ├── Title (2).flac
│       └── cover.jpg
├── Unknown Artist/
│   └── Unknown Album/
│       └── Title.mp3
└── Playlists/
    └── Road Trip.m3u
```

- **Paths** are `<Artist>/<Album>/<Title>.<ext>`: the track's album artist (`Unknown Artist` when
  there is none), its album (`Unknown Album`), its name, and the extension of the file on the
  server (`mp3` when it has none).
- **Names** lose the characters `/ : * ? " < > | \`, spaces at both ends, and are cut to 200
  bytes (never in the middle of a UTF-8 character). A name that ends up empty, `.` or `..` becomes
  `Unknown Artist`, `Unknown Album` or `Unknown Track`.
- **Same path, different tracks.** When two tracks map to the same file (same artist, album and
  name), they are sorted by their path on the server, then track number, then id; the first keeps
  the name and the others get ` (2)`, ` (3)`, … before the extension. The order is the same on
  every run, so every track keeps its own file.
- **Covers** (`--no-covers` turns them off): `cover.jpg` in each album folder with at least one
  synced track. It is the primary image of the folder's album in Jellyfin when there is one (the
  album is known to Jellyfin as an album, `AlbumId`, and has an image, `AlbumPrimaryImageTag`).
  Otherwise it is the primary image of the first track in the folder (in file name order) that has
  one (`ImageTags.Primary`, usually the cover art embedded in the file), so libraries whose tracks
  are not grouped into albums get covers too. Only images the server lists are asked for, and an
  image the server then cannot find (404) is skipped. An existing `cover.jpg` is kept unless
  `--overwrite`.
- **Playlists** (`--no-playlists` turns them off): each of the user's playlists becomes
  `Playlists/<name>.m3u` in extended M3U, with the tracks that are synced, paths relative to the
  Playlists folder, and lengths in seconds:

  ```
  #EXTM3U
  #EXTINF:312,Artist - Title
  ../Artist/Album/Title.flac
  ```

  A file is only rewritten when its content changed.

## Deletion

Jellyfin is the master: what the server no longer has is deleted locally. Only files the sync
manages are ever deleted:

- audio files (`mp3 flac ogg opus m4a wav aac`, in any case) that no server track maps to
- `cover.jpg` in an `Artist/Album` folder that no server track maps to any more
- `.m3u` files directly in `Playlists/` whose playlist is gone from the server (left alone with
  `--no-playlists`, or when the playlists could not be listed)
- `.part` files of audio files that no server track maps to

Any other file (your own notes, other images, other playlists) is never touched, and symlinks are
neither followed nor deleted. After deleting, empty folders below the music folder are removed;
the music folder itself always stays.

Deletion only happens after a complete run: every library page was read, the connection held, and
no Ctrl-C.

**The guard.** If the deletions would remove more than `--max-delete` percent (default 50) of the
local audio files, and at least 10 files, nothing is deleted at all and the sync exits with code
8. This protects against a server that suddenly lists far fewer tracks, for example because a
library was removed or a drive was half mounted. If the deletions are intended, run once with a
higher limit; `--max-delete 100` allows anything.

## Exit codes

| Code | Status (JSON)        | Meaning                                                                    |
|-----:|----------------------|----------------------------------------------------------------------------|
|    0 | `ok`                 | Sync complete. Tracks that failed are counted and listed but still give 0. |
|    1 | `error`              | Something else went wrong: rejected API key, failed library request, unreadable config file, … |
|    2 |                      | Usage error (unknown option, invalid value).                               |
|    3 | `not_configured`     | No server URL or no API key: run `login`.                                  |
|    4 | `unreachable`        | The server could not be reached. Nothing was changed.                      |
|    5 | `already_running`    | Another sync holds the lock.                                               |
|    6 | `empty_library`      | The server listed no tracks. Nothing was changed.                          |
|    7 | `connection_lost`    | The connection broke during the sync. `.part` files are kept, nothing was deleted. |
|    8 | `too_many_deletions` | The guard refused the deletions. Nothing was deleted.                      |
|  130 | `interrupted`        | Ctrl-C or SIGTERM. `.part` files are kept, nothing was deleted.            |

`login` uses 4 for an unreachable server and 1 for a rejected key; `server-info` uses 3, 4 and 1.
These codes are stable.

## JSON output

With `--json`, `sync` prints exactly one JSON object on stdout when it ends, whatever the outcome
(usage errors excepted), and nothing else; the human-readable lines go to stderr.

```json
{
  "status": "ok",
  "dry_run": false,
  "tracks": 1234,
  "downloaded": 1,
  "updated": 1,
  "up_to_date": 1229,
  "moved": 1,
  "removed": 1,
  "bytes": 31457280,
  "covers": 1,
  "playlists": 2,
  "failed": [
    { "track": "Artist/Album/Title.mp3", "error": "HTTP 404" }
  ]
}
```

| Field        | Meaning                                                                          |
|--------------|----------------------------------------------------------------------------------|
| `status`     | One of the statuses in the exit code table.                                      |
| `dry_run`    | Whether `--dry-run` was given.                                                   |
| `tracks`     | Tracks on the server (0 when the sync stopped before reading the library).      |
| `downloaded` | New tracks downloaded.                                                           |
| `updated`    | Tracks downloaded again: a different size on the server, or `--overwrite`.       |
| `up_to_date` | Tracks already present.                                                          |
| `moved`      | Tracks moved from the old flat layout that needed nothing else.                 |
| `removed`    | Audio files deleted because the server no longer has them.                       |
| `bytes`      | Bytes of audio received (with `--dry-run`: the size of what would be downloaded). |
| `covers`     | `cover.jpg` files written.                                                       |
| `playlists`  | Playlist files written because they were new or changed.                         |
| `failed`     | The tracks that failed: `track` is the local path below the music folder, `error` the reason (`HTTP 404`, `connection lost: timed out`, `received 100 bytes, the server lists 200`, …). |
| `message`    | Only when `status` is not `ok`: what happened, in words.                         |

Every track lands in exactly one of `downloaded`, `updated`, `up_to_date`, `moved` and `failed`,
except the downloads that never started because the connection broke or the sync was interrupted.

## Development

```bash
cargo fmt
cargo clippy --all-targets
cargo test
cargo build --release
```

On the 1st of every month, the [checkup workflow](.github/workflows/checkup.yml) runs formatting,
clippy and the tests on the newest stable Rust, `cargo audit` for security advisories and yanked
or unmaintained crates, and tries the dependency updates Cargo.toml allows. Anything that needs a
look becomes an issue labelled `checkup`. Run the same checks locally with
`.github/scripts/checkup.sh` (needs `cargo-audit` and `jq`).

To release, set `version` in Cargo.toml, then push a tag that matches it:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The [release workflow](.github/workflows/release.yml) builds static binaries for x86_64 and
aarch64, checks them and publishes them with their SHA-256 checksums.

## License

GNU General Public License v3.0 only. See [LICENSE](LICENSE).
