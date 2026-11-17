//! This module handles the locking logic during compilation.

use crate::util::flock;
use crate::{
    CargoResult,
    compiler::{BuildRunner, Unit},
    util::{FileLock, Filesystem},
};

use anyhow::bail;
use cargo_util::collections::HashMap;
use std::{
    collections::hash_map::Entry,
    fmt::{Display, Formatter},
    path::PathBuf,
    sync::{Mutex, RwLock},
};
use tracing::instrument;

pub(super) const GC_LOCK_FILE: &str = ".cargo-gc-lock";

/// A struct to store the lock handles for build units during compilation.
pub struct LockManager {
    locks: RwLock<HashMap<LockKey, FileLock>>,
    gc_locks: Mutex<HashMap<PathBuf, FileLock>>,
}

impl LockManager {
    pub fn new() -> Self {
        Self {
            locks: RwLock::new(HashMap::default()),
            gc_locks: Mutex::new(HashMap::default()),
        }
    }

    /// Takes a shared lock on a given [`Unit`]
    /// This prevents other Cargo instances from compiling (writing) to
    /// this build unit.
    ///
    /// This function returns a [`LockKey`] which can be used to
    /// upgrade/unlock the lock.
    #[instrument(skip_all, fields(key))]
    pub fn lock_shared(
        &self,
        build_runner: &BuildRunner<'_, '_>,
        unit: &Unit,
    ) -> CargoResult<LockKey> {
        let key = LockKey::from_unit(build_runner, unit);
        tracing::Span::current().record("key", key.0.to_str());

        if build_runner.bcx.gctx.cli_unstable().build_dir_new_layout
            && !crate::workspace::features::is_fork_defaults_opt_out()
        {
            // Dirty units release their write lock while queued. Keep GC away
            // until all jobs finish, using a lock outside the removable unit.
            let pkg_dir = key.0.parent().unwrap().parent().unwrap().to_path_buf();
            let mut gc_locks = self.gc_locks.lock().unwrap();
            if let Entry::Vacant(entry) = gc_locks.entry(pkg_dir) {
                let fs = Filesystem::new(entry.key().clone());
                let lock = open_current_lock(|| {
                    fs.open_ro_shared_create(
                        GC_LOCK_FILE,
                        build_runner.bcx.gctx,
                        "package build directory",
                    )
                })?;
                entry.insert(lock);
            }
        }

        let mut locks = self.locks.write().unwrap();
        if let Some(lock) = locks.get_mut(&key) {
            flock::lock_shared(lock.file())?;
        } else {
            let fs = Filesystem::new(key.0.clone());
            let lock_msg = format!(
                "{} ({})",
                unit.pkg.name(),
                build_runner.files().unit_hash(unit)
            );
            let lock = fs.open_ro_shared_create(&key.0, build_runner.bcx.gctx, &lock_msg)?;
            locks.insert(key.clone(), lock);
        }

        Ok(key)
    }

    pub fn release_gc_locks(&self) {
        self.gc_locks.lock().unwrap().clear();
    }

    #[instrument(skip(self))]
    pub fn lock(&self, key: &LockKey) -> CargoResult<()> {
        let locks = self.locks.read().unwrap();
        if let Some(lock) = locks.get(&key) {
            flock::lock_exclusive(lock.file())?;
        } else {
            bail!("lock was not found in lock manager: {key}");
        }

        Ok(())
    }

    /// Upgrades an existing exclusive lock into a shared lock.
    #[instrument(skip(self))]
    pub fn downgrade_to_shared(&self, key: &LockKey) -> CargoResult<()> {
        let locks = self.locks.read().unwrap();
        let Some(lock) = locks.get(key) else {
            bail!("lock was not found in lock manager: {key}");
        };
        flock::lock_shared(lock.file())?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub fn unlock(&self, key: &LockKey) -> CargoResult<()> {
        let locks = self.locks.read().unwrap();
        if let Some(lock) = locks.get(key) {
            flock::unlock(lock.file())?;
        };

        Ok(())
    }
}

/// Takes the lock `open` returns until it is the lock of the file now at its
/// path. GC removes the lock file of an empty package, possibly while a builder
/// waits on it. A lock on that unlinked file guards nothing, so the builder
/// locks the new file instead.
fn open_current_lock(mut open: impl FnMut() -> CargoResult<FileLock>) -> CargoResult<FileLock> {
    loop {
        let lock = open()?;
        if is_current_lock(lock.file(), lock.path()) {
            return Ok(lock);
        }
        tracing::debug!("{} was replaced while locking it", lock.path().display());
    }
}

/// Whether `path` still names the open `file`. Missing paths also return false.
#[cfg(unix)]
pub(super) fn is_current_lock(file: &std::fs::File, path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(locked), Ok(current)) => locked.dev() == current.dev() && locked.ino() == current.ino(),
        _ => false,
    }
}

/// Non-Unix platforms retain the existing behavior without an identity check.
#[cfg(not(unix))]
pub(super) fn is_current_lock(_file: &std::fs::File, _path: &std::path::Path) -> bool {
    true
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct LockKey(PathBuf);

impl LockKey {
    fn from_unit(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> Self {
        Self(build_runner.files().build_unit_lock(unit))
    }
}

impl Display for LockKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::GlobalContext;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn lock_is_retaken_when_its_file_is_replaced() {
        let tmp = std::env::temp_dir().join(format!("locking-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let gctx = GlobalContext::default().unwrap();
        let fs = Filesystem::new(tmp.clone());
        let lock_path = tmp.join(GC_LOCK_FILE);
        let mut opened: Vec<()> = Vec::new();
        let lock = open_current_lock(|| {
            let lock = fs.open_ro_shared_create(GC_LOCK_FILE, &gctx, "test")?;
            opened.push(());
            // GC drops an empty package while the builder waits for its lock.
            if opened.len() == 1 {
                std::fs::remove_file(&lock_path)?;
                std::fs::remove_dir(&tmp)?;
            } else if opened.len() == 2 {
                std::fs::remove_file(&lock_path)?;
            }
            Ok(lock)
        })
        .unwrap();
        assert_eq!(opened.len(), 3);
        assert!(is_current_lock(lock.file(), lock.path()));
        let current = std::fs::metadata(&lock_path).unwrap();
        let held = lock.file().metadata().unwrap();
        assert_eq!((held.dev(), held.ino()), (current.dev(), current.ino()));
        drop(lock);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn lock_of_an_unlinked_file_is_not_current() {
        let tmp = std::env::temp_dir().join(format!("locking-unlinked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let gctx = GlobalContext::default().unwrap();
        let fs = Filesystem::new(tmp.clone());
        let lock = fs
            .open_ro_shared_create(GC_LOCK_FILE, &gctx, "test")
            .unwrap();
        assert!(is_current_lock(lock.file(), lock.path()));
        std::fs::remove_file(tmp.join(GC_LOCK_FILE)).unwrap();
        assert!(!is_current_lock(lock.file(), lock.path()));
        std::fs::write(tmp.join(GC_LOCK_FILE), "").unwrap();
        assert!(!is_current_lock(lock.file(), lock.path()));
        drop(lock);
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
