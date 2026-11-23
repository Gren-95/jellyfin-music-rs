//! How a command ends: the status names used in the JSON output and the exit codes that go with
//! them. Both are part of the interface scripts rely on, so they must stay stable.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Error,
    NotConfigured,
    Unreachable,
    AlreadyRunning,
    EmptyLibrary,
    ConnectionLost,
    TooManyDeletions,
    Interrupted,
}

impl Status {
    /// The name used for `status` in the JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::NotConfigured => "not_configured",
            Status::Unreachable => "unreachable",
            Status::AlreadyRunning => "already_running",
            Status::EmptyLibrary => "empty_library",
            Status::ConnectionLost => "connection_lost",
            Status::TooManyDeletions => "too_many_deletions",
            Status::Interrupted => "interrupted",
        }
    }

    /// The process exit code. 2 is left to clap for usage errors.
    pub fn exit_code(self) -> u8 {
        match self {
            Status::Ok => 0,
            Status::Error => 1,
            Status::NotConfigured => 3,
            Status::Unreachable => 4,
            Status::AlreadyRunning => 5,
            Status::EmptyLibrary => 6,
            Status::ConnectionLost => 7,
            Status::TooManyDeletions => 8,
            Status::Interrupted => 130,
        }
    }
}

/// A command that stopped before finishing: why, and the message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub status: Status,
    pub message: String,
}

impl Failure {
    pub fn new(status: Status, message: impl Into<String>) -> Self {
        Failure {
            status,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Failure::new(Status::Error, message)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Status; 9] = [
        Status::Ok,
        Status::Error,
        Status::NotConfigured,
        Status::Unreachable,
        Status::AlreadyRunning,
        Status::EmptyLibrary,
        Status::ConnectionLost,
        Status::TooManyDeletions,
        Status::Interrupted,
    ];

    #[test]
    fn exit_codes_are_distinct_and_leave_2_to_clap() {
        let mut codes: Vec<u8> = ALL.iter().map(|status| status.exit_code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len());
        assert!(!codes.contains(&2));
    }

    #[test]
    fn documented_codes_do_not_change() {
        let table: Vec<(&str, u8)> = ALL.iter().map(|s| (s.name(), s.exit_code())).collect();
        assert_eq!(
            table,
            [
                ("ok", 0),
                ("error", 1),
                ("not_configured", 3),
                ("unreachable", 4),
                ("already_running", 5),
                ("empty_library", 6),
                ("connection_lost", 7),
                ("too_many_deletions", 8),
                ("interrupted", 130),
            ]
        );
    }
}
