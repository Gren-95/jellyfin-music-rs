//! Whether a track needs downloading.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The local file is there and matches the server's size (or the server lists no size).
    UpToDate,
    /// There is no local file.
    Download,
    /// The local file differs in size from the server's copy, or `--overwrite` was given.
    Update,
}

/// Decides from the local file's size (`None` when missing), the server's size (when known) and
/// `--overwrite`.
pub fn decide(local_size: Option<u64>, server_size: Option<u64>, overwrite: bool) -> Action {
    match local_size {
        None => Action::Download,
        Some(_) if overwrite => Action::Update,
        Some(local) if server_size.is_some_and(|server| server != local) => Action::Update,
        Some(_) => Action::UpToDate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_downloaded() {
        assert_eq!(decide(None, Some(10), false), Action::Download);
        assert_eq!(decide(None, None, true), Action::Download);
    }

    #[test]
    fn present_files_are_kept_unless_the_size_differs() {
        assert_eq!(decide(Some(10), Some(10), false), Action::UpToDate);
        assert_eq!(decide(Some(10), None, false), Action::UpToDate);
        assert_eq!(decide(Some(10), Some(11), false), Action::Update);
        assert_eq!(decide(Some(0), Some(11), false), Action::Update);
    }

    #[test]
    fn overwrite_downloads_everything_again() {
        assert_eq!(decide(Some(10), Some(10), true), Action::Update);
        assert_eq!(decide(Some(10), None, true), Action::Update);
    }
}
