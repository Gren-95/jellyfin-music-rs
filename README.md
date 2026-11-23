# jellyfin-music-rs

[![CI](https://github.com/Gren-95/jellyfin-music-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Gren-95/jellyfin-music-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Gren-95/jellyfin-music-rs)](https://github.com/Gren-95/jellyfin-music-rs/releases/latest)
[![License: GPL-3.0](https://img.shields.io/github/license/Gren-95/jellyfin-music-rs)](LICENSE)

Mirror the music library of your [Jellyfin](https://jellyfin.org) server into a local folder.

- **Keeps in step:** new tracks are downloaded, changed ones downloaded again, removed ones
  deleted (with a guard against mass deletions).
- **Safe on a flaky connection:** downloads resume, a stalled transfer fails instead of hanging,
  and a server that is down or lists nothing changes nothing.
- **Covers and playlists:** `cover.jpg` per album, and your playlists as `.m3u` files.
- **Made for timers:** one static binary, parallel downloads, a JSON summary and clear exit codes.

Not an official Jellyfin project.

## Install

Static builds for Linux (x86_64, aarch64):

```bash
arch=$(uname -m)
base=https://github.com/Gren-95/jellyfin-music-rs/releases/latest/download
curl -fsSLO "$base/jellyfin-music-rs-$arch-linux.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "jellyfin-music-rs-$arch-linux.tar.gz" jellyfin-music-rs
install -Dm755 jellyfin-music-rs ~/.local/bin/jellyfin-music-rs
```

Or from source (Rust 1.89 or newer): `cargo install --locked --git https://github.com/Gren-95/jellyfin-music-rs`

## Quick start

Create an API key in Jellyfin under Dashboard, API Keys, then:

```bash
jellyfin-music-rs login https://jellyfin.example.com YOUR_API_KEY
jellyfin-music-rs sync --dry-run    # see what would happen
jellyfin-music-rs sync
```

Music goes to `~/Music` (your `XDG_MUSIC_DIR`), or wherever `--dir` points.

## Usage

| Command | What it does |
|---------|--------------|
| `login <url> <key>` | Checks the key, then saves the URL and key (mode 0600) |
| `logout` | Removes them |
| `server-info` | Shows the server, its version and the synced user |
| `sync` | Mirrors the library |

`sync` options:

| Option | Default | |
|--------|---------|---|
| `--dir <path>` | `~/Music` | Music folder (env `JELLYFIN_MUSIC_DIR`) |
| `--jobs <n>` | 4 | Downloads at the same time |
| `--dry-run` | | Show what would happen, change nothing |
| `--overwrite` | | Download every track and cover again |
| `--no-covers`, `--no-playlists` | | Skip covers or playlists |
| `--max-delete <percent>` | 50 | Deletion guard, see below |
| `--json` | | One JSON summary on stdout |
| `--no-progress` | | No progress bars |
| `--user <name>` | first user | Whose library to sync |
| `--url`, `--key`, `--config <file>` | | Override the saved login (env `JELLYFIN_URL`, `JELLYFIN_API_KEY`) |

The config file is `~/.config/jellyfin-music-rs/config`, with `JELLYFIN_URL=…` and
`JELLYFIN_API_KEY=…` lines in shell syntax. The key is never printed or logged.

## How a sync works

1. Only one sync runs at a time, and nothing is touched when the server is unreachable or lists
   no tracks.
2. Tracks land in `Artist/Album/Title.ext`. Characters that are invalid in file names are dropped,
   and two tracks with the same name get `Title (2).ext`.
3. Missing tracks are downloaded, and present ones again when their size differs from the
   server's. Downloads go to a `.part` file first, so a failed one never replaces a good copy,
   and the next run resumes it.
4. `cover.jpg` comes from the album's image, or else from the first track's own image.
   Playlists are written to `Playlists/<name>.m3u` with relative paths.
5. Files the server no longer has are deleted (see below), then empty folders are removed.

If the connection breaks or you press Ctrl-C, running downloads stop, `.part` files are kept and
nothing is deleted.

## Deletion

Only files the sync manages are deleted: audio files (`mp3 flac ogg opus m4a wav aac`),
`cover.jpg` of albums that are gone, stale `.m3u` files in `Playlists/`, and leftover `.part`
files. Your own files are never touched.

Deletion only happens after a complete run. If it would remove more than `--max-delete` percent of
your local tracks (and at least 10 files), nothing is deleted and the sync exits with code 8. That
protects against a server that suddenly lists far fewer tracks, such as a half-mounted drive. Run
once with `--max-delete 100` if the deletions are intended.

## Exit codes

| Code | Status | Meaning |
|-----:|--------|---------|
| 0 | `ok` | Done (failed tracks are listed, but still 0) |
| 1 | `error` | Rejected key, failed request, bad config |
| 2 | | Usage error |
| 3 | `not_configured` | No URL or key: run `login` |
| 4 | `unreachable` | Server not reachable, nothing changed |
| 5 | `already_running` | Another sync is running |
| 6 | `empty_library` | Server listed no tracks, nothing changed |
| 7 | `connection_lost` | Connection broke, nothing deleted |
| 8 | `too_many_deletions` | Guard refused, nothing deleted |
| 130 | `interrupted` | Ctrl-C or SIGTERM, nothing deleted |

With `--json`, the summary looks like this (`message` is added when the status is not `ok`):

```json
{"status": "ok", "dry_run": false, "tracks": 1234, "downloaded": 1, "updated": 1,
 "up_to_date": 1229, "moved": 0, "removed": 1, "bytes": 31457280, "covers": 1,
 "playlists": 2, "failed": [{"track": "Artist/Album/Title.mp3", "error": "HTTP 404"}]}
```

## Development

```bash
cargo fmt && cargo clippy --all-targets && cargo test
```

A [monthly checkup](.github/workflows/checkup.yml) tests the newest stable Rust, runs
`cargo audit` and tries dependency updates, and opens an issue when something needs a look. To
release, set `version` in Cargo.toml and push a matching tag (`git tag v0.1.0 && git push origin
v0.1.0`); the [release workflow](.github/workflows/release.yml) builds and publishes the binaries.

## License

GNU General Public License v3.0. See [LICENSE](LICENSE).
