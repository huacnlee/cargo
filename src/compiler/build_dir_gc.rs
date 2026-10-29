//! Automatic garbage collection of the build-dir.
//!
//! Every distinct set of features, profile settings, `RUSTFLAGS` or compiler
//! version produces new build units (see [`compute_metadata`]) instead of
//! overwriting old ones, so without cleanup the build-dir only ever grows.
//!
//! After each successful build this module:
//!
//! 1. Marks every build unit of the current unit graph as used by touching
//!    `<build-unit>/.cargo-last-use`.
//! 2. At most once per `build.gc.frequency`, collects the whole build-dir of
//!    the workspace (every `[<target>]/<profile>/` in it) as one pool. Oldest
//!    first, it removes
//!    * build units (`build/<pkg>/<hash>/`) not used for `build.gc.max-unused`,
//!    * incremental session dirs (`incremental/<crate>-<id>/`) likewise,
//!    * leftovers from the old layout (`deps/`, `.fingerprint/`,
//!      `build/<pkg>-<hash>/`),
//!
//!    then keeps removing the least recently used ones while the build-dir is
//!    larger than `build.gc.max-size`.
//!
//! The build-dir shared by all projects for registry and git dependencies
//! (see [`Workspace::shared_deps_dir`]) is collected the same way, after the
//! build-dir of the workspace, with its own size limit,
//! `build.gc.shared-max-size` (50 GiB by default).
//!
//! Units of the current build are never removed, nor are units another Cargo
//! holds a lock on (`-Zfine-grain-locking` keeps a shared lock on every unit
//! a build uses), nor, when trimming to size, anything used in the last hour.
//! Removing anything else is always safe: at worst it is rebuilt. Only the
//! new build-dir layout is handled.
//!
//! [`compute_metadata`]: super::build_runner::compilation_files
//! [`Workspace::shared_deps_dir`]: crate::workspace::Workspace::shared_deps_dir

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cargo_util::paths;
use filetime::FileTime;
use tracing::debug;

use crate::compiler::BuildRunner;
use crate::util::time_span::maybe_parse_time_span;
use crate::util::{CargoResult, HumanBytes};
use crate::workspace::gc::parse_human_size;

/// Marker touched in every build unit used by a build.
const LAST_USE_FILE: &str = ".cargo-last-use";
/// Marker recording when a build-dir was last collected.
const LAST_GC_FILE: &str = ".cargo-gc-last";

const DEFAULT_MAX_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_MAX_SIZE: u64 = 10 * 1024 * 1024 * 1024;
const DEFAULT_SHARED_MAX_SIZE: u64 = 50 * 1024 * 1024 * 1024;
const DEFAULT_FREQUENCY: Duration = Duration::from_secs(60 * 60);
/// Trimming to `max-size` never touches what was used this recently, so
/// concurrent builds sharing the build-dir do not evict each other.
const MIN_AGE_FOR_SIZE_LIMIT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, PartialEq)]
enum MaxSize {
    None,
    Bytes(u64),
    /// [`DEFAULT_MAX_SIZE`], raised to twice what the current build uses so
    /// huge projects keep room for one more configuration (e.g. `test`).
    Auto,
}

#[derive(Debug, PartialEq)]
struct GcSettings {
    max_unused: Duration,
    max_size: MaxSize,
    /// Limit for the shared build-dir; `Auto` is not used there.
    shared_max_size: MaxSize,
    /// `None` means never collect.
    frequency: Option<Duration>,
}

impl Default for GcSettings {
    fn default() -> Self {
        GcSettings {
            max_unused: DEFAULT_MAX_UNUSED,
            max_size: MaxSize::Auto,
            shared_max_size: MaxSize::Bytes(DEFAULT_SHARED_MAX_SIZE),
            frequency: Some(DEFAULT_FREQUENCY),
        }
    }
}

impl GcSettings {
    fn from_config(build_runner: &BuildRunner<'_, '_>) -> CargoResult<Option<GcSettings>> {
        let Some(cfg) = &build_runner.bcx.gctx.build_config()?.gc else {
            return Ok(Some(GcSettings::default()));
        };
        if cfg.enabled == Some(false) {
            return Ok(None);
        }
        let span = |key: &str, value: &str| {
            maybe_parse_time_span(value).ok_or_else(|| {
                anyhow::format_err!(
                    "config option `build.gc.{key}` expected a value of the form \
                     \"N seconds/minutes/days/weeks/months\", got: {value:?}"
                )
            })
        };
        let max_unused = match cfg.max_unused.as_deref() {
            None => DEFAULT_MAX_UNUSED,
            Some(s) => span("max-unused", s)?,
        };
        let max_size = match cfg.max_size.as_deref() {
            None | Some("auto") => MaxSize::Auto,
            Some("none") => MaxSize::None,
            Some(s) => MaxSize::Bytes(parse_human_size(s)?),
        };
        let shared_max_size = match cfg.shared_max_size.as_deref() {
            None => MaxSize::Bytes(DEFAULT_SHARED_MAX_SIZE),
            Some("none") => MaxSize::None,
            Some(s) => MaxSize::Bytes(parse_human_size(s)?),
        };
        let frequency = match cfg.frequency.as_deref() {
            None => Some(DEFAULT_FREQUENCY),
            Some("never") => None,
            Some("always") => Some(Duration::ZERO),
            Some(s) => Some(span("frequency", s)?),
        };
        Ok(Some(GcSettings {
            max_unused,
            max_size,
            shared_max_size,
            frequency,
        }))
    }
}

/// Runs after a successful build. Never fails the build: problems are
/// reported as warnings.
pub fn run(build_runner: &BuildRunner<'_, '_>, build_start: SystemTime) {
    if let Err(e) = run_inner(build_runner, build_start) {
        let _ = build_runner
            .bcx
            .gctx
            .shell()
            .warn(format!("failed to clean the build directory: {e:#}"));
    }
}

fn run_inner(build_runner: &BuildRunner<'_, '_>, build_start: SystemTime) -> CargoResult<()> {
    let gctx = build_runner.bcx.gctx;
    // The old layout mixes the files of all units in shared directories.
    if !gctx.cli_unstable().build_dir_new_layout {
        return Ok(());
    }
    let Some(settings) = GcSettings::from_config(build_runner)? else {
        return Ok(());
    };

    let now = FileTime::now();
    let files = build_runner.files();
    let mut live = HashSet::new();
    for unit in build_runner.bcx.unit_graph.keys() {
        let dir = files.build_unit_dir(unit);
        if live.insert(dir.clone()) && dir.is_dir() {
            touch(&dir.join(LAST_USE_FILE), now)?;
        }
    }

    let Some(frequency) = settings.frequency else {
        return Ok(());
    };
    let ws = build_runner.bcx.ws;
    let mut roots = vec![(ws.build_dir().into_path_unlocked(), settings.max_size)];
    if files.layouts().any(|l| l.shared_build_dir().is_some()) {
        if let Some(shared) = ws.shared_deps_dir() {
            roots.push((shared, settings.shared_max_size));
        }
    }

    let mut removed = Removed::default();
    for (root, max_size) in roots {
        let marker = root.join(LAST_GC_FILE);
        if age(mtime(&marker)) < frequency {
            continue;
        }
        collect(
            &root,
            &live,
            FileTime::from_system_time(build_start),
            &GcSettings {
                max_size,
                ..settings
            },
            &mut removed,
        )?;
        touch(&marker, now)?;
    }

    if removed.count > 0 {
        gctx.shell().status(
            "Cleaned",
            format!(
                "{} stale build unit{}, {} freed (build.gc)",
                removed.count,
                if removed.count == 1 { "" } else { "s" },
                format!("{:.1}", HumanBytes(removed.bytes)),
            ),
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct Removed {
    count: usize,
    bytes: u64,
}

/// Something the collector may remove.
struct Candidate {
    path: PathBuf,
    last_use: FileTime,
}

impl Candidate {
    fn new(path: PathBuf, last_use: FileTime) -> Candidate {
        Candidate { path, last_use }
    }
}

/// Collects one workspace's build-dir, all its targets and profiles together.
fn collect(
    root: &Path,
    live: &HashSet<PathBuf>,
    build_start: FileTime,
    settings: &GcSettings,
    removed: &mut Removed,
) -> CargoResult<()> {
    let profiles = profile_dirs(root)?;
    let mut candidates = Vec::new();
    for profile in &profiles {
        for pkg in read_dirs(&profile.join("build"))? {
            if is_legacy_unit_dir(&pkg) {
                candidates.push(Candidate::new(pkg.clone(), mtime(&pkg)));
                continue;
            }
            for unit in read_dirs(&pkg)? {
                if live.contains(&unit) {
                    continue;
                }
                let last_use =
                    mtime(&unit.join(LAST_USE_FILE)).max(mtime(&unit.join("fingerprint")));
                candidates.push(Candidate::new(unit, last_use));
            }
        }
        // rustc names these after the crate's `StableCrateId`, which Cargo does
        // not know, so they are aged by when rustc last wrote a session into
        // them. Anything written during this build belongs to it.
        for dir in read_dirs(&profile.join("incremental"))? {
            let last_use = newest_child_mtime(&dir);
            if last_use < build_start {
                candidates.push(Candidate::new(dir, last_use));
            }
        }
        for name in ["deps", ".fingerprint"] {
            let dir = profile.join(name);
            if dir.is_dir() {
                candidates.push(Candidate::new(dir.clone(), mtime(&dir)));
            }
        }
    }

    candidates.sort_by_key(|c| c.last_use);
    let mut kept = Vec::with_capacity(candidates.len());
    for c in candidates {
        if age(c.last_use) > settings.max_unused {
            remove(&c.path, removed)?;
        } else {
            kept.push(c);
        }
    }

    let max_size = match settings.max_size {
        MaxSize::None => None,
        MaxSize::Bytes(n) => Some(n),
        MaxSize::Auto => {
            let live_bytes: u64 = live
                .iter()
                .filter(|d| d.starts_with(root))
                .map(|d| cargo_util::du(d, &[]).unwrap_or(0))
                .sum();
            Some(DEFAULT_MAX_SIZE.max(live_bytes * 2))
        }
    };
    if let Some(max_size) = max_size {
        let mut total = 0;
        for profile in &profiles {
            for name in ["build", "incremental", "deps", ".fingerprint"] {
                let dir = profile.join(name);
                if dir.is_dir() {
                    total += cargo_util::du(&dir, &[])?;
                }
            }
        }
        debug!(
            "build-dir {} is {total} bytes (limit {max_size})",
            root.display()
        );
        for c in kept {
            if total <= max_size || age(c.last_use) < MIN_AGE_FOR_SIZE_LIMIT {
                break;
            }
            total = total.saturating_sub(remove(&c.path, removed)?);
        }
    }

    // Drop package dirs left without any unit.
    for profile in &profiles {
        for pkg in read_dirs(&profile.join("build"))? {
            if fs::read_dir(&pkg).is_ok_and(|mut d| d.next().is_none()) {
                let _ = fs::remove_dir(&pkg);
            }
        }
    }
    Ok(())
}

/// The `[<target>]/<profile>/` directories of a build-dir.
fn profile_dirs(root: &Path) -> CargoResult<Vec<PathBuf>> {
    let is_profile = |dir: &Path| {
        ["build", "incremental", "deps", ".fingerprint"]
            .iter()
            .any(|name| dir.join(name).is_dir())
    };
    let mut profiles = Vec::new();
    for dir in read_dirs(root)? {
        if is_profile(&dir) {
            profiles.push(dir);
        } else {
            profiles.extend(read_dirs(&dir)?.into_iter().filter(|d| is_profile(d)));
        }
    }
    Ok(profiles)
}

/// Removes a directory, returning the bytes freed. Skips build units another
/// Cargo holds the lock of.
fn remove(path: &Path, removed: &mut Removed) -> CargoResult<u64> {
    // Held until the directory is gone.
    let _lock = match fs::File::open(path.join(".lock")) {
        Ok(file) => match file.try_lock() {
            Ok(()) => Some(file),
            Err(fs::TryLockError::WouldBlock) => {
                debug!("build.gc skipping {}: in use", path.display());
                return Ok(0);
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        },
        Err(_) => None,
    };
    let size = cargo_util::du(path, &[]).unwrap_or(0);
    debug!("build.gc removing {}", path.display());
    paths::remove_dir_all(path)?;
    removed.count += 1;
    removed.bytes += size;
    Ok(size)
}

/// Whether `dir` is an old-layout `build/<pkg>-<16 hex digits>` directory.
fn is_legacy_unit_dir(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match name.rsplit_once('-') {
        Some((_, hash)) => hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

fn read_dirs(dir: &Path) -> CargoResult<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow::Error::from(e).context(format!("reading {}", dir.display()))),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// Modification time, or the epoch if the path does not exist.
fn mtime(path: &Path) -> FileTime {
    fs::metadata(path)
        .map(|m| FileTime::from_last_modification_time(&m))
        .unwrap_or_else(|_| FileTime::zero())
}

fn newest_child_mtime(dir: &Path) -> FileTime {
    let mut newest = mtime(dir);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            newest = newest.max(mtime(&entry.path()));
        }
    }
    newest
}

fn age(time: FileTime) -> Duration {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(time.unix_seconds().max(0) as u64);
    SystemTime::now()
        .duration_since(then)
        .unwrap_or(Duration::ZERO)
}

fn touch(path: &Path, time: FileTime) -> CargoResult<()> {
    if !path.exists() {
        paths::write(path, b"")?;
    }
    filetime::set_file_mtime(path, time)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_unit_dirs() {
        assert!(is_legacy_unit_dir(Path::new(
            "build/nsh-core-0123456789abcdef"
        )));
        assert!(is_legacy_unit_dir(Path::new(
            "build/serde-abcdef0123456789"
        )));
        assert!(!is_legacy_unit_dir(Path::new("build/nsh-core")));
        assert!(!is_legacy_unit_dir(Path::new("build/serde")));
        assert!(!is_legacy_unit_dir(Path::new("build/foo-0123")));
    }

    #[test]
    fn profile_dirs_of_a_build_dir() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-{}", std::process::id()));
        for dir in [
            "debug/build",
            "release/incremental",
            "aarch64-apple-ios/debug/build",
            "tmp",
        ] {
            fs::create_dir_all(tmp.join(dir)).unwrap();
        }
        let mut found = profile_dirs(&tmp).unwrap();
        found.sort();
        let _ = fs::remove_dir_all(&tmp);
        let rel: Vec<_> = found
            .iter()
            .map(|p| p.strip_prefix(&tmp).unwrap())
            .collect();
        assert_eq!(
            rel,
            [
                Path::new("aarch64-apple-ios/debug"),
                Path::new("debug"),
                Path::new("release")
            ]
        );
    }
}
