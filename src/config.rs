//! The server URL and API key. Highest priority first: `--url`/`--key`, the environment
//! (`JELLYFIN_URL`, `JELLYFIN_API_KEY`; clap merges these two levels), then the config file.
//!
//! The config file holds `KEY=VALUE` lines in shell syntax, so the file the old bash script
//! wrote with `printf '%q'` can be used as it is. The key is never printed: not in errors, not in
//! `Debug` output.

use std::fmt;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use crate::dirs;
use crate::fsutil;
use crate::shell;

pub const URL_VARIABLE: &str = "JELLYFIN_URL";
pub const KEY_VARIABLE: &str = "JELLYFIN_API_KEY";

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub url: String,
    pub key: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("url", &self.url)
            .field("key", &"<hidden>")
            .finish()
    }
}

impl Credentials {
    /// Checks and normalises both values: see [`normalize_url`] and [`check_key`].
    pub fn new(url: &str, key: &str) -> Result<Self, String> {
        let url = normalize_url(url)?;
        check_key(key)?;
        Ok(Credentials {
            url,
            key: key.to_string(),
        })
    }
}

/// `$XDG_CONFIG_HOME/jellyfin-music-rs/config` (or `~/.config/…`).
pub fn default_path() -> Option<PathBuf> {
    dirs::config_home().map(|dir| dir.join("jellyfin-music-rs").join("config"))
}

/// Drops trailing slashes and checks the URL is an `http://` or `https://` address on one line.
pub fn normalize_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let lower = url.to_ascii_lowercase();
    let host = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
        .ok_or("the server URL must start with http:// or https://")?;
    if host.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the server URL is not valid".to_string());
    }
    Ok(url.to_string())
}

/// The key goes into the `Token="…"` part of the authorization header, which has no way to
/// escape a quote or a backslash.
pub fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("the API key is empty".to_string());
    }
    if key.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err("the API key contains a quote, a backslash or a control character".to_string());
    }
    Ok(())
}

/// The two values a config file can set. Empty values count as unset.
#[derive(Default, PartialEq, Eq)]
pub struct FileValues {
    pub url: Option<String>,
    pub key: Option<String>,
}

/// Parses the config file. Blank lines and `#` comments are skipped, unknown names ignored.
pub fn parse(text: &str) -> Result<FileValues, String> {
    let mut values = FileValues::default();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |message: &str| format!("line {}: {message}", index + 1);
        let (name, rest) = line.split_once('=').ok_or_else(|| at("expected NAME=VALUE"))?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(at("expected NAME=VALUE"));
        }
        let (value, after) = shell::parse_word(rest).map_err(|error| at(&error))?;
        let after = after.trim_start();
        if !after.is_empty() && !after.starts_with('#') {
            return Err(at("unexpected text after the value"));
        }
        let value = Some(value).filter(|value| !value.is_empty());
        match name {
            URL_VARIABLE => values.url = value,
            KEY_VARIABLE => values.key = value,
            _ => {}
        }
    }
    Ok(values)
}

/// Reads the config file. A missing file is the same as an empty one.
pub fn read(path: &Path) -> Result<FileValues, String> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|error| format!("Invalid config file {}: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileValues::default()),
        Err(error) => Err(format!("Could not read the config file {}: {error}", path.display())),
    }
}

/// The text `login` writes. Values are shell-quoted so bash can source the file too.
pub fn format(credentials: &Credentials) -> String {
    format!(
        "# Written by jellyfin-music-rs login.\n{URL_VARIABLE}={}\n{KEY_VARIABLE}={}\n",
        shell::quote(&credentials.url),
        shell::quote(&credentials.key)
    )
}

/// Writes the config file atomically with mode 0600, creating its folder (mode 0700) as needed.
pub fn write(path: &Path, credentials: &Credentials) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    fsutil::write_atomic(path, format(credentials).as_bytes(), 0o600)
}

/// Removes the config file; `Ok(false)` when there was none.
pub fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Fills in what the command line and environment left unset from the config file (read only
/// when needed). `Ok(None)`: no URL or no key from any source.
pub fn resolve(url: Option<String>, key: Option<String>, path: Option<&Path>) -> Result<Option<Credentials>, String> {
    let mut url = url.filter(|value| !value.is_empty());
    let mut key = key.filter(|value| !value.is_empty());
    if (url.is_none() || key.is_none())
        && let Some(path) = path
    {
        let file = read(path)?;
        url = url.or(file.url);
        key = key.or(file.key);
    }
    match (url, key) {
        (Some(url), Some(key)) => Credentials::new(&url, &key).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::testdir::TestDir;
    use std::os::unix::fs::PermissionsExt;

    const FAKE_KEY: &str = "fake0123456789abcdef0123456789ab";

    fn credentials(url: &str, key: &str) -> Credentials {
        Credentials {
            url: url.to_string(),
            key: key.to_string(),
        }
    }

    #[test]
    fn reads_the_file_the_bash_script_wrote() {
        let text = format!("JELLYFIN_URL=http://192.0.2.10:8096\nJELLYFIN_API_KEY={FAKE_KEY}\n");
        let values = parse(&text).expect("parse");
        assert_eq!(values.url.as_deref(), Some("http://192.0.2.10:8096"));
        assert_eq!(values.key.as_deref(), Some(FAKE_KEY));
    }

    #[test]
    fn reads_every_quoting_style() {
        let text = "# comment\n\n  JELLYFIN_URL=\"https://jellyfin.example.com/\"  # trailing comment\n\
                    JELLYFIN_API_KEY=$'fake\\x41key'\nOTHER='ignored'\n";
        let values = parse(text).expect("parse");
        assert_eq!(values.url.as_deref(), Some("https://jellyfin.example.com/"));
        assert_eq!(values.key.as_deref(), Some("fakeAkey"));
        let escaped = parse("JELLYFIN_API_KEY=fake\\ key\\'s\n").expect("parse");
        assert_eq!(escaped.key.as_deref(), Some("fake key's"));
        let single = parse("JELLYFIN_API_KEY='fake key'\\''s'\n").expect("parse");
        assert_eq!(single.key.as_deref(), Some("fake key's"));
    }

    #[test]
    fn empty_values_count_as_unset() {
        let values = parse("JELLYFIN_URL=''\nJELLYFIN_API_KEY=\n").expect("parse");
        assert!(values.url.is_none() && values.key.is_none());
    }

    #[test]
    fn reports_the_line_but_not_the_value() {
        let error = parse(&format!("# c\nJELLYFIN_API_KEY='{FAKE_KEY}\n"))
            .err()
            .expect("invalid");
        assert_eq!(error, "line 2: unterminated single quote");
        let error = parse(&format!("JELLYFIN_API_KEY={FAKE_KEY} extra\n"))
            .err()
            .expect("invalid");
        assert_eq!(error, "line 1: unexpected text after the value");
        assert!(parse("just text\n").is_err());
        assert!(parse("BAD NAME=x\n").is_err());
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_url("https://jellyfin.example.com/").as_deref(),
            Ok("https://jellyfin.example.com")
        );
        assert_eq!(
            normalize_url("http://192.0.2.10:8096//").as_deref(),
            Ok("http://192.0.2.10:8096")
        );
        assert!(normalize_url("jellyfin.example.com").is_err());
        assert!(normalize_url("http://").is_err());
        assert!(normalize_url("http://a b").is_err());
    }

    #[test]
    fn rejects_keys_the_header_cannot_carry() {
        assert!(check_key(FAKE_KEY).is_ok());
        assert!(check_key("").is_err());
        assert!(check_key("a\"b").is_err());
        assert!(check_key("a\\b").is_err());
        assert!(check_key("a\nb").is_err());
    }

    #[test]
    fn debug_output_hides_the_key() {
        let text = format!("{:?}", credentials("http://192.0.2.10", FAKE_KEY));
        assert!(!text.contains(FAKE_KEY), "{text}");
    }

    #[test]
    fn flags_and_environment_win_over_the_file() {
        let dir = TestDir::new("resolve");
        let path = dir.write("config", b"JELLYFIN_URL=http://192.0.2.10/\nJELLYFIN_API_KEY=filekey\n");
        let from_file = resolve(None, None, Some(&path)).expect("resolve");
        assert_eq!(from_file, Some(credentials("http://192.0.2.10", "filekey")));
        let mixed = resolve(None, Some("flagkey".into()), Some(&path)).expect("resolve");
        assert_eq!(mixed, Some(credentials("http://192.0.2.10", "flagkey")));
        let flags = resolve(Some("https://jellyfin.example.com".into()), Some("k".into()), None).expect("resolve");
        assert_eq!(flags, Some(credentials("https://jellyfin.example.com", "k")));
        assert_eq!(resolve(Some(String::new()), None, None), Ok(None));
        let missing = dir.path().join("missing");
        assert_eq!(
            resolve(Some("http://192.0.2.10".into()), None, Some(&missing)),
            Ok(None)
        );
    }

    #[test]
    fn a_broken_file_is_not_read_when_flags_give_everything() {
        let dir = TestDir::new("broken");
        let path = dir.write("config", b"JELLYFIN_URL='open\n");
        assert!(resolve(None, None, Some(&path)).is_err());
        let both = resolve(Some("http://192.0.2.10".into()), Some("k".into()), Some(&path));
        assert!(matches!(both, Ok(Some(_))));
    }

    #[test]
    fn writes_a_private_file_that_reads_back() {
        let dir = TestDir::new("write");
        let path = dir.path().join("new/folder/config");
        let original = credentials("https://jellyfin.example.com", "fake key's $x #y");
        write(&path, &original).expect("write");
        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let folder_mode = fs::metadata(path.parent().expect("parent"))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(folder_mode & 0o777, 0o700);
        let values = read(&path).expect("read");
        assert_eq!(values.url.as_deref(), Some(original.url.as_str()));
        assert_eq!(values.key.as_deref(), Some(original.key.as_str()));
        assert_eq!(remove(&path).ok(), Some(true));
        assert_eq!(remove(&path).ok(), Some(false));
    }
}
