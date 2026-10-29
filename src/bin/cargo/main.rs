use cargo::util::{self, CargoResult, closest_msg, command_prelude};
use cargo::workspace::features;
use cargo_util::{ProcessBuilder, ProcessError};
use cargo_util_schemas::manifest::StringOrVec;
use cargo_util_terminal::Shell;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

mod cli;
mod commands;

use crate::command_prelude::*;

fn main() {
    let _guard = setup_logger();
    raise_fd_limit();
    if !cargo::workspace::features::is_fork_defaults_opt_out() {
        pin_rustup_toolchain();
    }

    let mut gctx = match GlobalContext::default() {
        Ok(gctx) => gctx,
        Err(e) => {
            let mut shell = Shell::new();
            cargo::exit_with_error(e.into(), &mut shell)
        }
    };

    let nightly_features_allowed = matches!(&*features::channel(), "nightly" | "dev");
    if nightly_features_allowed {
        let _span = tracing::span!(tracing::Level::TRACE, "completions").entered();
        let args = std::env::args_os();
        let current_dir = std::env::current_dir().ok();
        let completer = clap_complete::CompleteEnv::with_factory(|| {
            let mut gctx = GlobalContext::default().expect("already loaded without errors");
            cli::cli(&mut gctx)
        })
        .var("CARGO_COMPLETE");
        if completer
            .try_complete(args, current_dir.as_deref())
            .unwrap_or_else(|e| {
                let mut shell = Shell::new();
                cargo::exit_with_error(e.into(), &mut shell)
            })
        {
            return;
        }
    }

    let result = if let Some(lock_addr) = cargo::ops::fix_get_proxy_lock_addr() {
        cargo::ops::fix_exec_rustc(&gctx, &lock_addr).map_err(|e| CliError::from(e))
    } else {
        let _token = cargo::util::job::setup();
        cli::main(&mut gctx)
    };

    match result {
        Err(e) => cargo::exit_with_error(e, &mut *gctx.shell()),
        Ok(()) => {}
    }
}

fn setup_logger() -> Option<tracing_chrome::FlushGuard> {
    use tracing_subscriber::prelude::*;

    let env = tracing_subscriber::EnvFilter::from_env("CARGO_LOG");
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_timer(tracing_subscriber::fmt::time::Uptime::default())
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .with_filter(env);

    let (profile_layer, profile_guard) = chrome_layer();

    let registry = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(profile_layer);
    registry.init();
    tracing::trace!(start = jiff::Timestamp::now().to_string());
    profile_guard
}

fn chrome_layer<S>() -> (
    Option<tracing_chrome::ChromeLayer<S>>,
    Option<tracing_chrome::FlushGuard>,
)
where
    S: tracing::Subscriber
        + for<'span> tracing_subscriber::registry::LookupSpan<'span>
        + Send
        + Sync,
{
    #![expect(clippy::disallowed_methods, reason = "runs before config is loaded")]
    if env_to_bool(std::env::var_os("CARGO_LOG_PROFILE").as_deref()) {
        let capture_args =
            env_to_bool(std::env::var_os("CARGO_LOG_PROFILE_CAPTURE_ARGS").as_deref());
        let (layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
            .include_args(capture_args)
            .build();
        (Some(layer), Some(guard))
    } else {
        (None, None)
    }
}

fn env_to_bool(os: Option<&OsStr>) -> bool {
    match os.and_then(|os| os.to_str()) {
        Some("1") | Some("true") => true,
        _ => false,
    }
}

/// Table for defining the aliases which come builtin in `Cargo`.
/// The contents are structured as: `(alias, aliased_command, description)`.
const BUILTIN_ALIASES: [(&str, &str, &str); 6] = [
    ("b", "build", "alias: build"),
    ("c", "check", "alias: check"),
    ("d", "doc", "alias: doc"),
    ("r", "run", "alias: run"),
    ("t", "test", "alias: test"),
    ("rm", "remove", "alias: remove"),
];

/// Function which contains the list of all of the builtin aliases and it's
/// corresponding execs represented as &str.
fn builtin_aliases_execs(cmd: &str) -> Option<&(&str, &str, &str)> {
    BUILTIN_ALIASES.iter().find(|alias| alias.0 == cmd)
}

/// Resolve the aliased command from the [`GlobalContext`] with a given command string.
///
/// The search fallback chain is:
///
/// 1. Get the aliased command as a string.
/// 2. If an `Err` occurs (missing key, type mismatch, or any possible error),
///    try to get it as an array again.
/// 3. If still cannot find any, finds one insides [`BUILTIN_ALIASES`].
fn aliased_command(gctx: &GlobalContext, command: &str) -> CargoResult<Option<Vec<String>>> {
    let alias_name = format!("alias.{}", command);
    let user_alias = match gctx.get_string(["alias", command]) {
        Ok(Some(record)) => Some(
            record
                .val
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        ),
        Ok(None) => None,
        Err(_) => gctx.get::<Option<Vec<String>>>(["alias", command])?,
    };

    let result = user_alias.or_else(|| {
        builtin_aliases_execs(command).map(|command_str| vec![command_str.1.to_string()])
    });
    if result
        .as_ref()
        .map(|alias| alias.is_empty())
        .unwrap_or_default()
    {
        anyhow::bail!("subcommand is required, but `{alias_name}` is empty");
    }
    Ok(result)
}

/// List all runnable commands
fn list_commands(gctx: &GlobalContext) -> BTreeMap<String, CommandInfo> {
    let mut commands = third_party_subcommands(gctx);

    for cmd in commands::builtin() {
        commands.insert(
            cmd.get_name().to_string(),
            CommandInfo::BuiltIn {
                about: cmd.get_about().map(|s| s.to_string()),
            },
        );
    }

    // Add the builtin_aliases and them descriptions to the
    // `commands` `BTreeMap`.
    for command in &BUILTIN_ALIASES {
        commands.insert(
            command.0.to_string(),
            CommandInfo::BuiltIn {
                about: Some(command.2.to_string()),
            },
        );
    }

    // Add the user-defined aliases
    let alias_commands = user_defined_aliases(gctx);
    commands.extend(alias_commands);

    // `help` is special, so it needs to be inserted separately.
    commands.insert(
        "help".to_string(),
        CommandInfo::BuiltIn {
            about: Some("Displays help for a cargo command".to_string()),
        },
    );

    commands
}

fn third_party_subcommands(gctx: &GlobalContext) -> BTreeMap<String, CommandInfo> {
    let prefix = "cargo-";
    let suffix = env::consts::EXE_SUFFIX;
    let mut commands = BTreeMap::new();
    for dir in search_directories(gctx) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            _ => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(name) = filename.strip_circumfix(prefix, suffix) else {
                continue;
            };
            if is_executable(entry.path()) {
                commands.insert(
                    name.to_string(),
                    CommandInfo::External { path: path.clone() },
                );
            }
        }
    }
    commands
}

fn user_defined_aliases(gctx: &GlobalContext) -> BTreeMap<String, CommandInfo> {
    let mut commands = BTreeMap::new();
    if let Ok(aliases) = gctx.get::<BTreeMap<String, StringOrVec>>(["alias"]) {
        for (name, target) in aliases.iter() {
            commands.insert(
                name.to_string(),
                CommandInfo::Alias {
                    target: target.clone(),
                },
            );
        }
    }
    commands
}

fn find_external_subcommand(gctx: &GlobalContext, cmd: &str) -> Option<PathBuf> {
    let command_exe = format!("cargo-{}{}", cmd, env::consts::EXE_SUFFIX);
    search_directories(gctx)
        .iter()
        .map(|dir| dir.join(&command_exe))
        .find(|file| is_executable(file))
}

fn execute_external_subcommand(gctx: &GlobalContext, cmd: &str, args: &[&OsStr]) -> CliResult {
    let path = find_external_subcommand(gctx, cmd);
    let command = match path {
        Some(command) => command,
        None => {
            let script_suggestion = if gctx.cli_unstable().script
                && std::path::Path::new(cmd).is_file()
            {
                let sep = std::path::MAIN_SEPARATOR;
                format!(
                    "\nhelp: to run the file `{cmd}`, provide a relative path like `.{sep}{cmd}`"
                )
            } else {
                "".to_owned()
            };
            let err = if cmd.starts_with('+') {
                anyhow::format_err!(
                    "no such command: `{cmd}`\n\n\
                    help: invoke `cargo` through `rustup` to handle `+toolchain` directives{script_suggestion}",
                )
            } else {
                let suggestions = list_commands(gctx);
                let did_you_mean = if cmd == "rustfmt" {
                    "\n\nhelp: a command with a similar name exists: `fmt`".to_string()
                } else {
                    closest_msg(cmd, suggestions.keys(), |c| c, "command")
                };

                anyhow::format_err!(
                    "no such command: `{cmd}`{did_you_mean}\n\n\
                    help: view all installed commands with `cargo --list`\n\
                    help: find a package to install `{cmd}` with `cargo search cargo-{cmd}`{script_suggestion}",
                )
            };

            return Err(CliError::new(err, 101));
        }
    };
    execute_subcommand(gctx, Some(&command), args)
}

fn execute_internal_subcommand(gctx: &GlobalContext, args: &[&OsStr]) -> CliResult {
    execute_subcommand(gctx, None, args)
}

// This function is used to execute a subcommand. It is used to execute both
// internal and external subcommands.
// If `cmd_path` is `None`, then the subcommand is an internal subcommand.
fn execute_subcommand(
    gctx: &GlobalContext,
    cmd_path: Option<&PathBuf>,
    args: &[&OsStr],
) -> CliResult {
    let cargo_exe = gctx.cargo_exe()?;
    let mut cmd = match cmd_path {
        Some(cmd_path) => ProcessBuilder::new(cmd_path),
        None => ProcessBuilder::new(&cargo_exe),
    };
    cmd.env(cargo::CARGO_ENV, cargo_exe).args(args);
    if let Some(client) = gctx.jobserver_from_env() {
        cmd.inherit_jobserver(client);
    }
    let err = match cmd.exec_replace() {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };

    if let Some(perr) = err.downcast_ref::<ProcessError>() {
        if let Some(code) = perr.code {
            return Err(CliError::code(code));
        }
    }
    Err(CliError::new(err, 101))
}

#[cfg(unix)]
fn is_executable<P: AsRef<Path>>(path: P) -> bool {
    use std::os::unix::prelude::*;
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
#[cfg(windows)]
fn is_executable<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref().is_file()
}

fn search_directories(gctx: &GlobalContext) -> Vec<PathBuf> {
    let mut path_dirs = if let Some(val) = gctx.get_env_os("PATH") {
        env::split_paths(&val).collect()
    } else {
        vec![]
    };

    let home_bin = gctx.home().clone().into_path_unlocked().join("bin");

    // If any of that PATH elements contains `home_bin`, do not
    // add it again. This is so that the users can control priority
    // of it using PATH, while preserving the historical
    // behavior of preferring it over system global directories even
    // when not in PATH at all.
    // See https://github.com/rust-lang/cargo/issues/11020 for details.
    //
    // Note: `p == home_bin` will ignore trailing slash, but we don't
    // `canonicalize` the paths.
    if !path_dirs.iter().any(|p| p == &home_bin) {
        path_dirs.insert(0, home_bin);
    };

    path_dirs
}

/// Initialize libgit2.
#[tracing::instrument(skip_all)]
fn init_git() {
    // Disabling the owner validation in git can, in theory, lead to code execution
    // vulnerabilities. However, libgit2 does not launch executables, which is the foundation of
    // the original security issue. Meanwhile, issues with refusing to load git repos in
    // `CARGO_HOME` for example will likely be very frustrating for users. So, we disable the
    // validation.
    //
    // For further discussion of Cargo's current interactions with git, see
    //
    //   https://github.com/rust-lang/rfcs/pull/3279
    //
    // and in particular the subsection on "Git support".
    //
    // Note that we only disable this when Cargo is run as a binary. If Cargo is used as a library,
    // this code won't be invoked. Instead, developers will need to explicitly disable the
    // validation in their code. This is inconvenient, but won't accidentally open consuming
    // applications up to security issues if they use git2 to open repositories elsewhere in their
    // code.
    unsafe {
        git2::opts::set_verify_owner_validation(false)
            .expect("set_verify_owner_validation should never fail");
    }
}

/// Raises the soft limit on open files to the hard limit.
///
/// Fine-grained locking keeps a lock file open for every build unit until the
/// build ends, so a large workspace exceeds macOS's default soft limit of 256.
#[cfg(unix)]
fn raise_fd_limit() {
    // SAFETY: plain libc calls on locals.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut wanted = limit.rlim_max;
        // macOS rejects a soft limit above `kern.maxfilesperproc`.
        #[cfg(target_os = "macos")]
        {
            let mut max: libc::c_int = 0;
            let mut size = std::mem::size_of::<libc::c_int>();
            if libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&raw mut max).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            ) == 0
                && max > 0
            {
                wanted = wanted.min(max as libc::rlim_t);
            }
        }
        if wanted > limit.rlim_cur {
            limit.rlim_cur = wanted;
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

/// Pins the toolchain rustup picks for this directory, as the rustup `cargo`
/// proxy does, when this Cargo is run directly instead of through the proxy.
///
/// Otherwise every `rustc` proxy Cargo spawns resolves the toolchain from its
/// own working directory, and a dependency shipping a `rust-toolchain.toml`
/// gets compiled by a different compiler than the rest of the build.
#[allow(
    clippy::disallowed_methods,
    reason = "runs before the config is loaded"
)]
fn pin_rustup_toolchain() {
    // Installed in place of the rustup proxy, so honour `cargo +<toolchain>`
    // the way the proxy would: re-run under that toolchain.
    let mut args = std::env::args_os().skip(1);
    if let Some(toolchain) = args.next().and_then(|a| {
        a.to_str()
            .and_then(|a| a.strip_prefix('+'))
            .map(str::to_owned)
    }) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new("rustup")
            .args(["run", &toolchain])
            .arg(exe)
            .args(args)
            .status();
        match status {
            Ok(status) => std::process::exit(status.code().unwrap_or(101)),
            Err(e) => {
                let mut shell = Shell::new();
                cargo::exit_with_error(
                    anyhow::format_err!("`+{toolchain}` requires rustup: {e}").into(),
                    &mut shell,
                )
            }
        }
    }
    if std::env::var_os("RUSTUP_TOOLCHAIN").is_some() || std::env::var_os("RUSTC").is_some() {
        return;
    }
    let Ok(out) = std::process::Command::new("rustup")
        .args(["show", "active-toolchain"])
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let (true, Some(name)) = (out.status.success(), stdout.split_whitespace().next()) {
        // SAFETY: single-threaded, before anything reads the environment.
        unsafe { std::env::set_var("RUSTUP_TOOLCHAIN", name) };
    }
}
