use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tempfile::Builder;

use crate::error::{OrbitError, Result, code};

pub const TEMP_PREFIX: &str = ".tmp-orbit-";

pub fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        OrbitError::invalid_argument(format!("{} has no parent directory", path.display()))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;

    let mut temp = Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|error| {
            OrbitError::io(format!("create temp file in {}", parent.display()), error)
        })?;

    temp.write_all(bytes)
        .map_err(|error| OrbitError::io("write temp file", error))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| OrbitError::io("fsync temp file", error))?;

    #[cfg(unix)]
    {
        // Best effort: some filesystems do not allow fsync on a directory.
        let _ = std::fs::File::open(parent).and_then(|directory| directory.sync_all());
    }

    #[cfg(debug_assertions)]
    if std::env::var_os("ORBIT_TEST_CRASH_AFTER_TEMP_WRITE").is_some() {
        std::process::abort();
    }

    if replace {
        temp.persist(path)
            .map_err(|error| OrbitError::io(format!("replace {}", path.display()), error.error))?;
    } else {
        temp.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                OrbitError::new(
                    code::ID_COLLISION,
                    format!("{} already exists", path.display()),
                )
            } else {
                OrbitError::io(format!("create {}", path.display()), error.error)
            }
        })?;
    }

    Ok(())
}

pub fn stale_temps(directory: &Path, max_age: Duration) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let now = SystemTime::now();
    let mut stale = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        if age >= max_age {
            stale.push(entry.path());
        }
    }
    stale.sort();
    stale
}

pub fn clean_stale_temps(directory: &Path, max_age: Duration) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for path in stale_temps(directory, max_age) {
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(OrbitError::io(
                    format!("remove stale temp {}", path.display()),
                    error,
                ));
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.md");

        write_atomic(&path, b"hello", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn replaces_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.md");
        std::fs::write(&path, "old").unwrap();

        write_atomic(&path, b"new", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn refuses_to_overwrite_with_noclobber() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.md");
        std::fs::write(&path, "old").unwrap();

        let error = write_atomic(&path, b"new", false).unwrap_err();
        assert_eq!(error.code, code::ID_COLLISION);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn cleans_only_stale_temps() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(format!("{TEMP_PREFIX}old"));
        let new = dir.path().join(format!("{TEMP_PREFIX}new"));
        std::fs::write(&old, "").unwrap();
        std::fs::write(&new, "").unwrap();

        let removed = clean_stale_temps(dir.path(), Duration::ZERO).unwrap();
        assert!(removed.contains(&old));
        assert!(removed.contains(&new));
        assert!(!old.exists());
        assert!(!new.exists());
    }
}
