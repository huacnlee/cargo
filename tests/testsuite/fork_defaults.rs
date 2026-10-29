//! Tests for this fork's defaults: fixed target and build directories and
//! the build-dir garbage collector. The rest of the testsuite runs with
//! `__CARGO_FORK_DEFAULTS_OPT_OUT=1`; these tests remove it.

use std::fs;
use std::path::PathBuf;

use crate::prelude::*;
use cargo_test_support::{Execs, Project, paths, project, str};

fn fork_cargo(p: &Project, args: &str) -> Execs {
    let mut execs = p.cargo(args);
    execs.env_remove("__CARGO_FORK_DEFAULTS_OPT_OUT");
    execs
}

/// The shared build-dir of the `foo` project.
fn shared_build_dir() -> PathBuf {
    let targets = paths::cargo_home().join("targets");
    let mut dirs = fs::read_dir(&targets)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("foo-"))
        .collect::<Vec<_>>();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}

#[cargo_test]
fn target_dir_cannot_be_moved() {
    let p = project()
        .file("src/main.rs", "fn main() {}")
        .file(
            ".cargo/config.toml",
            r#"
            [build]
            build-dir = "config-build-dir"
            "#,
        )
        .build();

    fork_cargo(&p, "build --target-dir flag-dir")
        .env("CARGO_TARGET_DIR", "env-dir")
        .with_stderr_data(str![[r#"
[WARNING] ignoring the configured target directory (--target-dir, CARGO_TARGET_DIR, build.target-dir); this cargo always uses `./target`
[WARNING] ignoring the configured build directory (build.build-dir); this cargo always uses `~/.cargo/targets`
[COMPILING] foo v0.0.1 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();

    assert!(p.bin("foo").is_file());
    for dir in ["flag-dir", "env-dir", "config-build-dir"] {
        assert!(!p.root().join(dir).exists(), "{dir} was created");
    }
    assert!(shared_build_dir().join("debug/build").is_dir());
    assert!(!p.root().join("target/debug/build").exists());
}

#[cargo_test]
fn gc_keeps_two_incremental_dirs_per_crate() {
    let p = project().file("src/main.rs", "fn main() {}").build();
    fork_cargo(&p, "build").run();

    let incremental = shared_build_dir().join("debug/incremental");
    for id in ["a", "b", "c", "d"] {
        let dir = incremental.join(format!("bar-{id}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("s-session"), "").unwrap();
    }
    fork_cargo(&p, "build")
        .env("CARGO_BUILD_GC_FREQUENCY", "always")
        .run();

    let left = fs::read_dir(&incremental)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_str().unwrap().starts_with("bar-")
        })
        .count();
    assert_eq!(left, 2);
}

#[cargo_test]
fn gc_records_build_sizes() {
    let p = project().file("src/main.rs", "fn main() {}").build();
    fork_cargo(&p, "build").run();
    fork_cargo(&p, "build").run();

    let sizes = fs::read_to_string(shared_build_dir().join(".cargo-gc-sizes")).unwrap();
    assert_eq!(sizes.lines().count(), 2, "{sizes}");
}
