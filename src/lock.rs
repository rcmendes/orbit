use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{OrbitError, Result};

#[derive(Debug)]
pub struct BoardLock {
    file: File,
}

impl BoardLock {
    pub fn acquire(path: &Path, timeout_ms: u64, exclusive: bool) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| OrbitError::io(format!("create {}", parent.display()), error))?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| OrbitError::io(format!("open lock {}", path.display()), error))?;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let result = if exclusive {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };

            match result {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(OrbitError::locked());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(OrbitError::io(format!("lock {}", path.display()), error));
                }
            }
        }
    }
}

impl Drop for BoardLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[test]
    #[cfg(unix)]
    fn exclusive_lock_excludes_other_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");

        let first = BoardLock::acquire(&path, 0, true).unwrap();
        let error = BoardLock::acquire(&path, 0, true).unwrap_err();
        assert_eq!(error.code, crate::error::code::LOCKED);
        assert!(error.retryable);

        drop(first);
        let _second = BoardLock::acquire(&path, 0, true).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn shared_lock_is_blocked_by_exclusive_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");

        let exclusive = BoardLock::acquire(&path, 0, true).unwrap();
        assert!(BoardLock::acquire(&path, 0, false).is_err());
        drop(exclusive);
        let _shared = BoardLock::acquire(&path, 0, false).unwrap();
    }
}
