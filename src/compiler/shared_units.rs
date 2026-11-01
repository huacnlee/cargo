//! Build units of path packages shared across checkouts of a repository.
//!
//! The build-dir of a project lives in `{cargo-cache-home}/targets/` and is
//! shared by all checkouts (worktrees) of its repository. Build units of path
//! packages would normally mix the checkout's root into their hash, because
//! the default mtime-based freshness check cannot tell two checkouts with the
//! same relative paths apart. Like Go's build cache, this module instead keys
//! them by the content of the package's files, so a second worktree of the
//! same commit reuses the units the first one built.
//!
//! * [`SharedUnits::new`] hashes the files of every eligible package once per
//!   build (`PathSource::list_files`, which follows `.gitignore`, `include`
//!   and `exclude`). Each target is keyed by the files it may read (see
//!   [`belongs_to`]). The hash goes into the unit's `unit_id` (its directory)
//!   but not into `-C metadata`, so incremental compilation keeps working
//!   across edits; incremental caches live in the profile's `incremental/`
//!   directory instead of the unit's.
//! * After rustc finishes, [`Record::write`] stores which checkout built the
//!   unit in `<unit>/.cargo-checkout`. It marks the unit `unverified` if one
//!   of the files it is keyed by changed after they were hashed, or `bound`
//!   if the crate read an environment variable that names a path in the
//!   checkout, like `CARGO_MANIFEST_DIR`.
//! * When checking freshness, the files a unit is keyed by need no mtime
//!   check: the hash already proves they are the ones it was built from. So
//!   a second worktree, or switching back to a branch, reuses the unit. Files
//!   outside of them are checked by mtime as usual. Other checkouts rebuild a
//!   unit marked `bound` or `unverified`; the one that built an `unverified`
//!   unit checks all its files by mtime.
//!
//! Packages with a build script, and units of `rustdoc`, keep the checkout in
//! their hash: their output may depend on the checkout in ways Cargo does not
//! see. `CARGO_SHARE_WORKSPACE_UNITS=0` turns this off.

use std::collections::hash_map::Entry;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use cargo_util::paths;
use filetime::FileTime;
use tracing::debug;

use crate::compiler::fingerprint;
use crate::compiler::{BuildContext, CompileMode, Unit};
use crate::util::GlobalContext;
use crate::util::data_structures::{HashMap, HashSet};
use crate::util::{CargoResult, StableHasher, short_hash};
use crate::workspace::{Package, PackageId, Target};

/// Records which checkout built a unit, see [`Record::write`].
const MARKER: &str = ".cargo-checkout";
/// Second line of [`MARKER`] for a unit that read a path in the checkout:
/// other checkouts must not reuse it.
const BOUND: &str = "bound";
/// Second line of [`MARKER`] for a unit whose package files changed while it
/// was built: it may not match its hash.
const UNVERIFIED: &str = "unverified";

/// Environment variables Cargo sets to a path inside the checkout.
fn names_checkout_path(key: &str) -> bool {
    matches!(
        key,
        "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_RUSTC_CURRENT_DIR"
    ) || key.starts_with("CARGO_BIN_EXE_")
}

/// The profile's directory of `--out-dir` symlinks (see
/// `CompilationFiles::out_dir_link`).
pub const OUT_LINKS_DIR: &str = ".out";

/// Points the `--out-dir` symlink `link` at `dir`, replacing it atomically.
pub fn point_out_dir(link: &Path, dir: &Path) -> CargoResult<()> {
    #[cfg(unix)]
    {
        if std::fs::read_link(link).is_ok_and(|t| t == dir) {
            return Ok(());
        }
        let parent = link.parent().expect("out-dir link has a parent");
        paths::create_dir_all(parent)?;
        let tmp = link.with_extension(format!("tmp{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(dir, &tmp)
            .and_then(|()| std::fs::rename(&tmp, link))
            .with_context(|| {
                format!("failed to link `{}` to `{}`", link.display(), dir.display())
            })?;
    }
    #[cfg(not(unix))]
    let _ = (link, dir);
    Ok(())
}

/// Locks an `--out-dir` link for the duration of a compile, so two checkouts
/// sharing it do not repoint it under each other.
pub fn lock_out_dir(link: &Path) -> CargoResult<fs::File> {
    let path = link.with_extension("lock");
    paths::create_dir_all(path.parent().expect("out-dir link has a parent"))?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open `{}`", path.display()))?;
    crate::util::flock::lock_exclusive(&file)
        .with_context(|| format!("failed to lock `{}`", path.display()))?;
    Ok(file)
}

/// Identifies a checkout in `.cargo-checkout` and `.cargo-users` files.
pub fn checkout_key(root: &Path) -> String {
    short_hash(&root)
}

/// The files a unit is keyed by, and their hash.
pub struct UnitContent {
    pub hash: u64,
    pub files: Arc<HashSet<PathBuf>>,
    /// When hashing started. A file modified after it may not match the hash.
    pub hashed_at: FileTime,
}

pub struct SharedUnits {
    /// Identifies this checkout: a hash of the workspace root.
    checkout: String,
    targets: HashMap<(PackageId, Target), UnitContent>,
}

impl SharedUnits {
    pub fn new(bcx: &BuildContext<'_, '_>) -> CargoResult<Option<SharedUnits>> {
        let gctx = bcx.gctx;
        if !gctx.cli_unstable().build_dir_new_layout
            || crate::workspace::features::is_fork_defaults_opt_out()
            || gctx.get_env_os("CARGO_SHARE_WORKSPACE_UNITS") == Some("0".as_ref())
        {
            return Ok(None);
        }
        let Some(root) = bcx.ws.shared_build_dir_root() else {
            return Ok(None);
        };
        let checkout = checkout_key(root);

        // The hash of each file of each eligible package.
        let mut packages: HashMap<PackageId, Option<(FileTime, Vec<(PathBuf, u64)>)>> =
            HashMap::default();
        let mut targets = HashMap::default();
        for unit in bcx.unit_graph.keys() {
            let id = unit.pkg.package_id();
            let key = (id, unit.target.clone());
            if unit.is_std || !id.source_id().is_path() || targets.contains_key(&key) {
                continue;
            }
            let files = match packages.entry(id) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => e.insert(hash_files(&unit.pkg, gctx)?),
            };
            let Some((hashed_at, files)) = files else {
                continue;
            };
            let own = files
                .iter()
                .filter(|(file, _)| belongs_to(file, &unit.target, unit.pkg.targets()))
                .collect::<Vec<_>>();
            let mut hasher = StableHasher::new();
            for (file, hash) in &own {
                file.strip_prefix(unit.pkg.root())
                    .unwrap_or(file)
                    .hash(&mut hasher);
                hash.hash(&mut hasher);
            }
            targets.insert(
                key,
                UnitContent {
                    hash: Hasher::finish(&hasher),
                    files: Arc::new(own.into_iter().map(|(f, _)| f.clone()).collect()),
                    hashed_at: *hashed_at,
                },
            );
        }
        debug!("keying {} path targets by content", targets.len());
        Ok(Some(SharedUnits { checkout, targets }))
    }

    /// Identifies this checkout.
    pub fn checkout(&self) -> &str {
        &self.checkout
    }

    /// The content the unit is keyed by, if it is.
    pub fn content(&self, unit: &Unit) -> Option<&UnitContent> {
        match unit.mode {
            CompileMode::Build | CompileMode::Check { .. } | CompileMode::Test => self
                .targets
                .get(&(unit.pkg.package_id(), unit.target.clone())),
            CompileMode::Doc
            | CompileMode::Doctest
            | CompileMode::Docscrape
            | CompileMode::RunCustomBuild => None,
        }
    }

    /// How the freshness check may treat a content-keyed unit built in
    /// `unit_dir`.
    pub fn reuse(&self, unit: &Unit, unit_dir: &Path) -> Option<Reuse> {
        let content = self.content(unit)?;
        let marker = unit_dir.join(MARKER);
        let text = fs::read_to_string(&marker).unwrap_or_default();
        let mut lines = text.lines();
        let builder = lines.next().unwrap_or("");
        let ours = builder == self.checkout;
        Some(match lines.next() {
            _ if builder.is_empty() => Reuse::Rebuild(marker),
            Some(UNVERIFIED) if ours => Reuse::Mtime,
            Some(BOUND) if !ours => Reuse::Rebuild(marker),
            Some(UNVERIFIED) => Reuse::Rebuild(marker),
            _ => Reuse::Content(Arc::clone(&content.files)),
        })
    }

    /// What to record about the unit once rustc has built it.
    pub fn record(&self, unit: &Unit, unit_dir: &Path) -> Option<Record> {
        let content = self.content(unit)?;
        Some(Record {
            marker: unit_dir.join(MARKER),
            checkout: self.checkout.clone(),
            files: Arc::clone(&content.files),
            hashed_at: content.hashed_at,
        })
    }
}

/// Hashes the files of a path package, or `None` if its units must stay
/// bound to the checkout: a build script may read anything in it.
fn hash_files(
    pkg: &Package,
    gctx: &GlobalContext,
) -> CargoResult<Option<(FileTime, Vec<(PathBuf, u64)>)>> {
    if pkg.targets().iter().any(|t| t.is_custom_build()) {
        return Ok(None);
    }
    let hashed_at = FileTime::now();
    let mut files = Vec::new();
    for entry in crate::sources::path::list_files(pkg, gctx)? {
        if !entry.is_file() {
            continue;
        }
        let file = entry.into_path_buf();
        let mut hasher = StableHasher::new();
        paths::read_bytes(&file)?.hash(&mut hasher);
        files.push((file, Hasher::finish(&hasher)));
    }
    files.sort();
    Ok(Some((hashed_at, files)))
}

/// Whether `file` may be read when compiling `target`: it is not the root of
/// another target, nor in the directory of another target's root unless that
/// directory also holds `target`'s root (like `src/` of `src/main.rs` for
/// `src/lib.rs`). Files left out are still checked by mtime, so this only
/// decides what an edit rebuilds: editing `tests/a.rs` does not rebuild the
/// library and everything that depends on it.
fn belongs_to(file: &Path, target: &Target, targets: &[Target]) -> bool {
    let Some(own) = target.src_path().path() else {
        return true;
    };
    let own_dir = own.parent().unwrap_or(own);
    for other in targets {
        let Some(root) = other.src_path().path() else {
            continue;
        };
        if root == own {
            continue;
        }
        if file == root {
            return false;
        }
        let dir = root.parent().unwrap_or(root);
        if !own_dir.starts_with(dir) && file.starts_with(dir) {
            return false;
        }
    }
    true
}

pub enum Reuse {
    /// Built from the package files the unit's hash names: those need no
    /// mtime check, the rest (files outside the package) do.
    Content(Arc<HashSet<PathBuf>>),
    /// Built by this checkout while its files changed: check freshness as
    /// usual.
    Mtime,
    /// Not reusable by this checkout: always rebuild.
    Rebuild(PathBuf),
}

pub struct Record {
    marker: PathBuf,
    checkout: String,
    files: Arc<HashSet<PathBuf>>,
    hashed_at: FileTime,
}

impl Record {
    /// Writes `<unit>/.cargo-checkout` after rustc wrote `rustc_dep_info` and
    /// Cargo translated it to `dep_info`.
    pub fn write(
        &self,
        rustc_dep_info: &Path,
        dep_info: &Path,
        pkg_root: &Path,
        build_root: &Path,
        ws_root: &Path,
    ) -> CargoResult<()> {
        let state = self.state(rustc_dep_info, dep_info, pkg_root, build_root, ws_root)?;
        let mut text = self.checkout.clone();
        if let Some(state) = state {
            text.push('\n');
            text.push_str(state);
        }
        paths::write(&self.marker, text)
    }

    /// [`UNVERIFIED`], [`BOUND`], or `None` for a unit any checkout can reuse.
    fn state(
        &self,
        rustc_dep_info: &Path,
        dep_info: &Path,
        pkg_root: &Path,
        build_root: &Path,
        ws_root: &Path,
    ) -> CargoResult<Option<&'static str>> {
        let Some(info) = fingerprint::parse_dep_info(pkg_root, build_root, ws_root, dep_info)?
        else {
            return Ok(Some(UNVERIFIED));
        };
        for file in info.files.keys() {
            if !self.files.contains(file) {
                continue;
            }
            match paths::mtime(file) {
                Ok(mtime) if mtime < self.hashed_at => {}
                _ => {
                    debug!("{} changed after hashing", file.display());
                    return Ok(Some(UNVERIFIED));
                }
            }
        }
        // Cargo's own dep-info drops the variables it set itself.
        let rustc_info = fingerprint::parse_rustc_dep_info(rustc_dep_info)?;
        if let Some((key, _)) = rustc_info.env.iter().find(|(k, _)| names_checkout_path(k)) {
            debug!("{} reads {key}, bound to this checkout", dep_info.display());
            return Ok(Some(BOUND));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CrateType;
    use crate::workspace::Edition;

    #[test]
    fn files_of_a_target() {
        let p = |s: &str| PathBuf::from("/p").join(s);
        let lib = Target::lib_target(
            "p",
            vec![CrateType::Lib],
            p("src/lib.rs"),
            Edition::Edition2021,
        );
        let main = Target::bin_target("p", None, p("src/main.rs"), None, Edition::Edition2021);
        let bin = Target::bin_target(
            "x",
            None,
            p("src/bin/x/main.rs"),
            None,
            Edition::Edition2021,
        );
        let test = Target::test_target("t", p("tests/t.rs"), None, Edition::Edition2021);
        let targets = [lib.clone(), main.clone(), bin.clone(), test.clone()];

        let files = |t: &Target| {
            [
                "Cargo.toml",
                "src/lib.rs",
                "src/m.rs",
                "src/main.rs",
                "src/bin/x/main.rs",
                "src/bin/x/y.rs",
                "tests/t.rs",
                "tests/common/mod.rs",
            ]
            .into_iter()
            .filter(|f| belongs_to(&p(f), t, &targets))
            .collect::<Vec<_>>()
        };
        assert_eq!(files(&lib), ["Cargo.toml", "src/lib.rs", "src/m.rs"]);
        assert_eq!(files(&main), ["Cargo.toml", "src/m.rs", "src/main.rs"]);
        assert_eq!(
            files(&bin),
            [
                "Cargo.toml",
                "src/m.rs",
                "src/bin/x/main.rs",
                "src/bin/x/y.rs"
            ]
        );
        assert_eq!(
            files(&test),
            ["Cargo.toml", "tests/t.rs", "tests/common/mod.rs"]
        );
    }
}
