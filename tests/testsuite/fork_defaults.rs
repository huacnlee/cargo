//! Tests for this fork's defaults: fixed target and build directories and
//! the build-dir garbage collector. The rest of the testsuite runs with
//! `__CARGO_FORK_DEFAULTS_OPT_OUT=1`; these tests remove it.

use std::fs;
use std::path::PathBuf;

use crate::prelude::*;
use cargo_test_support::registry::Package;
use cargo_test_support::{
    Execs, Project, basic_lib_manifest, basic_manifest, git, paths, project, str,
};

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

#[cargo_test]
fn check_and_build_share_registry_dependencies() {
    Package::new("bar", "1.0.0")
        .file("src/lib.rs", "pub fn bar() {}")
        .publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.1.0"
            edition = "2015"

            [dependencies]
            bar = "1.0"
            "#,
        )
        .file("src/main.rs", "fn main() { bar::bar(); }")
        .build();

    // `bar` is built, not only checked, so the build below reuses it.
    fork_cargo(&p, "check")
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[LOCKING] 1 package to highest compatible version
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[COMPILING] bar v1.0.0
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
    fork_cargo(&p, "build")
        .with_stderr_data(str![[r#"
[COMPILING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();

    // Opting out checks it again.
    fork_cargo(&p, "check")
        .env("CARGO_CHECK_SHARED_DEPS", "0")
        .with_stderr_data(str![[r#"
[CHECKING] bar v1.0.0
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn registry_dependencies_of_a_patched_crate_are_checked() {
    Package::new("bar", "1.0.0")
        .file("src/lib.rs", "pub struct Bar;")
        .publish();
    Package::new("baz", "1.0.0")
        .dep("bar", "1.0")
        .file("src/lib.rs", "pub fn bar() -> bar::Bar { bar::Bar }")
        .publish();
    Package::new("qux", "1.0.0")
        .file("src/lib.rs", "pub fn qux() {}")
        .publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.1.0"
            edition = "2015"

            [dependencies]
            bar = "1.0"
            baz = "1.0"
            qux = "1.0"

            [patch.crates-io]
            bar = { path = "bar" }
            "#,
        )
        .file(
            "src/lib.rs",
            "pub fn bar() -> bar::Bar { qux::qux(); baz::bar() }",
        )
        .file("bar/Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("bar/src/lib.rs", "pub struct Bar;")
        .build();

    // `baz` links the patched `bar`, so it is checked against the same `bar`
    // as `foo`; `qux` links nothing local and is still built.
    fork_cargo(&p, "check")
        .with_stderr_data(
            str![[r#"
[UPDATING] `dummy-registry` index
[LOCKING] 3 packages to highest compatible versions
[DOWNLOADING] crates ...
[DOWNLOADED] baz v1.0.0 (registry `dummy-registry`)
[DOWNLOADED] qux v1.0.0 (registry `dummy-registry`)
[CHECKING] bar v1.0.0 ([ROOT]/foo/bar)
[CHECKING] baz v1.0.0
[COMPILING] qux v1.0.0
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]]
            .unordered(),
        )
        .run();
    fork_cargo(&p, "build")
        .with_stderr_data(
            str![[r#"
[COMPILING] bar v1.0.0 ([ROOT]/foo/bar)
[COMPILING] baz v1.0.0
[COMPILING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]]
            .unordered(),
        )
        .run();
}

/// A git repository `foo` with a library member `bar`, and a worktree of it.
fn repo_with_worktree(main: &str) -> (Project, PathBuf) {
    let p = git::new("foo", |p| {
        p.file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            version = "0.1.0"
            edition = "2015"

            [dependencies]
            bar = { path = "bar" }

            [workspace]
            "#,
        )
        .file("src/main.rs", main)
        .file("bar/Cargo.toml", &basic_lib_manifest("bar"))
        .file("bar/src/lib.rs", "pub fn bar() -> u32 { 1 }")
        .file("bar/tests/t.rs", "#[test] fn t() {}")
    });
    let repo = git2::Repository::open(p.root()).unwrap();
    let worktree = paths::root().join("wt");
    repo.worktree("wt", &worktree, Some(&git2::WorktreeAddOptions::new()))
        .unwrap();
    (p, worktree)
}

#[cargo_test]
fn worktrees_share_workspace_units() {
    let (p, wt) = repo_with_worktree("fn main() { println!(\"{}\", bar::bar()); }");
    fork_cargo(&p, "build")
        .with_stderr_data(str![[r#"
[COMPILING] bar v0.5.0 ([ROOT]/foo/bar)
[COMPILING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();

    // Same files: nothing to build.
    fork_cargo(&p, "build")
        .cwd(&wt)
        .with_stderr_data(str![[r#"
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();

    // An edit in one worktree leaves the other one's units alone.
    fs::write(wt.join("bar/src/lib.rs"), "pub fn bar() -> u32 { 2 }").unwrap();
    fork_cargo(&p, "run")
        .cwd(&wt)
        .with_stdout_data("2\n")
        .with_stderr_data(str![[r#"
[COMPILING] bar v0.5.0 ([ROOT]/wt/bar)
[COMPILING] foo v0.1.0 ([ROOT]/wt)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .run();
    fork_cargo(&p, "run")
        .with_stdout_data("1\n")
        .with_stderr_data(str![[r#"
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .run();

    // Going back to earlier files reuses their units.
    fs::write(wt.join("bar/src/lib.rs"), "pub fn bar() -> u32 { 1 }").unwrap();
    fork_cargo(&p, "run")
        .cwd(&wt)
        .with_stdout_data("1\n")
        .with_stderr_data(str![[r#"
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .run();

    // Editing a test only rebuilds that test.
    fs::write(wt.join("bar/tests/t.rs"), "#[test] fn t2() {}").unwrap();
    fork_cargo(&p, "build")
        .cwd(&wt)
        .with_stderr_data(str![[r#"
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();

    // Opting out keeps the checkout in the hash.
    fork_cargo(&p, "build")
        .cwd(&wt)
        .env("CARGO_SHARE_WORKSPACE_UNITS", "0")
        .with_stderr_data(str![[r#"
[COMPILING] bar v0.5.0 ([ROOT]/wt/bar)
[COMPILING] foo v0.1.0 ([ROOT]/wt)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn worktrees_share_incremental_caches() {
    let (p, wt) = repo_with_worktree("fn main() { bar::bar(); }");
    fork_cargo(&p, "build").env("CARGO_INCREMENTAL", "1").run();
    fs::write(wt.join("bar/src/lib.rs"), "pub fn bar() -> u32 { 2 }").unwrap();
    let mut build = fork_cargo(&p, "build");
    build
        .cwd(&wt)
        .env("CARGO_INCREMENTAL", "1")
        .arg("-v")
        .with_stderr_contains("[..] -Zremap-cwd-prefix=.[..]");
    if cfg!(unix) {
        // rustc writes through a link that stays the same across edits.
        build.with_stderr_contains(
            "[..]--out-dir [ROOT]/home/.cargo/targets/foo-[..]/debug/.out/bar-[..]",
        );
    }
    if !cargo_test_support::is_nightly() {
        build.with_stderr_contains("[..] -Zallow-features= [..]");
    }
    build.run();

    let bar_dirs = fs::read_dir(shared_build_dir().join("debug/incremental"))
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_str().unwrap().starts_with("bar-")
        })
        .count();
    assert_eq!(bar_dirs, 1);
}

#[cargo_test]
fn stable_crates_cannot_use_features() {
    if cargo_test_support::is_nightly() {
        return;
    }
    let (p, _wt) = repo_with_worktree("#![feature(rustc_attrs)] fn main() {}");
    fork_cargo(&p, "build")
        .with_status(101)
        .with_stderr_contains(
            "[..]the feature `rustc_attrs` is not in the list of allowed features",
        )
        .run();
}

#[cargo_test]
fn crates_reading_their_path_are_not_shared() {
    let (p, wt) = repo_with_worktree(
        "fn main() { bar::bar(); println!(\"{}\", env!(\"CARGO_MANIFEST_DIR\")); }",
    );
    fork_cargo(&p, "run")
        .with_stdout_data(str![[r#"
[ROOT]/foo

"#]])
        .run();
    fork_cargo(&p, "run")
        .cwd(&wt)
        .with_stdout_data(str![[r#"
[ROOT]/wt

"#]])
        .with_stderr_data(str![[r#"
[COMPILING] foo v0.1.0 ([ROOT]/wt)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .run();
}
