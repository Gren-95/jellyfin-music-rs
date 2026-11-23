//! One sync at a time: an exclusive lock on a file in `$XDG_RUNTIME_DIR` (else the cache folder),
//! taken with `File::try_lock` and released by the OS when the process ends, however it ends.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::dirs;

const FILE_NAME: &str = "jellyfin-music-rs.lock";

/// Holds the lock until dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

#[derive(Debug)]
pub enum LockError {
    AlreadyRunning,
    Io(io::Error),
}

pub fn default_path() -> Option<PathBuf> {
    dirs::runtime_dir()
        .map(|dir| dir.join(FILE_NAME))
        .or_else(|| dirs::cache_home().map(|dir| dir.join("jellyfin-music-rs").join(FILE_NAME)))
}

pub fn acquire(path: &Path) -> Result<Lock, LockError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(LockError::Io)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(Lock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(LockError::AlreadyRunning),
        Err(TryLockError::Error(error)) => Err(LockError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::testdir::TestDir;

    #[test]
    fn a_second_lock_fails_until_the_first_is_dropped() {
        let dir = TestDir::new("lock");
        let path = dir.path().join("run/sync.lock");
        let first = acquire(&path).expect("first lock");
        assert!(matches!(acquire(&path), Err(LockError::AlreadyRunning)));
        drop(first);
        assert!(acquire(&path).is_ok());
    }
}
