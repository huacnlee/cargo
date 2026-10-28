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
2.6 GB. The shared build directory stays under its size limit, 10 GiB or twice
the last build, whichever is larger.

| Rebuild after editing a core crate | Cargo | This fork |
|---|---:|---:|
| Stable | 19.1 s | 19.9 s |
| Nightly, Cranelift | | 21.4 s |

Rebuilding after an edit is not faster on this workspace, on stable or with
Cranelift on nightly.

## What changes

- **One build directory per project.** Intermediate files go to
  `~/.cargo/targets/<name>-<hash>/`; the hash comes from the repository's main
  `.git` directory, so every worktree lands there wherever it is created.
  Worktrees reuse registry and git dependencies and build only their own
  crates. Binaries still appear in `./target`.
- **It cleans itself.** After a build, at most once an hour, units unused for
  7 days, stale incremental caches and old-layout leftovers are removed, then
  the least recently used units until the project fits the larger of 10 GiB
  and twice the last build. Units used in the last hour are never removed.
- **Worktrees build at the same time.** Fine-grained locking is on.
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
max-unused = "7 days"
frequency = "1 hour"  # or "always", "never"
enabled = true
```

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

