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
//!    * entries of `tmp/` (`CARGO_TARGET_TMPDIR`, which tests write to and
//!      nothing cleans) unused for `build.gc.max-unused`,
//!
//!    then keeps removing the least recently used ones while the build-dir is
//!    larger than `build.gc.max-size`.
//! 3. In the same pass, removes the binaries and libraries in the target-dir
//!    (`./target/<profile>/`) that no build unit holds any more: they are
//!    hard links to a unit's outputs, so they cost nothing until the unit is
//!    collected, and then keep its whole size alive. Only files unchanged for
//!    `build.gc.max-unused` are removed; a build links them again.
//!
//! The build-dir shared by all projects for registry and git dependencies
//! (see [`Workspace::shared_deps_dir`]) is collected the same way, after the
//! build-dir of the workspace, with its own limits: units unused for
//! `build.gc.shared-max-unused` (7 days by default) are removed, then the least
//! recently used ones while it is larger than `build.gc.shared-max-size`
//! (50 GiB by default). Projects use it at different times, so a day without
//! a build of one project must not throw away its dependencies.
//!
//! A dependency is rebuilt in a new unit whenever anything below it changes
//! (a `cargo update`, another project's lockfile, a new upstream snapshot), so
//! the shared build-dir collects many variants of the same crate with the same
//! features and flags. Each unit records that variant (`.cargo-variant`); a
//! unit whose variant a newer unit has built since is removed after
//! `build.gc.max-unused` instead of `build.gc.shared-max-unused`. The newest
//! unit of each variant still waits the full week.
//!
//! In the same pass, the build-dirs of other projects in
//! `{cargo-cache-home}/targets/` are collected too, since nothing else would
//! ever collect a project that is not built again (a deleted checkout, a
//! repository you stopped working on, the worktree of a finished task):
//!
//! * a project no build has used for `build.gc.max-unused` loses its
//!   incremental dirs (they only speed up the next edit, and are usually the
//!   larger part of a project) and stale `tmp/` entries; its units stay, so
//!   coming back to it rebuilds nothing,
//! * one no build has used for `build.gc.shared-max-unused` is removed whole;
//!   its dependencies are gone from the shared build-dir by then anyway,
//! * then the least recently built ones are removed whole while all of them
//!   together are larger than `build.gc.projects-max-size` (50 GiB by
//!   default), except those built within the last 6 hours, so that working
//!   on more projects than fit does not rebuild them over and over,
//! * and while the disk has less than 5% free (at most 20 GiB), the least
//!   recently built ones go whatever their age: a full disk fails every
//!   build. `projects-max-size = "none"` turns both off.
//!
//! Directories a Cargo without this build-dir (e.g. the rustup proxy's) left
//! in the target-dir (`./target/<profile>/{deps,incremental,build,.fingerprint}`)
//! are removed once nothing has written to them for an hour: this Cargo never
//! uses them, so nothing else would remove them.
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
use crate::util::flock;
use crate::util::time_span::maybe_parse_time_span;
use crate::util::{CargoResult, HumanBytes};
use crate::workspace::features;
use crate::workspace::gc::parse_human_size;
use cargo_util::collections::HashSet;

/// Marker touched in every build unit used by a build.
const LAST_USE_FILE: &str = ".cargo-last-use";
/// What a path package's unit is built as apart from its files and those of
/// its dependencies, see [`collect_superseded`].
const SLOT_FILE: &str = ".cargo-slot";
/// The checkouts that used a path package's unit, one per line.
const USERS_FILE: &str = ".cargo-users";
/// The crate, target, features, profile and flags of a shared unit: what
/// stays the same when only its dependencies change.
const VARIANT_FILE: &str = ".cargo-variant";
/// Marker recording when a build-dir was last collected.
const LAST_GC_FILE: &str = ".cargo-gc-last";
/// `<unix seconds> <bytes>` lines: what each recent build used.
const BUILD_SIZES_FILE: &str = ".cargo-gc-sizes";
/// The size of a whole project build-dir when it was last measured.
const SIZE_CACHE_FILE: &str = ".cargo-gc-du";

const DEFAULT_MAX_UNUSED: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_SHARED_MAX_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_SHARED_MAX_SIZE: u64 = 50 * 1024 * 1024 * 1024;
const DEFAULT_PROJECTS_MAX_SIZE: u64 = 50 * 1024 * 1024 * 1024;
/// Projects built this recently are kept over `build.gc.projects-max-size`.
const RECENT_PROJECT: Duration = Duration::from_secs(6 * 60 * 60);
/// The most free space a collection keeps on the disk, see [`min_free_space`].
const MAX_MIN_FREE_SPACE: u64 = 20 * 1024 * 1024 * 1024;
/// How long the target-dir directories of another Cargo must be left alone
/// before they are removed, so that a build running with it is not disturbed.
const FOREIGN_OUTPUTS_MAX_UNUSED: Duration = Duration::from_secs(60 * 60);
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
    /// Limit for the build-dirs of all other projects together; `Auto` is not
    /// used there.
    projects_max_size: MaxSize,
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
            projects_max_size: MaxSize::Bytes(DEFAULT_PROJECTS_MAX_SIZE),
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
        let projects_max_size = match cfg.projects_max_size.as_deref() {
            None => MaxSize::Bytes(DEFAULT_PROJECTS_MAX_SIZE),
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
            projects_max_size,
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
    record_variants(build_runner)?;
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
    // (dir, max-unused, max-size, max-unused of a superseded variant)
    let mut roots = vec![(root.clone(), settings.max_unused, settings.max_size, None)];
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
            Some(settings.max_unused),
        ));
    }

    let mut removed = Removed::default();
    for (dir, max_unused, max_size, superseded_max_unused) in roots {
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
            superseded_max_unused,
            &mut removed,
        )?;
        if dir == root {
            let target_dir = ws.target_dir().into_path_unlocked();
            collect_orphan_outputs(&target_dir, settings.max_unused, &mut removed)?;
            if !root.starts_with(&target_dir) {
                collect_foreign_outputs(&target_dir, FOREIGN_OUTPUTS_MAX_UNUSED, &mut removed)?;
            }
        }
        touch(&marker, now)?;
        if Some(&dir) == shared.as_ref()
            && let Some(targets) = dir.parent()
        {
            collect_idle_projects(
                targets,
                &[root.as_path(), dir.as_path()],
                &settings,
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

/// Writes [`VARIANT_FILE`] in every unit of this build that is in the shared
/// build-dir. `-C metadata` cannot serve: it hashes the dependencies too.
fn record_variants(build_runner: &BuildRunner<'_, '_>) -> CargoResult<()> {
    let Some(shared) = build_runner.bcx.ws.shared_deps_dir() else {
        return Ok(());
    };
    let files = build_runner.files();
    for unit in build_runner.bcx.unit_graph.keys() {
        let dir = files.build_unit_dir(unit);
        if !dir.starts_with(&shared) || !dir.is_dir() {
            continue;
        }
        let variant = crate::util::to_hex(crate::util::hash_u64((
            unit.pkg.name().as_str(),
            unit.pkg.package_id().source_id().is_git(),
            &unit.target,
            &unit.mode,
            &unit.kind,
            &unit.features,
            &unit.profile,
            &unit.rustflags,
            build_runner.bcx.extra_args_for(unit),
            &build_runner.bcx.rustc().verbose_version,
        )));
        let file = dir.join(VARIANT_FILE);
        if fs::read_to_string(&file).ok().as_deref() != Some(&variant) {
            paths::write(&file, &variant)?;
        }
    }
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
    superseded_max_unused: Option<Duration>,
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
            let mut units = Vec::new();
            for unit in read_dirs(&pkg)? {
                // Build scripts can also create native output directories here.
                if !unit.join(LAST_USE_FILE).is_file() && !unit.join("fingerprint").is_dir() {
                    continue;
                }
                let last_use =
                    mtime(&unit.join(LAST_USE_FILE)).max(mtime(&unit.join("fingerprint")));
                units.push((unit, last_use));
            }
            if let Some(max_unused) = superseded_max_unused {
                remove_superseded_variants(&mut units, live, max_unused, removed)?;
            }
            candidates.extend(
                units
                    .into_iter()
                    .filter(|(unit, _)| !live.contains(unit))
                    .map(|(unit, last_use)| Candidate::new(unit, last_use)),
            );
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

    // `CARGO_TARGET_TMPDIR`: tests write there and nothing else cleans it.
    let tmp = root.join("tmp");
    tmp_candidates(&tmp, TMP_DEPTH, settings.max_unused, &mut candidates)?;

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
        if tmp.is_dir() {
            total += cargo_util::du(&tmp, &[])?;
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

/// How deep [`tmp_candidates`] looks into `tmp/`. A test suite usually keeps
/// one directory per test a few levels down (Cargo's own:
/// `tmp/cit/testsuite/<module>/<test>`), and only some of them run again.
const TMP_DEPTH: usize = 4;

/// Adds the directories in `dir` that nothing was written to for
/// `max_unused`; looks into the others, `depth` levels at most. Files are
/// left alone: a directory holding only stale files goes as a whole.
fn tmp_candidates(
    dir: &Path,
    depth: usize,
    max_unused: Duration,
    candidates: &mut Vec<Candidate>,
) -> CargoResult<()> {
    for entry in read_dirs(dir)? {
        let last_use = newest_mtime_within(&entry, max_unused);
        if age(last_use) > max_unused {
            candidates.push(Candidate::new(entry, last_use));
        } else if depth > 1 {
            tmp_candidates(&entry, depth - 1, max_unused, candidates)?;
        }
    }
    Ok(())
}

/// Removes the units of one package (`build/<pkg>/`) that a newer unit of the
/// same variant ([`VARIANT_FILE`]) has replaced and that no build has used for
/// `max_unused`, and drops them from `units`.
fn remove_superseded_variants(
    units: &mut Vec<(PathBuf, FileTime)>,
    live: &HashSet<PathBuf>,
    max_unused: Duration,
    removed: &mut Removed,
) -> CargoResult<()> {
    if units.len() < 2 {
        return Ok(());
    }
    let mut newest: cargo_util::collections::HashMap<String, FileTime> = Default::default();
    let variants: Vec<Option<String>> = units
        .iter()
        .map(|(unit, last_use)| {
            let variant = fs::read_to_string(unit.join(VARIANT_FILE)).ok()?;
            let use_ = if live.contains(unit) {
                FileTime::now()
            } else {
                *last_use
            };
            let entry = newest.entry(variant.clone()).or_insert(use_);
            *entry = (*entry).max(use_);
            Some(variant)
        })
        .collect();
    let mut kept = Vec::with_capacity(units.len());
    for ((unit, last_use), variant) in units.drain(..).zip(variants) {
        let superseded = variant.is_some_and(|v| newest[&v] > last_use)
            && !live.contains(&unit)
            && age(last_use) > max_unused;
        if superseded && remove(&unit, removed)? > 0 {
            continue;
        }
        if unit.exists() {
            kept.push((unit, last_use));
        }
    }
    *units = kept;
    Ok(())
}

/// Removes the outputs Cargo linked into the target-dir
/// (`[<target>/]<profile>/[examples/]`) that no build unit holds any more and
/// that have not changed for `max_unused`, with their dep-info files.
///
/// Outputs are hard links to a unit's files, so one with a single link is all
/// that is left of a unit the collector removed. Where Cargo had to copy
/// instead (another file system), every output has a single link; a build
/// links those it still needs again.
fn collect_orphan_outputs(
    target_dir: &Path,
    max_unused: Duration,
    removed: &mut Removed,
) -> CargoResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // A profile dir is where Cargo keeps its `.cargo-lock`.
        let mut profiles = Vec::new();
        for dir in read_dirs(target_dir)? {
            if dir.join(".cargo-lock").is_file() {
                profiles.push(dir);
            } else {
                profiles.extend(
                    read_dirs(&dir)?
                        .into_iter()
                        .filter(|d| d.join(".cargo-lock").is_file()),
                );
            }
        }
        for profile in profiles {
            for dir in [profile.clone(), profile.join("examples")] {
                let Ok(entries) = fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if !meta.is_file()
                        || meta.nlink() != 1
                        || path.extension().is_some_and(|e| e == "d")
                        || age(FileTime::from_last_modification_time(&meta)) <= max_unused
                    {
                        continue;
                    }
                    // Only what Cargo put there: an output has its dep-info.
                    let Some(stem) = path.file_stem() else {
                        continue;
                    };
                    let dep_info = dir.join(stem).with_extension("d");
                    if !dep_info.is_file() {
                        continue;
                    }
                    debug!("build.gc removing orphan output {}", path.display());
                    paths::remove_file(&path)?;
                    removed.count += 1;
                    removed.bytes += meta.len();
                    let _ = fs::remove_file(&dep_info);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (target_dir, max_unused, removed);
    Ok(())
}

/// Removes what a Cargo that builds in the target-dir itself (the rustup
/// proxy's, or this one with its defaults turned off) left in
/// `[<target>/]<profile>/`: `deps/`, `incremental/`, `build/` and
/// `.fingerprint/`. This Cargo keeps its build units in the build-dir and
/// only links outputs into the target-dir, so it never uses them. Only
/// directories nothing has written to for `max_unused` go.
fn collect_foreign_outputs(
    target_dir: &Path,
    max_unused: Duration,
    removed: &mut Removed,
) -> CargoResult<()> {
    let mut profiles = Vec::new();
    for dir in read_dirs(target_dir)? {
        if dir.join(".cargo-lock").is_file() {
            profiles.push(dir);
        } else {
            profiles.extend(
                read_dirs(&dir)?
                    .into_iter()
                    .filter(|d| d.join(".cargo-lock").is_file()),
            );
        }
    }
    for profile in profiles {
        for name in ["deps", "incremental", "build", ".fingerprint"] {
            let dir = profile.join(name);
            if !dir.is_dir() || age(newest_mtime_within(&dir, max_unused)) <= max_unused {
                continue;
            }
            let size = cargo_util::du(&dir, &[]).unwrap_or(0);
            debug!("build.gc removing foreign outputs {}", dir.display());
            paths::remove_dir_all(&dir)?;
            removed.count += 1;
            removed.bytes += size;
        }
    }
    Ok(())
}

/// Collects the build-dirs in `targets` (`{cargo-cache-home}/targets/`)
/// other than `keep`, see the module docs. Only directories Cargo created
/// (with a `CACHEDIR.TAG`) are considered, and a project a running build has
/// locked is never removed whole.
fn collect_idle_projects(
    targets: &Path,
    keep: &[&Path],
    settings: &GcSettings,
    removed: &mut Removed,
) -> CargoResult<()> {
    let mut projects = Vec::new();
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
        {
            continue;
        }
        let idle = age(last_project_use(&dir)?);
        if idle > settings.shared_max_unused {
            remove_project(targets, &dir, removed)?;
            continue;
        }
        if idle > settings.max_unused {
            strip_idle_project(&dir, settings.max_unused, removed)?;
        }
        projects.push((dir, idle));
    }

    let MaxSize::Bytes(max_size) = settings.projects_max_size else {
        return Ok(());
    };
    let mut sized = projects
        .into_iter()
        .map(|(dir, idle)| {
            let size = project_size(&dir);
            (dir, idle, size)
        })
        .collect::<Vec<_>>();
    // Least recently built first.
    sized.sort_by(|a, b| b.1.cmp(&a.1));
    let total: u64 = sized.iter().map(|(_, _, size)| size).sum();
    let short_of_free = disk_space(targets)
        .map(|(free, size)| min_free_space(size).saturating_sub(free))
        .unwrap_or(0);
    debug!(
        "project build-dirs are {total} bytes (limit {max_size}), \
         disk short of {short_of_free} free bytes"
    );
    evict_projects(targets, sized, total, max_size, short_of_free, removed)
}

/// Removes whole projects, `sized` least recently built first, see
/// [`collect_idle_projects`].
fn evict_projects(
    targets: &Path,
    sized: Vec<(PathBuf, Duration, u64)>,
    mut total: u64,
    max_size: u64,
    short_of_free: u64,
    removed: &mut Removed,
) -> CargoResult<()> {
    // Over the limit: projects built recently stay, so that switching between
    // more projects than fit does not rebuild them over and over.
    let mut freed = 0;
    let mut left = Vec::with_capacity(sized.len());
    for (dir, idle, size) in sized {
        if total > max_size && idle > RECENT_PROJECT && remove_project(targets, &dir, removed)? {
            total = total.saturating_sub(size);
            freed += size;
        } else {
            left.push((dir, size));
        }
    }
    // A nearly full disk fails every build: then any project not being built
    // goes, least recently built first.
    for (dir, size) in left {
        if freed >= short_of_free {
            break;
        }
        if remove_project(targets, &dir, removed)? {
            freed += size;
        }
    }
    Ok(())
}

/// The size of a project's build-dir, cached in [`SIZE_CACHE_FILE`]: a
/// project no build has used since it was measured has not grown, so the
/// projects of a machine with many of them are not walked every time.
fn project_size(dir: &Path) -> u64 {
    let cache = dir.join(SIZE_CACHE_FILE);
    // Without `.cargo-gc-sizes` the last use comes from the directories, which
    // writing the cache would change.
    let cacheable = dir.join(BUILD_SIZES_FILE).is_file();
    if cacheable
        && let Ok(last_use) = last_project_use(dir)
        && mtime(&cache) > last_use
        && let Some(size) = fs::read_to_string(&cache)
            .ok()
            .and_then(|s| s.trim().parse().ok())
    {
        return size;
    }
    let size = cargo_util::du(dir, &[]).unwrap_or(0);
    if cacheable {
        let _ = fs::write(&cache, size.to_string());
    }
    size
}

/// The free space a collection keeps on the disk of the build-dirs: 5% of
/// it, at most [`MAX_MIN_FREE_SPACE`].
fn min_free_space(disk_size: u64) -> u64 {
    (disk_size / 20).min(MAX_MIN_FREE_SPACE)
}

/// `(available, total)` bytes of the file system holding `path`.
#[cfg(unix)]
fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` is written on success.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `statvfs` succeeded.
    let stat = unsafe { stat.assume_init() };
    let block = stat.f_frsize as u64;
    Some((stat.f_bavail as u64 * block, stat.f_blocks as u64 * block))
}

#[cfg(windows)]
fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut available, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated; the out pointers are valid or null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some((available, total))
}

#[cfg(not(any(unix, windows)))]
fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Removes what only the next edit of an idle project would use: its
/// incremental dirs, and `tmp/` entries unused for `max_unused`. Its build
/// units stay, so building it again as it was rebuilds nothing.
fn strip_idle_project(dir: &Path, max_unused: Duration, removed: &mut Removed) -> CargoResult<()> {
    let before = removed.bytes;
    for profile in profile_dirs(dir)? {
        for incremental in read_dirs(&profile.join("incremental"))? {
            remove(&incremental, removed)?;
        }
    }
    let mut stale = Vec::new();
    tmp_candidates(&dir.join("tmp"), TMP_DEPTH, max_unused, &mut stale)?;
    for c in stale {
        remove(&c.path, removed)?;
    }
    if removed.bytes != before {
        let _ = fs::remove_file(dir.join(SIZE_CACHE_FILE));
    }
    Ok(())
}

const REMOVING_SUFFIX: &str = ".removing-";

/// When a build last used a project's build-dir. Every successful build
/// rewrites `.cargo-gc-sizes`, and only a build does: collecting the project
/// from another one changes the directories, not this file. Older build-dirs
/// only have `.cargo-gc-last` and the directories a build creates entries in.
fn last_project_use(dir: &Path) -> CargoResult<FileTime> {
    let sizes = dir.join(BUILD_SIZES_FILE);
    if sizes.is_file() {
        return Ok(mtime(&sizes).max(mtime(&dir.join(LAST_GC_FILE))));
    }
    let mut last = newest_child_mtime(dir);
    for profile in profile_dirs(dir)? {
        last = last.max(newest_child_mtime(&profile));
    }
    Ok(last)
}

/// Removes a whole project build-dir unless a build holds one of its
/// `<profile>/.cargo-build-lock`. It is first renamed away so that a build
/// starting meanwhile gets a fresh, empty directory.
fn remove_project(targets: &Path, dir: &Path, removed: &mut Removed) -> CargoResult<bool> {
    let mut _locks = Vec::new();
    for profile in profile_dirs(dir)? {
        let Ok(file) = fs::File::open(profile.join(".cargo-build-lock")) else {
            continue;
        };
        match flock::try_lock_exclusive(&file) {
            Ok(()) => _locks.push(file),
            Err(fs::TryLockError::WouldBlock) => {
                debug!("build.gc skipping {}: in use", dir.display());
                return Ok(false);
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
        return Ok(false);
    }
    drop(_locks);
    paths::remove_dir_all(&trash)?;
    removed.projects += 1;
    removed.bytes += size;
    Ok(true)
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

/// The newest modification time of `dir` and everything in it, stopping as
/// soon as anything is newer than `max_age`: that is all the caller needs.
fn newest_mtime_within(dir: &Path, max_age: Duration) -> FileTime {
    let mut newest = mtime(dir);
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if let Ok(meta) = entry.metadata() {
            newest = newest.max(FileTime::from_last_modification_time(&meta));
        }
        if age(newest) <= max_age {
            break;
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
        let settings = GcSettings {
            projects_max_size: MaxSize::None,
            ..GcSettings::default()
        };
        collect_idle_projects(&tmp, &[current.as_path()], &settings, &mut removed).unwrap();
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

    /// A project build-dir last built `days` ago, with a unit and an
    /// incremental dir of `bytes` each.
    fn built_project(tmp: &Path, name: &str, days: i64, bytes: usize) -> PathBuf {
        let dir = tmp.join(name);
        let unit = dir.join("debug/build/foo/0123456789abcdef");
        let inc = dir.join("debug/incremental/foo-1abc/s-1");
        fs::create_dir_all(&unit).unwrap();
        fs::create_dir_all(&inc).unwrap();
        fs::write(unit.join("libfoo.rlib"), vec![0u8; bytes]).unwrap();
        fs::write(inc.join("0.o"), vec![0u8; bytes]).unwrap();
        fs::write(dir.join("CACHEDIR.TAG"), "").unwrap();
        fs::write(dir.join(BUILD_SIZES_FILE), "").unwrap();
        filetime::set_file_mtime(dir.join(BUILD_SIZES_FILE), days_ago(days)).unwrap();
        dir
    }

    #[test]
    fn idle_projects_lose_incremental_dirs() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-strip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let active = built_project(&tmp, "active-1", 0, 10);
        let idle = built_project(&tmp, "idle-1", 2, 10);

        let mut removed = Removed::default();
        let settings = GcSettings {
            projects_max_size: MaxSize::None,
            ..GcSettings::default()
        };
        collect_idle_projects(&tmp, &[], &settings, &mut removed).unwrap();
        let result = (
            active.join("debug/incremental/foo-1abc").exists(),
            idle.join("debug/incremental/foo-1abc").exists(),
            idle.join("debug/build/foo/0123456789abcdef").exists(),
            // Stripping is not a use: it still ages out in a week.
            age(last_project_use(&idle).unwrap()) > Duration::from_secs(24 * 3600),
            removed.projects,
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (true, false, true, true, 0));
    }

    #[test]
    fn projects_fit_their_limit() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-total-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let size = 64 * 1024;
        let newest = built_project(&tmp, "newest-1", 0, size);
        let middle = built_project(&tmp, "middle-1", 0, size);
        let oldest = built_project(&tmp, "oldest-1", 0, size);
        filetime::set_file_mtime(newest.join(BUILD_SIZES_FILE), hours_ago(7)).unwrap();
        filetime::set_file_mtime(middle.join(BUILD_SIZES_FILE), hours_ago(8)).unwrap();
        filetime::set_file_mtime(oldest.join(BUILD_SIZES_FILE), hours_ago(9)).unwrap();
        let current = built_project(&tmp, "current-1", 5, size);

        let mut removed = Removed::default();
        let settings = GcSettings {
            // Room for two of the three others.
            projects_max_size: MaxSize::Bytes(5 * size as u64),
            ..GcSettings::default()
        };
        collect_idle_projects(&tmp, &[current.as_path()], &settings, &mut removed).unwrap();
        let result = (
            newest.exists(),
            middle.exists(),
            oldest.exists(),
            current.exists(),
            removed.projects,
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (true, true, false, true, 1));
    }

    fn hours_ago(hours: i64) -> FileTime {
        FileTime::from_unix_time(FileTime::now().unix_seconds() - hours * 3600, 0)
    }

    #[test]
    fn recent_projects_stay_unless_the_disk_is_full() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-evict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let projects = [("old-1", 9), ("recent-1", 2), ("newest-1", 1)].map(|(name, hours)| {
            let dir = built_project(&tmp, name, 0, 1);
            (dir, Duration::from_secs(hours * 3600), 100)
        });
        let left = |projects: &[(PathBuf, Duration, u64)]| {
            projects
                .iter()
                .map(|(d, _, _)| d.exists())
                .collect::<Vec<_>>()
        };

        // Over the limit: only the project not built recently goes.
        let mut removed = Removed::default();
        evict_projects(&tmp, projects.to_vec(), 300, 50, 0, &mut removed).unwrap();
        let over_limit = (left(&projects), removed.projects);

        // Short of free space: recent ones go too, oldest first, until enough.
        evict_projects(&tmp, projects.to_vec(), 200, u64::MAX, 150, &mut removed).unwrap();
        let disk_full = (left(&projects), removed.projects);
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(over_limit, (vec![false, true, true], 1));
        assert_eq!(disk_full, (vec![false, false, false], 3));
    }

    #[test]
    fn project_sizes_are_cached_until_the_next_build() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-du-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let dir = built_project(&tmp, "foo-1", 2, 4096);
        let measured = project_size(&dir);
        // Not walked again: a stale cache shows.
        fs::write(dir.join(SIZE_CACHE_FILE), "7").unwrap();
        let cached = project_size(&dir);
        // A build since makes it measure again.
        filetime::set_file_mtime(dir.join(SIZE_CACHE_FILE), days_ago(3)).unwrap();
        let rebuilt = project_size(&dir);
        let _ = fs::remove_dir_all(&tmp);
        assert!(measured >= 2 * 4096, "{measured}");
        assert_eq!(cached, 7);
        // Measured again (now with the cache file in it).
        assert!(rebuilt >= measured && rebuilt < measured + 64, "{rebuilt}");
    }

    #[test]
    fn free_space_kept() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(min_free_space(100 * gib), 5 * gib);
        assert_eq!(min_free_space(1000 * gib), 20 * gib);
    }

    #[test]
    fn foreign_outputs() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-foreign-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let profile = tmp.join("debug");
        let cross = tmp.join("wasm32-unknown-unknown/debug");
        for p in [&profile, &cross] {
            fs::create_dir_all(p.join("deps")).unwrap();
            fs::write(p.join(".cargo-lock"), "").unwrap();
            fs::write(p.join("deps/libfoo.rlib"), "x").unwrap();
            fs::write(p.join("foo"), "bin").unwrap();
        }
        fs::create_dir_all(profile.join("incremental/foo-1/s-1")).unwrap();
        for f in [
            "deps/libfoo.rlib",
            "deps",
            "incremental/foo-1/s-1",
            "incremental/foo-1",
            "incremental",
        ] {
            filetime::set_file_mtime(profile.join(f), days_ago(1)).unwrap();
        }
        // Written a moment ago: a build may be running.
        let mut removed = Removed::default();
        collect_foreign_outputs(&tmp, Duration::from_secs(3600), &mut removed).unwrap();
        let result = (
            profile.join("deps").exists(),
            profile.join("incremental").exists(),
            profile.join("foo").exists(),
            cross.join("deps").exists(),
            removed.count,
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (false, false, true, true, 2));
    }

    fn days_ago(days: i64) -> FileTime {
        FileTime::from_unix_time(FileTime::now().unix_seconds() - days * 24 * 3600, 0)
    }

    #[test]
    fn superseded_variants() {
        let tmp =
            std::env::temp_dir().join(format!("build-dir-gc-variants-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let unit = |name: &str, variant: Option<&str>| {
            let dir = tmp.join(name);
            fs::create_dir_all(&dir).unwrap();
            if let Some(v) = variant {
                fs::write(dir.join(VARIANT_FILE), v).unwrap();
            }
            dir
        };
        // a: replaced by a newer build two days ago -> removed.
        // b: the newest of variant 1 -> kept.
        // c: replaced, but used within a day -> kept.
        // d: the only unit of variant 2 -> kept.
        // e: no variant recorded (built before) -> kept.
        // f: replaced by g, which this build uses -> removed.
        let units = vec![
            (unit("a", Some("1")), days_ago(3)),
            (unit("b", Some("1")), days_ago(2)),
            (unit("c", Some("3")), FileTime::now()),
            (unit("c2", Some("3")), FileTime::now()),
            (unit("d", Some("2")), days_ago(5)),
            (unit("e", None), days_ago(5)),
            (unit("f", Some("4")), days_ago(2)),
            (unit("g", Some("4")), days_ago(9)),
        ];
        let mut live = HashSet::default();
        live.insert(tmp.join("g"));
        let mut units = units;
        let mut removed = Removed::default();
        let day = Duration::from_secs(24 * 3600);
        remove_superseded_variants(&mut units, &live, day, &mut removed).unwrap();
        let mut left: Vec<_> = units
            .iter()
            .map(|(u, _)| u.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        left.sort();
        let gone = (tmp.join("a").exists(), tmp.join("f").exists());
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(left, ["b", "c", "c2", "d", "e", "g"]);
        assert_eq!(gone, (false, false));
        assert_eq!(removed.count, 2);
    }

    #[cfg(unix)]
    #[test]
    fn orphan_outputs() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-orphans-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let profile = tmp.join("target/debug");
        let unit = tmp.join("build-dir/unit");
        fs::create_dir_all(profile.join("examples")).unwrap();
        fs::create_dir_all(&unit).unwrap();
        fs::write(profile.join(".cargo-lock"), "").unwrap();
        let output = |rel: &str, linked: bool, time: FileTime| {
            let path = profile.join(rel);
            fs::write(&path, "bin").unwrap();
            fs::write(path.with_extension("d"), "").unwrap();
            if linked {
                fs::hard_link(&path, unit.join(rel.replace('/', "-"))).unwrap();
            }
            filetime::set_file_mtime(&path, time).unwrap();
            path
        };
        let orphan = output("examples/old", false, days_ago(3));
        let linked = output("app", true, days_ago(3));
        let recent = output("fresh", false, FileTime::now());
        let not_cargo = profile.join("notes.txt");
        fs::write(&not_cargo, "mine").unwrap();
        filetime::set_file_mtime(&not_cargo, days_ago(3)).unwrap();

        let mut removed = Removed::default();
        let day = Duration::from_secs(24 * 3600);
        collect_orphan_outputs(&tmp.join("target"), day, &mut removed).unwrap();
        let result = (
            orphan.exists(),
            orphan.with_extension("d").exists(),
            linked.exists(),
            recent.exists(),
            not_cargo.exists(),
            removed.count,
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (false, false, true, true, true, 1));
    }

    #[test]
    fn tmp_entries_age_out() {
        let tmp = std::env::temp_dir().join(format!("build-dir-gc-tmp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("debug/build")).unwrap();
        for (name, time) in [("old", days_ago(3)), ("touched", days_ago(3))] {
            let dir = tmp.join("tmp").join(name).join("deep");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("file"), "").unwrap();
            for p in [
                dir.join("file"),
                dir.clone(),
                dir.parent().unwrap().to_path_buf(),
            ] {
                filetime::set_file_mtime(&p, time).unwrap();
            }
        }
        // A test wrote deep inside it today, next to a stale test dir.
        let stale = tmp.join("tmp/touched/stale");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("file"), "").unwrap();
        filetime::set_file_mtime(stale.join("file"), days_ago(3)).unwrap();
        filetime::set_file_mtime(&stale, days_ago(3)).unwrap();
        filetime::set_file_mtime(tmp.join("tmp/touched/deep/file"), FileTime::now()).unwrap();

        let mut removed = Removed::default();
        let settings = GcSettings {
            max_size: MaxSize::None,
            ..GcSettings::default()
        };
        collect(
            &tmp,
            &HashSet::default(),
            0,
            FileTime::now(),
            &settings,
            None,
            &mut removed,
        )
        .unwrap();
        let result = (
            tmp.join("tmp/old").exists(),
            tmp.join("tmp/touched/deep/file").exists(),
            stale.exists(),
        );
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(result, (false, true, false));
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
