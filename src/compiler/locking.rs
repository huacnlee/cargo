//! This module handles the locking logic during compilation.

use crate::util::flock;
use crate::{
    CargoResult,
    compiler::{BuildRunner, Unit},
    util::{FileLock, Filesystem},
};

use crate::util::data_structures::HashMap;
use anyhow::bail;
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
                let lock = fs.open_ro_shared_create(
                    GC_LOCK_FILE,
                    build_runner.bcx.gctx,
                    "package build directory",
                )?;
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
