//! Silent updates of the fork.
//!
//! After a command, at most once a day, a release build installed as
//! `$CARGO_HOME/bin/cargo` starts a detached copy of itself that looks up the
//! latest release on GitHub and, when it is newer, installs it the way
//! `install.sh` does: the archive is checked against `SHA256SUMS`, the new
//! binary must run, and it is renamed into place so running builds keep the
//! old one. Nothing is printed; the outcome of the last attempt is written to
//! `$CARGO_HOME/.fork-update.log`.
//!
//! Off with `CARGO_FORK_AUTO_UPDATE=0`, in CI (`CI` set), for builds that are
//! not a release (no `CARGO_FORK_RELEASE`) and for a cargo that is not the
//! installed one (e.g. `./target/debug/cargo`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, bail};
use cargo::GlobalContext;
use cargo::util::CargoResult;

const REPO: &str = "https://github.com/huacnlee/cargo";
/// Set on the detached child: run the update instead of a command.
const CHILD_ENV: &str = "__CARGO_FORK_SELF_UPDATE";
const STAMP_FILE: &str = ".fork-update";
const LOG_FILE: &str = ".fork-update.log";
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASE: Option<&str> = option_env!("CARGO_FORK_RELEASE");

/// Runs the update when this process is the detached child. Returns whether
/// it was, in which case the caller exits.
#[allow(clippy::disallowed_methods)] // Runs before a `GlobalContext` exists.
pub fn run_if_child() -> bool {
    let Some(home) = env::var_os(CHILD_ENV).map(PathBuf::from) else {
        return false;
    };
    let message = match update(&home) {
        Ok(message) => message,
        Err(e) => format!("failed: {e:#}"),
    };
    let _ = fs::write(home.join(LOG_FILE), format!("{} {message}\n", unix_now()));
    true
}

/// Starts the detached update check when it is due.
pub fn maybe_spawn(gctx: &GlobalContext) {
    let Some(_) = RELEASE else { return };
    let off = gctx
        .get_env_os("CARGO_FORK_AUTO_UPDATE")
        .is_some_and(|v| v == "0" || v == "false");
    if gctx.get_env_os("CI").is_some()
        || off
        || cargo::workspace::features::is_fork_defaults_opt_out()
    {
        return;
    }
    let home = gctx.home().as_path_unlocked().to_path_buf();
    let Ok(exe) = env::current_exe() else { return };
    if !is_installed(&exe, &home) {
        return;
    }
    let stamp = home.join(STAMP_FILE);
    let due = fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .map_or(true, |t| t.elapsed().map_or(true, |age| age >= INTERVAL));
    if !due {
        return;
    }
    // Stamp first: a failing or slow check must not run on every command.
    if fs::write(&stamp, "").is_err() {
        return;
    }
    let mut cmd = Command::new(&exe);
    cmd.env(CHILD_ENV, &home)
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut cmd);
    let _ = cmd.spawn();
}

/// Whether `exe` is `$CARGO_HOME/bin/cargo`, the binary the installer put
/// there. rustup's proxy runs the toolchain's cargo, never this path.
fn is_installed(exe: &Path, home: &Path) -> bool {
    let name = format!("cargo{}", env::consts::EXE_SUFFIX);
    let expected = home.join("bin").join(name);
    match (exe.canonicalize(), expected.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Its own process group: Ctrl-C in the terminal does not reach it.
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

fn update(home: &Path) -> CargoResult<String> {
    let current = RELEASE.context("not a release build")?;
    let bin = home.join("bin");
    let target = bin.join(format!("cargo{}", env::consts::EXE_SUFFIX));
    // Left by the previous update on Windows, where the running binary
    // can only be moved aside.
    let _ = fs::remove_file(bin.join("cargo.old.exe"));

    let latest = latest_release()?;
    if !is_newer(&latest, current) {
        return Ok(format!("up to date ({current})"));
    }
    let triple = host_triple()?;
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("cargo-{latest}-{triple}.{ext}");
    let base = format!("{REPO}/releases/download/v{latest}");

    let work = tempfile::Builder::new()
        .prefix(".cargo-update-")
        .tempdir_in(&bin)
        .context("creating a temporary directory")?;
    let archive = work.path().join(&asset);
    fs::write(&archive, download(&format!("{base}/{asset}"))?)?;
    let sums = String::from_utf8(download(&format!("{base}/SHA256SUMS"))?)?;
    let expected =
        expected_checksum(&sums, &asset).with_context(|| format!("no checksum for {asset}"))?;
    let actual = cargo_util::Sha256::new()
        .update_path(&archive)?
        .finish_hex();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {asset}");
    }

    let unpacked = work.path().join("unpacked");
    fs::create_dir(&unpacked)?;
    extract(&archive, &unpacked)?;
    let new = unpacked.join(format!("cargo{}", env::consts::EXE_SUFFIX));
    let ok = Command::new(&new)
        .arg("--version")
        .env_remove(CHILD_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("the downloaded cargo does not run");
    }
    install(&new, &target)?;
    Ok(format!("updated {current} -> {latest}"))
}

/// The newest release, from where `releases/latest` redirects to.
fn latest_release() -> CargoResult<String> {
    let mut easy = curl::easy::Easy::new();
    easy.url(&format!("{REPO}/releases/latest"))?;
    easy.follow_location(true)?;
    easy.nobody(true)?;
    easy.useragent("huacnlee-cargo-updater")?;
    easy.timeout(Duration::from_secs(30))?;
    easy.perform()?;
    let url = easy
        .effective_url()?
        .context("no redirect for the latest release")?
        .to_string();
    let tag = url.rsplit('/').next().unwrap_or("");
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if parse_release(version).is_none() {
        bail!("unexpected latest release {tag:?}");
    }
    Ok(version.to_string())
}

fn download(url: &str) -> CargoResult<Vec<u8>> {
    let mut data = Vec::new();
    let mut easy = curl::easy::Easy::new();
    easy.url(url)?;
    easy.follow_location(true)?;
    easy.fail_on_error(true)?;
    easy.useragent("huacnlee-cargo-updater")?;
    easy.timeout(Duration::from_secs(600))?;
    {
        let mut transfer = easy.transfer();
        transfer.write_function(|chunk| {
            data.extend_from_slice(chunk);
            Ok(chunk.len())
        })?;
        transfer
            .perform()
            .with_context(|| format!("downloading {url}"))?;
    }
    Ok(data)
}

/// `0.102.0-6` -> `([0, 102, 0], 6)`; a release without `-N` is `-0`.
fn parse_release(v: &str) -> Option<([u64; 3], u64)> {
    let (semver, n) = match v.split_once('-') {
        Some((s, n)) => (s, n.parse().ok()?),
        None => (v, 0),
    };
    let mut parts = semver.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some((version, n))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_release(latest), parse_release(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn expected_checksum<'a>(sums: &'a str, asset: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(hash)
    })
}

fn host_triple() -> CargoResult<&'static str> {
    Ok(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => bail!("no release for {arch} {os}"),
    })
}

#[cfg(not(windows))]
fn extract(archive: &Path, into: &Path) -> CargoResult<()> {
    let file = fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries()? {
        let mut entry = entry?;
        // `unpack_in` refuses paths that leave `into`.
        if !entry.unpack_in(into)? {
            bail!("release archive contains an unsafe path");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn extract(archive: &Path, into: &Path) -> CargoResult<()> {
    // Windows 10 and later ship bsdtar, which reads zip files.
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        bail!("tar could not extract {}", archive.display());
    }
    Ok(())
}

/// Renames `new` over `target`. Running cargo processes keep the old file.
fn install(new: &Path, target: &Path) -> CargoResult<()> {
    let dir = target.parent().context("no install directory")?;
    let staged = dir.join(format!(".cargo-staged-{}", std::process::id()));
    fs::copy(new, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        // A running executable cannot be replaced, but it can be moved.
        let old = dir.join("cargo.old.exe");
        let _ = fs::remove_file(&old);
        if target.exists() {
            fs::rename(target, &old)?;
        }
    }
    if let Err(e) = fs::rename(&staged, target) {
        let _ = fs::remove_file(&staged);
        return Err(e.into());
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_order() {
        assert!(is_newer("0.102.0-6", "0.102.0-5"));
        assert!(is_newer("0.102.0-10", "0.102.0-9"));
        assert!(is_newer("0.103.0-1", "0.102.0-9"));
        assert!(!is_newer("0.102.0-5", "0.102.0-5"));
        assert!(!is_newer("0.102.0-4", "0.102.0-5"));
        assert!(!is_newer("garbage", "0.102.0-5"));
        assert!(!is_newer("0.102-1", "0.102.0-5"));
    }

    #[test]
    fn checksums() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  cargo-1-x.tar.gz\n{}  *cargo-1-y.zip\n",
            "b".repeat(64)
        );
        assert_eq!(
            expected_checksum(&sums, "cargo-1-x.tar.gz"),
            Some(h.as_str())
        );
        assert_eq!(
            expected_checksum(&sums, "cargo-1-y.zip"),
            Some("b".repeat(64).as_str())
        );
        assert_eq!(expected_checksum(&sums, "cargo-1-z.zip"), None);
    }
}
