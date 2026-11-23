//! The Jellyfin endpoints the tool uses, over one ureq agent whose timeouts keep a sync from
//! hanging on a server that went away.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::config::RedirectAuthHeaders;
use ureq::http::Response;
use ureq::{Agent, Body};

use crate::config::Credentials;

const NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Opening a connection (and resolving the host name) may take this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A transfer with no data for this long fails. This is ureq's per-read timeout, which restarts
/// on every read from the socket, so a long download that keeps moving is never cut off.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole reachability check, like the bash script's `curl --max-time`.
const REACHABLE_TIMEOUT: Duration = Duration::from_secs(15);
/// JSON answers are read into memory up to this size (a page of 500 tracks is about 1.5 MB).
const JSON_LIMIT: u64 = 256 << 20;
/// Tracks, playlists and playlist entries are listed this many at a time.
pub const PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No complete answer: DNS or connection failure, a timeout or a dropped connection.
    Transport(String),
    /// The server answered with this unexpected HTTP status.
    Status(u16),
    /// Anything else: a TLS failure, an invalid URL, an answer that is not the expected JSON.
    Other(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Transport(message) | ApiError::Other(message) => f.write_str(message),
            ApiError::Status(401) => f.write_str("HTTP 401: the server rejected the API key"),
            ApiError::Status(code) => write!(f, "HTTP {code}"),
        }
    }
}

/// Sorts a ureq error into the three kinds above. The messages never contain a URL, because a
/// stream URL from the server may carry an access token.
pub fn classify(error: ureq::Error) -> ApiError {
    match error {
        ureq::Error::StatusCode(code) => ApiError::Status(code),
        ureq::Error::Timeout(_) => ApiError::Transport("timed out".to_string()),
        ureq::Error::HostNotFound => ApiError::Transport("host not found".to_string()),
        ureq::Error::ConnectionFailed => ApiError::Transport("connection failed".to_string()),
        ureq::Error::Io(error) => ApiError::Transport(error.to_string()),
        ureq::Error::Protocol(_) | ureq::Error::BodyStalled => {
            ApiError::Transport("the connection broke off mid-answer".to_string())
        }
        ureq::Error::BadUri(_) => ApiError::Other("invalid URL".to_string()),
        other => ApiError::Other(other.to_string()),
    }
}

/// Classifies an error from reading a response body, which ureq wraps in an `io::Error`.
pub fn classify_io(error: std::io::Error) -> ApiError {
    classify(ureq::Error::from(error))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub server_name: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    pub id: String,
    pub name: String,
}

/// A track (`Audio` item) with the fields the sync uses.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    pub name: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    /// Set only when the track belongs to an album entity (a folder Jellyfin grouped as one).
    pub album_id: Option<String>,
    /// Set when that album has a primary image, so the image is only requested when it exists.
    pub album_primary_image_tag: Option<String>,
    /// The track's own images; `Primary` is set when it has one (usually embedded cover art).
    pub image_tags: Option<ImageTags>,
    pub path: Option<String>,
    pub index_number: Option<i64>,
    pub run_time_ticks: Option<i64>,
    pub media_sources: Option<Vec<MediaSource>>,
}

impl Item {
    /// The file size the server lists (`MediaSources[0].Size`), when it gives a positive one.
    pub fn size(&self) -> Option<u64> {
        let source = self.media_sources.as_ref()?.first()?;
        source
            .size
            .and_then(|size| u64::try_from(size).ok())
            .filter(|&size| size > 0)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageTags {
    pub primary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSource {
    pub size: Option<i64>,
    pub direct_stream_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Playlist {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistEntry {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfo {
    pub media_sources: Option<Vec<MediaSource>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Page<T> {
    items: Vec<T>,
    total_record_count: usize,
}

/// The `Authorization` header Jellyfin expects. The device id is constant so the server does not
/// list a new device for every run.
fn authorization(key: &str) -> String {
    format!(
        "MediaBrowser Client=\"{NAME}\", Device=\"{NAME}\", DeviceId=\"{NAME}\", Version=\"{VERSION}\", Token=\"{key}\""
    )
}

/// Percent-encodes an id for use in a URL path or query.
fn encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

/// Picks the user whose library is synced: the one named (case-insensitively), else the first
/// one the server lists, as the bash script did.
pub fn select_user(users: Vec<User>, name: Option<&str>) -> Result<User, String> {
    match name {
        Some(name) => {
            let wanted = name.to_lowercase();
            users
                .into_iter()
                .find(|user| user.name.to_lowercase() == wanted)
                .ok_or_else(|| format!("The server has no user named {name}"))
        }
        None => users
            .into_iter()
            .next()
            .ok_or_else(|| "The server lists no users".to_string()),
    }
}

#[derive(Clone)]
pub struct Client {
    agent: Agent,
    base: String,
    authorization: String,
}

impl Client {
    pub fn new(credentials: &Credentials) -> Self {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(format!("{NAME}/{VERSION}"))
            .redirect_auth_headers(RedirectAuthHeaders::SameHost)
            .timeout_resolve(Some(CONNECT_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_per_read(Some(STALL_TIMEOUT))
            .timeout_per_write(Some(STALL_TIMEOUT))
            .build()
            .new_agent();
        Client {
            agent,
            base: credentials.url.clone(),
            authorization: authorization(&credentials.key),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Whether the server answers at all, with any status: a 401 or 404 still proves it is up.
    /// Uses the public endpoint, so no key is sent.
    pub fn check_reachable(&self) -> Result<(), ApiError> {
        self.agent
            .get(format!("{}/System/Info/Public", self.base))
            .config()
            .timeout_global(Some(REACHABLE_TIMEOUT))
            .build()
            .call()
            .map(|_| ())
            .map_err(classify)
    }

    /// Sends an authenticated GET for `path` (which starts with `/`), optionally asking for the
    /// bytes from `range_start` on. Any HTTP status is returned as a response.
    pub fn get(&self, path: &str, range_start: Option<u64>) -> Result<Response<Body>, ApiError> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("Authorization", &self.authorization);
        if let Some(start) = range_start {
            request = request.header("Range", format!("bytes={start}-"));
        }
        request.call().map_err(classify)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let mut response = self.get(path, None)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(ApiError::Status(status));
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(classify)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| ApiError::Other(format!("unexpected answer from the server: {error}")))
    }

    /// Reads every page of a list (`path` already has a query string). Fails rather than return a
    /// partial list, since a missing track would be deleted locally.
    fn get_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, ApiError> {
        let mut items: Vec<T> = Vec::new();
        let mut total = None;
        loop {
            let page: Page<T> = self.get_json(&format!("{path}&StartIndex={}&Limit={PAGE_SIZE}", items.len()))?;
            if *total.get_or_insert(page.total_record_count) != page.total_record_count {
                return Err(ApiError::Other(
                    "the library changed while it was being read".to_string(),
                ));
            }
            let received = page.items.len();
            items.extend(page.items);
            if received == 0 || items.len() >= page.total_record_count {
                break;
            }
        }
        let total = total.unwrap_or_default();
        if items.len() != total {
            return Err(ApiError::Other(format!(
                "the server listed {} of {total} items",
                items.len()
            )));
        }
        Ok(items)
    }

    pub fn system_info(&self) -> Result<SystemInfo, ApiError> {
        self.get_json("/System/Info")
    }

    pub fn users(&self) -> Result<Vec<User>, ApiError> {
        self.get_json("/Users")
    }

    pub fn audio_items(&self, user_id: &str) -> Result<Vec<Item>, ApiError> {
        self.get_all(&format!(
            "/Users/{}/Items?Recursive=true&IncludeItemTypes=Audio&Fields=Path,MediaSources",
            encode(user_id)
        ))
    }

    pub fn playlists(&self, user_id: &str) -> Result<Vec<Playlist>, ApiError> {
        self.get_all(&format!(
            "/Users/{}/Items?Recursive=true&IncludeItemTypes=Playlist",
            encode(user_id)
        ))
    }

    pub fn playlist_entries(&self, playlist_id: &str, user_id: &str) -> Result<Vec<PlaylistEntry>, ApiError> {
        self.get_all(&format!(
            "/Playlists/{}/Items?UserId={}",
            encode(playlist_id),
            encode(user_id)
        ))
    }

    pub fn playback_info(&self, item_id: &str, user_id: &str) -> Result<PlaybackInfo, ApiError> {
        self.get_json(&format!(
            "/Items/{}/PlaybackInfo?UserId={}",
            encode(item_id),
            encode(user_id)
        ))
    }

    pub fn stream_path(item_id: &str) -> String {
        format!("/Audio/{}/stream?static=true", encode(item_id))
    }

    /// The primary image of an album or a track.
    pub fn primary_image_path(item_id: &str) -> String {
        format!("/Items/{}/Images/Primary?format=Jpg", encode(item_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, name: &str) -> User {
        User {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn authorization_header_has_a_constant_device() {
        assert_eq!(
            authorization("fakekey"),
            format!(
                "MediaBrowser Client=\"jellyfin-music-rs\", Device=\"jellyfin-music-rs\", \
                 DeviceId=\"jellyfin-music-rs\", Version=\"{VERSION}\", Token=\"fakekey\""
            )
        );
    }

    #[test]
    fn encodes_ids() {
        assert_eq!(encode("0f8f2c3a9e"), "0f8f2c3a9e");
        assert_eq!(encode("a b/c?d"), "a%20b%2Fc%3Fd");
    }

    #[test]
    fn selects_the_first_or_the_named_user() {
        let users = || vec![user("1", "alice"), user("2", "Bob")];
        assert_eq!(select_user(users(), None).map(|u| u.id).as_deref(), Ok("1"));
        assert_eq!(select_user(users(), Some("bob")).map(|u| u.id).as_deref(), Ok("2"));
        assert!(select_user(users(), Some("carol")).is_err());
        assert!(select_user(Vec::new(), None).is_err());
    }

    #[test]
    fn reads_the_size_only_when_positive() {
        let item = |size: Option<i64>| Item {
            media_sources: Some(vec![MediaSource {
                size,
                direct_stream_url: None,
            }]),
            ..Item::default()
        };
        assert_eq!(item(Some(1234)).size(), Some(1234));
        assert_eq!(item(Some(0)).size(), None);
        assert_eq!(item(None).size(), None);
        assert_eq!(Item::default().size(), None);
    }

    #[test]
    fn parses_a_jellyfin_item() {
        let json = r#"{"Name":"Song","Id":"abc","Path":"/media/music/Song.flac","Album":"Album",
            "AlbumArtist":"Artist","AlbumId":"def","AlbumPrimaryImageTag":"tag","RunTimeTicks":1807440000,
            "IndexNumber":3,"MediaSources":[{"Size":319136,"Container":"flac"}],"Type":"Audio",
            "ImageTags":{"Primary":"0f1e2d","Thumb":"x"}}"#;
        let item: Item = serde_json::from_str(json).expect("parse");
        assert_eq!(item.id, "abc");
        assert_eq!(item.album_id.as_deref(), Some("def"));
        assert_eq!(item.size(), Some(319_136));
        assert_eq!(item.image_tags.and_then(|tags| tags.primary).as_deref(), Some("0f1e2d"));
        let sparse: Item = serde_json::from_str(r#"{"Id":"x","Name":null,"MediaSources":null}"#).expect("parse");
        assert!(sparse.name.is_none() && sparse.size().is_none());
    }

    #[test]
    fn transport_errors_are_told_apart_from_answers() {
        assert_eq!(classify(ureq::Error::StatusCode(404)), ApiError::Status(404));
        assert!(matches!(classify(ureq::Error::HostNotFound), ApiError::Transport(_)));
        let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert!(matches!(classify_io(reset), ApiError::Transport(_)));
        let bad = classify(ureq::Error::BadUri("http://secret@x".to_string()));
        assert_eq!(bad, ApiError::Other("invalid URL".to_string()));
    }
}
