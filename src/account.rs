//! `login`, `logout` and `server-info`.

use std::path::Path;

use crate::api::{self, ApiError, Client};
use crate::config::{self, Credentials};
use crate::status::{Failure, Status};

pub const NOT_CONFIGURED: &str = "No Jellyfin server configured: run `jellyfin-music-rs login <url> <api-key>` (or set JELLYFIN_URL and JELLYFIN_API_KEY)";

fn missing_config_path() -> Failure {
    Failure::error("Cannot tell where the config file goes (no home folder): pass --config <file>")
}

/// A failed request: no answer at all means the server is unreachable, anything else is an error.
pub fn request_failure(error: &ApiError, url: &str) -> Failure {
    match error {
        ApiError::Transport(message) => Failure::new(
            Status::Unreachable,
            format!("Jellyfin server unreachable at {url}: {message}"),
        ),
        other => Failure::error(format!("Request to the Jellyfin server at {url} failed: {other}")),
    }
}

/// Checks the key with an authenticated `/System/Info` request, then saves both values. Nothing
/// is written when the check fails.
pub fn login(url: &str, key: &str, config_path: Option<&Path>) -> Result<(), Failure> {
    let path = config_path.ok_or_else(missing_config_path)?;
    let credentials = Credentials::new(url, key).map_err(|message| Failure::error(format!("Not saved: {message}")))?;
    let client = Client::new(&credentials);
    let info = client
        .system_info()
        .map_err(|error| request_failure(&error, &credentials.url))?;
    println!(
        "Logged in to {} (Jellyfin {})",
        info.server_name.as_deref().unwrap_or("Jellyfin"),
        info.version.as_deref().unwrap_or("unknown version")
    );
    config::write(path, &credentials)
        .map_err(|error| Failure::error(format!("Could not write {}: {error}", path.display())))?;
    println!("Saved the server URL and API key to {}", path.display());
    Ok(())
}

pub fn logout(config_path: Option<&Path>) -> Result<(), Failure> {
    let path = config_path.ok_or_else(missing_config_path)?;
    match config::remove(path) {
        Ok(true) => println!("Removed {}", path.display()),
        Ok(false) => println!("Not logged in: {} does not exist", path.display()),
        Err(error) => return Err(Failure::error(format!("Could not remove {}: {error}", path.display()))),
    }
    Ok(())
}

pub fn server_info(credentials: Option<Credentials>, user: Option<&str>) -> Result<(), Failure> {
    let credentials = credentials.ok_or_else(|| Failure::new(Status::NotConfigured, NOT_CONFIGURED))?;
    let client = Client::new(&credentials);
    let failure = |error: ApiError| request_failure(&error, &credentials.url);
    let info = client.system_info().map_err(failure)?;
    let users = client.users().map_err(failure)?;
    let user = api::select_user(users, user).map_err(Failure::error)?;
    println!("Server:  {}", info.server_name.as_deref().unwrap_or("unknown"));
    println!("Version: {}", info.version.as_deref().unwrap_or("unknown"));
    println!("URL:     {}", credentials.url);
    println!("User:    {}", user.name);
    Ok(())
}
