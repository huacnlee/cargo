//! Automatic garbage collection of the build-dir.
//!
//! Every distinct set of features, profile settings, `RUSTFLAGS` or compiler
//! version produces new build units (see `compute_metadata`) instead of
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
//!    * incremental session dirs (`incremental/<crate>-<id>/`) likewise, and
//!      all but the [`KEEP_INCREMENTAL`] most recent of each crate (every
//!      checkout, feature set and profile gets its own),
//!    * leftovers from the old layout (`deps/`, `.fingerprint/`,
//!      `build/<pkg>-<hash>/`),
//!
//!    then keeps removing the least recently used ones while the build-dir is
//!    larger than `build.gc.max-size`.
//!
//! The build-dir shared by all projects for registry and git dependencies
//! (see [`Workspace::shared_deps_dir`]) is collected the same way, after the
//! build-dir of the workspace, with its own limits: units unused for
//! `build.gc.shared-max-unused` (7 days by default) are removed, then the least
//! recently used ones while it is larger than `build.gc.shared-max-size`
//! (50 GiB by default). Projects use it at different times, so a day without
//! a build of one project must not throw away its dependencies.
//!
//! In the same pass, the build-dirs of other projects in
//! `{cargo-cache-home}/targets/` that no build has used for
//! `build.gc.shared-max-unused` are removed whole: nothing else would ever
//! collect a project that is not built again (a deleted checkout, a repository
//! you stopped working on), and its dependencies are gone from the shared
//! build-dir by then anyway.
//!
//! Units of the current build are never removed, nor are units another Cargo
//! holds a lock on (`-Zfine-grain-locking` keeps a shared lock on every unit
//! a build uses), nor incremental dirs a running rustc has locked.
//! Removing anything else is always safe: at worst it is rebuilt. Only the
//! new build-dir layout is handled.
//!
//! [`Workspace::shared_deps_dir`]: crate::workspace::Workspace::shared_deps_dir

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cargo_util::paths;
use filetime::FileTime;
use tracing::debug;

use crate::compiler::BuildRunner;
use crate::compiler::shared_units::{self, OUT_LINKS_DIR};
use crate::util::data_structures::HashSet;
use crate::util::flock;
use crate::util::time_span::maybe_parse_time_span;
use crate::util::{CargoResult, HumanBytes};
use crate::workspace::features;
use crate::workspace::gc::parse_human_size;

/// Marker touched in every build unit used by a build.
const LAST_USE_FILE: &str = ".cargo-last-use";
/// What a path package's unit is built as apart from its files and those of
/// its dependencies, see [`collect_superseded`].
const SLOT_FILE: &str = ".cargo-slot";
/// The checkouts that used a path package's unit, one per line.
const USERS_FILE: &str = ".cargo-users";
/// Marker recording when a build-dir was last collected.
const LAST_GC_FILE: &str = ".cargo-gc-last";
/// `<unix seconds> <bytes>` lines: what each recent build used.
const BUILD_SIZES_FILE: &str = ".cargo-gc-sizes";

const DEFAULT_MAX_UNUSED: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_SHARED_MAX_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_SHARED_MAX_SIZE: u64 = 50 * 1024 * 1024 * 1024;
const DEFAULT_FREQUENCY: Duration = Duration::from_secs(60 * 60);
/// Incremental dirs kept per crate name, the most recently used first.
const KEEP_INCREMENTAL: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
enum MaxSize {
    None,
    Bytes(u64),
    /// Twice the largest build within `max-unused`: bounded by the project's
    /// own size, with room for one more configuration (e.g. `test` or another
    /// worktree), and not shrunk by a small build such as a one-crate check.
    Auto,
}

#[derive(Debug, PartialEq)]
struct GcSettings {
    max_unused: Duration,
    max_size: MaxSize,
    /// Age limit for the shared build-dir and for whole idle projects.
    shared_max_unused: Duration,
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
            shared_max_unused: DEFAULT_SHARED_MAX_UNUSED,
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
        let shared_max_unused = match cfg.shared_max_unused.as_deref() {
            None => DEFAULT_SHARED_MAX_UNUSED,
            Some(s) => span("shared-max-unused", s)?,
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
            shared_max_unused,
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
    if !gctx.cli_unstable().build_dir_new_layout || features::is_fork_defaults_opt_out() {
        return Ok(());
    }
    let Some(settings) = GcSettings::from_config(build_runner)? else {
        return Ok(());
    };

    let now = FileTime::now();
    let files = build_runner.files();
    let mut live = HashSet::default();
    for unit in build_runner.bcx.unit_graph.keys() {
        let dir = files.build_unit_dir(unit);
        if live.insert(dir.clone()) && dir.is_dir() {
            touch(&dir.join(LAST_USE_FILE), now)?;
        }
    }

    let root = build_runner.bcx.ws.build_dir().into_path_unlocked();
    collect_superseded(build_runner, &live)?;
    let live_bytes = live
        .iter()
        .filter(|d| d.starts_with(&root))
        .map(|d| cargo_util::du(d, &[]).unwrap_or(0))
        .sum();
    let largest_build = record_build_size(&root, live_bytes, settings.max_unused)?;

    let Some(frequency) = settings.frequency else {
        return Ok(());
    };
    let ws = build_runner.bcx.ws;
    let mut roots = vec![(root.clone(), settings.max_unused, settings.max_size)];
    let shared = files
        .layouts()
        .any(|l| l.shared_build_dir().is_some())
        .then(|| ws.shared_deps_dir())
        .flatten();
    if let Some(shared) = &shared {
        roots.push((
            shared.clone(),
            settings.shared_max_unused,
            settings.shared_max_size,
        ));
    }

    let mut removed = Removed::default();
    for (dir, max_unused, max_size) in roots {
        let marker = dir.join(LAST_GC_FILE);
        if age(mtime(&marker)) < frequency {
            continue;
        }
        collect(
            &dir,
            &live,
            largest_build,
            FileTime::from_system_time(build_start),
            &GcSettings {
                max_unused,
                max_size,
                ..settings
            },
            &mut removed,
        )?;
        touch(&marker, now)?;
        if Some(&dir) == shared.as_ref()
            && let Some(targets) = dir.parent()
        {
            collect_idle_projects(
                targets,
                &[root.as_path(), dir.as_path()],
                settings.shared_max_unused,
                &mut removed,
            )?;
        }
    }

    if removed.count > 0 || removed.projects > 0 {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut what = Vec::new();
        if removed.count > 0 {
            what.push(format!(
                "{} stale build unit{}",
                removed.count,
                plural(removed.count)
            ));
        }
        if removed.projects > 0 {
            what.push(format!(
                "{} idle project{}",
                removed.projects,
                plural(removed.projects)
            ));
        }
        gctx.shell().status(
            "Cleaned",
            format!(
                "{}, {:.1} freed (build.gc)",
                what.join(" and "),
                HumanBytes(removed.bytes)
            ),
        )?;
    }
    Ok(())
}

/// Removes the units the units of path packages in this build replace.
///
/// Units of path packages are keyed by the content of their files (see
/// `shared_units`), and every unit that depends on one by its hash, so an
/// edit puts the edited crate and everything that depends on it in new
/// directories. Each unit records its slot (its `-C metadata` and flags, which
/// edits do not change) and the checkouts that used it, so this removes the other units
/// in the same slot that no other checkout has used. Those of other worktrees
/// stay until they move on themselves, or the periodic collection ages them
/// out.
fn collect_superseded(
    build_runner: &BuildRunner<'_, '_>,
    live: &HashSet<PathBuf>,
) -> CargoResult<()> {
    let ws = build_runner.bcx.ws;
    let Some(checkout) = ws.shared_build_dir_root().map(shared_units::checkout_key) else {
        return Ok(());
    };
    let files = build_runner.files();
    let mut removed = Removed::default();
    for unit in build_runner.bcx.unit_graph.keys() {
        if unit.is_std || !unit.pkg.package_id().source_id().is_path() {
            continue;
        }
        let dir = files.build_unit_dir(unit);
        if !dir.is_dir() {
            continue;
        }
        // `RUSTFLAGS` change `unit_id` but not `-C metadata`: keep a unit per
        // set of flags, as Cargo does for other packages.
        let slot = crate::util::to_hex(crate::util::hash_u64((
            files.metadata(unit).c_metadata().to_string(),
            &unit.rustflags,
            &unit.rustdocflags,
            build_runner.bcx.extra_args_for(unit),
        )));
        if fs::read_to_string(dir.join(SLOT_FILE)).ok().as_deref() != Some(&slot) {
            paths::write(dir.join(SLOT_FILE), &slot)?;
        }
        let users_file = dir.join(USERS_FILE);
        let users = fs::read_to_string(&users_file).unwrap_or_default();
        if !users.lines().any(|u| u == checkout) {
            paths::append(&users_file, format!("{checkout}\n").as_bytes())?;
        }
        let Some(pkg_dir) = dir.parent() else {
            continue;
        };
        for sibling in read_dirs(pkg_dir)? {
            if live.contains(&sibling)
                || fs::read_to_string(sibling.join(SLOT_FILE)).ok().as_deref() != Some(&slot)
            {
                continue;
            }
            let users = fs::read_to_string(sibling.join(USERS_FILE)).unwrap_or_default();
            if users.lines().all(|u| u == checkout) {
                remove(&sibling, &mut removed)?;
            }
        }
    }
    debug!(
        "build.gc removed {} superseded units, {} bytes",
        removed.count, removed.bytes
    );
    Ok(())
}

#[derive(Default)]
struct Removed {
    count: usize,
    /// Whole project build-dirs, see [`collect_idle_projects`].
    projects: usize,
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
    largest_build: u64,
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
        let mut incremental = read_dirs(&profile.join("incremental"))?
            .into_iter()
            .map(|dir| {
                let last_use = newest_child_mtime(&dir);
                (dir, last_use)
            })
            .collect::<Vec<_>>();
        incremental.sort_by(|(a, a_use), (b, b_use)| {
            crate_name(a).cmp(crate_name(b)).then(b_use.cmp(a_use))
        });
        for (i, (dir, last_use)) in incremental.iter().enumerate() {
            let rank = incremental[..i]
                .iter()
                .filter(|(d, _)| crate_name(d) == crate_name(dir))
                .count();
            if rank >= KEEP_INCREMENTAL {
                remove(dir, removed)?;
            } else if *last_use < build_start {
                candidates.push(Candidate::new(dir.clone(), *last_use));
            }
        }
        remove_dangling_out_links(profile);
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
        MaxSize::Auto => Some(largest_build * 2),
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
            if total <= max_size {
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

/// Removes the build-dirs in `targets` (`{cargo-cache-home}/targets/`) that
/// no build has used for `max_unused`, except `keep`. Only directories Cargo
/// created (with a `CACHEDIR.TAG`) are considered, and a project a running
/// build has locked is skipped.
fn collect_idle_projects(
    targets: &Path,
    keep: &[&Path],
    max_unused: Duration,
    removed: &mut Removed,
) -> CargoResult<()> {
    for dir in read_dirs(targets)? {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Left over from a removal that was interrupted.
        if name.starts_with('.') && name.contains(REMOVING_SUFFIX) {
            let _ = paths::remove_dir_all(&dir);
            continue;
        }
        if name.starts_with('.')
            || keep.iter().any(|k| *k == dir)
            || !dir.join("CACHEDIR.TAG").is_file()
            || age(last_project_use(&dir)?) <= max_unused
        {
            continue;
        }
        remove_project(targets, &dir, removed)?;
    }
    Ok(())
}

const REMOVING_SUFFIX: &str = ".removing-";

/// When a build last used a project's build-dir. Every build rewrites
/// `.cargo-gc-sizes`; older build-dirs only have `.cargo-gc-last` and the
/// directories a build creates entries in.
fn last_project_use(dir: &Path) -> CargoResult<FileTime> {
    let mut last = newest_child_mtime(dir);
    for profile in profile_dirs(dir)? {
        last = last.max(newest_child_mtime(&profile));
    }
    Ok(last)
}

/// Removes a whole project build-dir unless a build holds one of its
/// `<profile>/.cargo-build-lock`. It is first renamed away so that a build
/// starting meanwhile gets a fresh, empty directory.
fn remove_project(targets: &Path, dir: &Path, removed: &mut Removed) -> CargoResult<()> {
    let mut _locks = Vec::new();
    for profile in profile_dirs(dir)? {
        let Ok(file) = fs::File::open(profile.join(".cargo-build-lock")) else {
            continue;
        };
        match flock::try_lock_exclusive(&file) {
            Ok(()) => _locks.push(file),
            Err(fs::TryLockError::WouldBlock) => {
                debug!("build.gc skipping {}: in use", dir.display());
                return Ok(());
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    let size = cargo_util::du(dir, &[]).unwrap_or(0);
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let trash = targets.join(format!(".{name}{REMOVING_SUFFIX}{}", std::process::id()));
    debug!("build.gc removing idle project {}", dir.display());
    // Windows cannot rename a directory with open files, the lock files here.
    let renamed = fs::rename(dir, &trash).or_else(|_| {
        _locks.clear();
        fs::rename(dir, &trash)
    });
    if let Err(e) = renamed {
        debug!("build.gc skipping {}: {e}", dir.display());
        return Ok(());
    }
    drop(_locks);
    paths::remove_dir_all(&trash)?;
    removed.projects += 1;
    removed.bytes += size;
    Ok(())
}

/// Records what this build used and returns the largest build recorded
/// within `window`, this one included.
fn record_build_size(root: &Path, bytes: u64, window: Duration) -> CargoResult<u64> {
    let path = root.join(BUILD_SIZES_FILE);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let mut sizes = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (time, bytes) = line.split_once(' ')?;
            Some((time.parse::<u64>().ok()?, bytes.parse::<u64>().ok()?))
        })
        .filter(|(time, _)| now.saturating_sub(*time) <= window.as_secs())
        .collect::<Vec<_>>();
    sizes.push((now, bytes));
    let contents = sizes
        .iter()
        .map(|(time, bytes)| format!("{time} {bytes}\n"))
        .collect::<String>();
    paths::write(&path, contents)?;
    Ok(sizes.iter().map(|(_, bytes)| *bytes).max().unwrap_or(0))
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
/// Cargo holds the lock of, and incremental dirs whose session a running
/// rustc has locked (`s-*.lock`).
fn remove(path: &Path, removed: &mut Removed) -> CargoResult<u64> {
    let mut lock_files = vec![path.join(".lock")];
    if let Ok(entries) = fs::read_dir(path) {
        lock_files.extend(
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "lock")),
        );
    }
    // Held until the directory is gone.
    let mut _locks = Vec::new();
    for lock_file in lock_files {
        let Ok(file) = fs::File::open(&lock_file) else {
            continue;
        };
        match flock::try_lock_exclusive(&file) {
            Ok(()) => _locks.push(file),
            Err(fs::TryLockError::WouldBlock) => {
                debug!("build.gc skipping {}: in use", path.display());
                return Ok(0);
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    let size = cargo_util::du(path, &[]).unwrap_or(0);
    debug!("build.gc removing {}", path.display());
    paths::remove_dir_all(path)?;
    removed.count += 1;
    removed.bytes += size;
    Ok(size)
}

/// Removes `--out-dir` symlinks whose unit directory is gone, and their
/// lock files.
fn remove_dangling_out_links(profile: &Path) {
    let Ok(entries) = fs::read_dir(profile.join(OUT_LINKS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let link = entry.path();
        if link.is_symlink() && !link.exists() {
            let _ = fs::remove_file(&link);
            let _ = fs::remove_file(link.with_extension("lock"));
        }
    }
}

/// The crate name of an `incremental/<crate>-<id>` directory.
fn crate_name(dir: &Path) -> &str {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.rsplit_once('-').map_or(name, |(krate, _)| krate)
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

    #[test]
    fn idle_projects() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-idle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let old = FileTime::from_unix_time(FileTime::now().unix_seconds() - 8 * 24 * 3600, 0);
        let project = |name: &str, time: FileTime| {
            let dir = tmp.join(name);
            fs::create_dir_all(dir.join("debug/build/foo/0123456789abcdef")).unwrap();
            for file in ["CACHEDIR.TAG", "debug/.cargo-build-lock"] {
                fs::write(dir.join(file), "").unwrap();
                filetime::set_file_mtime(dir.join(file), time).unwrap();
            }
            for d in [
                "debug/build/foo/0123456789abcdef",
                "debug/build/foo",
                "debug/build",
                "debug",
                "",
            ] {
                filetime::set_file_mtime(dir.join(d), time).unwrap();
            }
            dir
        };
        let idle = project("idle-1", old);
        let recent = project("recent-1", FileTime::now());
        let current = project("current-1", old);
        let locked = project("locked-1", old);
        let not_cargo = project("other", old);
        fs::remove_file(not_cargo.join("CACHEDIR.TAG")).unwrap();
        filetime::set_file_mtime(&not_cargo, old).unwrap();
        let interrupted = tmp.join(".gone-1.removing-1");
        fs::create_dir_all(&interrupted).unwrap();
        let lock = fs::File::open(locked.join("debug/.cargo-build-lock")).unwrap();
        flock::lock_shared(&lock).unwrap();

        let mut removed = Removed::default();
        let week = Duration::from_secs(7 * 24 * 3600);
        collect_idle_projects(&tmp, &[current.as_path()], week, &mut removed).unwrap();
        drop(lock);
        let left = |d: &Path| d.exists();
        let result = (
            left(&idle),
            left(&recent),
            left(&current),
            left(&locked),
            left(&not_cargo),
            left(&interrupted),
            removed.projects,
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (false, true, true, true, true, false, 1));
    }

    #[test]
    fn largest_recent_build() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-sizes-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        // An entry older than the window no longer counts.
        fs::write(tmp.join(BUILD_SIZES_FILE), "1 999999\n").unwrap();
        assert_eq!(record_build_size(&tmp, 800, day).unwrap(), 800);
        assert_eq!(record_build_size(&tmp, 100, day).unwrap(), 800);
        assert_eq!(record_build_size(&tmp, 900, day).unwrap(), 900);
        let lines = fs::read_to_string(tmp.join(BUILD_SIZES_FILE)).unwrap();
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(lines.lines().count(), 3);
    }
}
