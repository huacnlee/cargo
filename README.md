# huacnlee/cargo

A drop-in Cargo fork: every git worktree of a project shares one build
directory, which stays under a size limit on its own. Nothing to configure.

https://huacnlee.github.io/cargo/

## Install

```sh
curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://huacnlee.github.io/cargo/install.ps1 | iex
```

This replaces `~/.cargo/bin/cargo` and keeps the rustup proxy as
`cargo.rustup`; run the script with `--uninstall` to put it back.
`cargo +nightly` and `rust-toolchain.toml` keep working. If you have a nightly
toolchain, the installer adds its Cranelift component. `rustup self update`
reinstalls the proxy, so rerun the installer after it.

Check which cargo is active:

```console
$ cargo -V
cargo 1.101.0 (ea4e34c28 2026-10-29) (huacnlee/cargo 0.102.0-1, https://huacnlee.github.io/cargo/)
```

## Results

[nsh](https://github.com/longbridge/nsh), a 525-crate workspace, dev build of
one binary on an 8-core Apple Silicon Mac. Five fresh git worktrees, built one
after another. One run each with no compiler cache, so expect some noise:

| Worktree | Cargo | Disk, all worktrees | This fork | Disk, all worktrees |
|---|---:|---:|---:|---:|
| 1 | 272.9 s | 6.6 GB | 233.9 s | 3.9 GB |
| 2 | 287.4 s | 13.2 GB | 88.9 s | 6.5 GB |
| 3 | 285.6 s | 19.8 GB | 91.4 s | 9.1 GB |
| 4 | 277.2 s | 26.4 GB | 87.5 s | 11.7 GB |
| 5 | 277.4 s | 33.0 GB | 88.7 s | 14.3 GB |
| **Total** | **23.3 min** | **33.0 GB** | **9.8 min** | **14.3 GB** |

Cargo compiles every dependency again in each worktree. The fork compiles them
once; each later worktree only builds its own workspace crates, about 90 s and
2.6 GB. The shared build directory stays under its size limit, twice the
largest build of the last day.

| Rebuild after editing a core crate | Cargo | This fork |
|---|---:|---:|
| Stable | 19.1 s | 19.9 s |
| Nightly, Cranelift | | 21.4 s |

Rebuilding after an edit is not faster on this workspace, on stable or with
Cranelift on nightly.

### Across projects

Release builds of Cargo 1.99.0 and this fork, run one after another on the
same machine: nsh, then [mado](https://github.com/longbridge/mado) (a 330-crate
app in its own repository), then mado again from an empty build directory, as
a new worktree or `cargo clean` would start. One run each:

| Build | Cargo | This fork |
|---|---:|---:|
| nsh, first build | 207.9 s | 206.7 s |
| mado, first build after nsh | 513.8 s | 379.2 s |
| mado again, empty build directory | 513.8 s ¹ | 16.2 s |
| Disk after nsh and mado | 10.3 GB | 4.9 GB |

¹ Cargo compiles all 330 crates again, as in its first build; the fork
compiles 2 and takes the rest from `~/.cargo/targets/shared-deps/`.

mado builds its dependencies at `opt-level = 3` and nsh at 0, so the two share
almost nothing (1 of 330 crates); the fork is faster on mado's first build
because dependencies carry no debug info. The fork's 4.9 GB is both project
directories (2.3 GB and 0.4 GB) plus the shared dependencies (2.2 GB).

## What changes

- **One build directory per project.** Your crates build in
  `~/.cargo/targets/<name>-<hash>/`; the hash comes from the repository's main
  `.git` directory, so every worktree lands there wherever it is created.
  Binaries still appear in `./target`.
- **Fixed directories.** `./target` and `~/.cargo/targets` cannot be moved:
  `--target-dir`, `CARGO_TARGET_DIR`, `build.target-dir` and `build.build-dir`
  are ignored with a warning, so no flag, env var or tool can scatter build
  output elsewhere.
- **Dependencies are shared by every project.** Registry and git dependencies
  build once in `~/.cargo/targets/shared-deps/` and are reused by any project
  that needs the same version, features and profile, the way Go's build cache
  works.
- **It cleans itself.** After a build, at most once an hour, units unused for
  a day are removed, then the least recently used units until each project
  fits twice its largest build of the last day, and the shared dependencies
  fit 50 GiB. Incremental caches live with their unit, so the crates you are
  working on keep them. Nothing a running build has locked is removed.
- **No duplicate object files on macOS.** Debuggers and backtraces read the
  objects inside each rlib, so the loose copies rustc leaves beside it are
  removed.
- **Worktrees build at the same time.** Fine-grained locking is on. When two
  builds need the same dependency at once, one compiles it and the other
  reuses the result.
- **Less debug info in dev builds.** Line tables for workspace members, none
  for dependencies. A `debug` setting in your profile still wins.
- **Cranelift on nightly.** `cargo build` compiles local crates with Cranelift
  and runs the front end on up to 8 threads (`-Zthreads`). Tests and release
  builds keep LLVM.

All of these are defaults. To change one:

```toml
# ~/.cargo/config.toml
[build.gc]
max-size = "auto"     # or "15 GiB", "none"
shared-max-size = "50 GiB"  # shared dependencies; or "none"
max-unused = "1 day"
frequency = "1 hour"  # or "always", "never"
enabled = true
```

`CARGO_SHARED_DEPS=0` keeps dependencies in the project's build directory;
`CARGO_DEV_CRANELIFT=0` keeps LLVM on nightly; `CARGO_RUSTC_THREADS=1` makes
the front end single-threaded.

---

# Cargo

Cargo downloads your Rust project’s dependencies and compiles your project.

**To start using Cargo**, learn more at [The Cargo Book].

**To start developing Cargo itself**, read the [Cargo Contributor Guide].

[The Cargo Book]: https://doc.rust-lang.org/cargo/
[Cargo Contributor Guide]: https://rust-lang.github.io/cargo/contrib/

> The Cargo binary distributed through with Rust is maintained by the Cargo
> team for use by the wider ecosystem.
> For all other uses of this crate (as a binary or library) this is maintained
> by the Cargo team, primarily for use by Cargo and not intended for external
> use (except as a transitive dependency). This crate may make major changes to
> its APIs.


## Compiling from Source

### Requirements

Cargo requires the following tools and packages to build:

* `cargo` and `rustc`
* A C compiler [for your platform](https://github.com/rust-lang/cc-rs#compile-time-requirements)
* `git` (to clone this repository)

**Other requirements:**

The following are optional based on your platform and needs.

* `pkg-config` — This is used to help locate system packages, such as `libssl` headers/libraries. This may not be required in all cases, such as using vendored OpenSSL, or on Windows.
* OpenSSL — Only needed on Unix-like systems and only if the `vendored-openssl` Cargo feature is not used.

  This requires the development headers, which can be obtained from the `libssl-dev` package on Ubuntu or `openssl-devel` with apk or yum or the `openssl` package from Homebrew on macOS.

  If using the `vendored-openssl` Cargo feature, then a static copy of OpenSSL will be built from source instead of using the system OpenSSL.
  This may require additional tools such as `perl` and `make`.

  On macOS, common installation directories from Homebrew, MacPorts, or pkgsrc will be checked. Otherwise it will fall back to `pkg-config`.

  On Windows, the system-provided Schannel will be used instead.

  LibreSSL is also supported.

**Optional system libraries:**

The build will automatically use vendored versions of the following libraries. However, if they are provided by the system and can be found with `pkg-config`, then the system libraries will be used instead:

* [`libcurl`](https://curl.se/libcurl/) — Used for network transfers.
* [`libgit2`](https://libgit2.org/) — Used for fetching git dependencies.
* [`libssh2`](https://www.libssh2.org/) — Used for SSH access to git repositories.
* [`libz`](https://zlib.net/) (AKA zlib) — Used by the above C libraries for data compression. (Rust code uses [`zlib-rs`](https://github.com/trifectatechfoundation/zlib-rs) instead.)

It is recommended to use the vendored versions as they are the versions that are tested to work with Cargo.

### Compiling

First, you'll want to check out this repository

```
git clone https://github.com/rust-lang/cargo.git
cd cargo
```

With `cargo` already installed, you can simply run:

```
cargo build --release
```

## Adding new subcommands to Cargo

Cargo is designed to be extensible with new subcommands without having to modify
Cargo itself. See [the Wiki page][third-party-subcommands] for more details and
a list of known community-developed subcommands.

[third-party-subcommands]: https://github.com/rust-lang/cargo/wiki/Third-party-cargo-subcommands


## Releases

Cargo releases coincide with Rust releases.
High level release notes are available as part of [Rust's release notes][rel].
Detailed release notes are available in the [changelog].

[rel]: https://github.com/rust-lang/rust/blob/master/RELEASES.md
[changelog]: https://doc.rust-lang.org/nightly/cargo/CHANGELOG.html

## Reporting issues

Found a bug? We'd love to know about it!

Please report all issues on the GitHub [issue tracker][issues].

[issues]: https://github.com/rust-lang/cargo/issues

## Contributing

See the **[Cargo Contributor Guide]** for a complete introduction
to contributing to Cargo.

Code documentation: <https://doc.rust-lang.org/nightly/nightly-rustc/cargo/>

## License

Cargo is primarily distributed under the terms of both the MIT license
and the Apache License (Version 2.0).

See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for details.

### Third party software

This product includes software developed by the OpenSSL Project
for use in the OpenSSL Toolkit (https://www.openssl.org/).

In binary form, this product includes software that is licensed under the
terms of the GNU General Public License, version 2, with a linking exception,
which can be obtained from the [upstream repository][1].

See [LICENSE-THIRD-PARTY](LICENSE-THIRD-PARTY) for details.

[1]: https://github.com/libgit2/libgit2

