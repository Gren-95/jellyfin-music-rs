//! Downloads one track into `<dest>.part` and renames it over `<dest>` only once it is complete,
//! so a failed download never replaces a good copy. An existing `.part` file is continued with a
//! `Range` request.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use ureq::Body;
use ureq::http::Response;

use crate::api::{self, ApiError, Client};
use crate::fsutil;
use crate::signal;

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// The server answered with this status: the track fails, the sync goes on.
    Status(u16),
    /// The connection failed, dropped or stalled: the sync stops starting downloads.
    ConnectionLost(String),
    /// Ctrl-C or SIGTERM.
    Interrupted,
    /// Anything else that fails this one track, with a short label and the details.
    Failed { label: &'static str, detail: String },
}

impl DownloadError {
    /// The short reason in `Failed [<label>]: <title>`.
    pub fn label(&self) -> String {
        match self {
            DownloadError::Status(code) => code.to_string(),
            DownloadError::ConnectionLost(_) => "connection lost".to_string(),
            DownloadError::Interrupted => "interrupted".to_string(),
            DownloadError::Failed { label, .. } => (*label).to_string(),
        }
    }

    /// The full reason, for the JSON output.
    pub fn detail(&self) -> String {
        match self {
            DownloadError::Status(code) => format!("HTTP {code}"),
            DownloadError::ConnectionLost(message) => format!("connection lost: {message}"),
            DownloadError::Interrupted => "interrupted".to_string(),
            DownloadError::Failed { detail, .. } => detail.clone(),
        }
    }
}

fn local_error(error: io::Error) -> DownloadError {
    DownloadError::Failed {
        label: "local error",
        detail: error.to_string(),
    }
}

/// Maps a request error. A signal also breaks off socket reads, so it is checked first.
fn request_error(error: ApiError) -> DownloadError {
    if signal::interrupted() {
        return DownloadError::Interrupted;
    }
    match error {
        ApiError::Transport(message) => DownloadError::ConnectionLost(message),
        ApiError::Status(code) => DownloadError::Status(code),
        ApiError::Other(detail) => DownloadError::Failed {
            label: "unexpected answer",
            detail,
        },
    }
}

/// Downloads track `id` to `dest`. `size` is the size the server lists, if any: a download of
/// another size fails. `on_bytes` is told about every chunk received.
pub fn download(
    client: &Client,
    user_id: &str,
    id: &str,
    dest: &Path,
    size: Option<u64>,
    on_bytes: &dyn Fn(u64),
) -> Result<(), DownloadError> {
    let part = fsutil::with_suffix(dest, ".part");
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(local_error)?;
    }
    let result = match fetch(client, &Client::stream_path(id), &part, size, on_bytes) {
        Err(DownloadError::Status(404)) => {
            let url = direct_stream_url(client, id, user_id)?;
            fetch(client, &url, &part, size, on_bytes)
        }
        result => result,
    };
    let received = fsutil::file_size(&part).unwrap_or(0);
    match result {
        // A signal that arrives after the last byte does not throw the download away.
        Err(DownloadError::Interrupted) if size == Some(received) => {}
        result => result?,
    }
    if let Some(expected) = size
        && received != expected
    {
        let _ = fs::remove_file(&part);
        return Err(DownloadError::Failed {
            label: "size mismatch",
            detail: format!("received {received} bytes, the server lists {expected}"),
        });
    }
    fs::rename(&part, dest).map_err(local_error)
}

/// The bash script's fallback for a 404 on the stream: `MediaSources[0].DirectStreamUrl` from
/// PlaybackInfo, relative to the server URL. Without one the track fails with the original 404.
fn direct_stream_url(client: &Client, id: &str, user_id: &str) -> Result<String, DownloadError> {
    let info = client.playback_info(id, user_id).map_err(|error| match error {
        ApiError::Status(_) | ApiError::Other(_) => DownloadError::Status(404),
        transport => request_error(transport),
    })?;
    info.media_sources
        .and_then(|sources| sources.into_iter().next())
        .and_then(|source| source.direct_stream_url)
        // Only a path on the same server, so the key is never sent anywhere else.
        .filter(|url| url.starts_with('/'))
        .ok_or(DownloadError::Status(404))
}

/// Fetches `path` into `part`, continuing what is already there. A 206 answer is appended; a 200
/// answer (the server ignored the range) starts the file over.
fn fetch(
    client: &Client,
    path: &str,
    part: &Path,
    size: Option<u64>,
    on_bytes: &dyn Fn(u64),
) -> Result<(), DownloadError> {
    let mut offset = fsutil::file_size(part).unwrap_or(0);
    if size.is_some_and(|size| offset > size) {
        fs::remove_file(part).map_err(local_error)?;
        offset = 0;
    }
    if offset > 0 && size == Some(offset) {
        return Ok(());
    }
    let range = |offset: u64| (offset > 0).then_some(offset);
    let mut response = client.get(path, range(offset)).map_err(request_error)?;
    if response.status().as_u16() == 416 && offset > 0 {
        fs::remove_file(part).map_err(local_error)?;
        offset = 0;
        response = client.get(path, None).map_err(request_error)?;
    }
    let append = match response.status().as_u16() {
        200 => false,
        206 if offset > 0 && content_range_start(&response) == Some(offset) => true,
        206 => {
            return Err(DownloadError::Failed {
                label: "unexpected answer",
                detail: "the server resumed at the wrong position".to_string(),
            });
        }
        code => return Err(DownloadError::Status(code)),
    };
    write_body(response, part, append, on_bytes)
}

/// The first byte position of a `Content-Range: bytes <start>-<end>/<total>` header.
fn content_range_start(response: &Response<Body>) -> Option<u64> {
    let value = response.headers().get("content-range")?.to_str().ok()?;
    parse_content_range_start(value)
}

fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes")?.trim_start();
    range.split_once('-')?.0.trim().parse().ok()
}

fn write_body(
    response: Response<Body>,
    part: &Path,
    append: bool,
    on_bytes: &dyn Fn(u64),
) -> Result<(), DownloadError> {
    let mut options = OpenOptions::new();
    if append {
        options.append(true);
    } else {
        options.write(true).create(true).truncate(true);
    }
    // Opened on the first data, so a transfer that breaks off before any byte arrives leaves the
    // `.part` file as it was instead of an empty one.
    let mut file = None;
    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        if signal::interrupted() {
            return Err(DownloadError::Interrupted);
        }
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(request_error(api::classify_io(error))),
        };
        let file = match &mut file {
            Some(file) => file,
            None => file.insert(options.open(part).map_err(local_error)?),
        };
        file.write_all(&buffer[..count]).map_err(local_error)?;
        on_bytes(count as u64);
    }
    if file.is_none() {
        options.open(part).map_err(local_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_start_of_a_content_range() {
        assert_eq!(parse_content_range_start("bytes 100000-319135/319136"), Some(100_000));
        assert_eq!(parse_content_range_start("bytes 0-9/*"), Some(0));
        assert_eq!(parse_content_range_start("bytes */319136"), None);
        assert_eq!(parse_content_range_start("items 1-2/3"), None);
    }

    #[test]
    fn labels_failures_like_the_bash_script() {
        assert_eq!(DownloadError::Status(404).label(), "404");
        assert_eq!(DownloadError::Status(404).detail(), "HTTP 404");
        let lost = DownloadError::ConnectionLost("timed out".to_string());
        assert_eq!(
            (lost.label().as_str(), lost.detail().as_str()),
            ("connection lost", "connection lost: timed out")
        );
    }
}
