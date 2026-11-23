//! Small file helpers: atomic writes and size lookups.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The size of a regular file (following symlinks), or `None` when there is none.
pub fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
}

/// `path` with `suffix` appended to its file name, e.g. `Title.flac` → `Title.flac.part`.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `contents` to a temporary file next to `path` (created with `mode`) and renames it
/// over `path`, so readers see either the old file or the complete new one.
pub fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".tmp{}", std::process::id()));
    let temp = path.with_file_name(temp_name);
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
pub mod testdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder under the system temp folder, removed again when dropped.
    pub struct TestDir(PathBuf);

    impl TestDir {
        pub fn new(name: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("jellyfin-music-rs-test-{name}-{}-{unique}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create test dir");
            TestDir(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        /// Writes a file below the folder, creating its parent folders.
        pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, contents).expect("write test file");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testdir::TestDir;
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn writes_atomically_with_the_given_mode() {
        let dir = TestDir::new("atomic");
        let path = dir.path().join("config");
        fs::write(&path, "old").expect("write old");
        write_atomic(&path, b"new", 0o600).expect("write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "new");
        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let names: Vec<_> = fs::read_dir(dir.path()).expect("list").collect();
        assert_eq!(names.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn sizes_and_suffixes() {
        let dir = TestDir::new("size");
        let file = dir.write("a/Title.flac", b"12345");
        assert_eq!(file_size(&file), Some(5));
        assert_eq!(file_size(&dir.path().join("a")), None);
        assert_eq!(file_size(&dir.path().join("missing")), None);
        assert_eq!(with_suffix(&file, ".part"), dir.path().join("a/Title.flac.part"));
    }
}
