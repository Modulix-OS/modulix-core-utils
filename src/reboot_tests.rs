//! Unit tests for [`super`].
//!
//! Only [`super::required_between`] is covered: it is the part that decides,
//! and the only part that can be exercised without a real NixOS `/run` or a
//! `nix` process. The temporary trees stand in for `/run/booted-system` and
//! `/run/current-system`.

use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::Path;

use tempfile::TempDir;

use super::required_between;

/// Creates `dir/name` as a symlink to `target`, as NixOS does for the
/// boot-critical entries of a system closure.
///
/// # Parameters
/// * `dir` - directory to create the link in; must exist.
/// * `name` - link name, e.g. `"kernel"`.
/// * `target` - what the link points at; does not have to exist.
///
/// # Panics
/// If the link cannot be created.
fn link(dir: &Path, name: &str, target: &str) {
    unix_fs::symlink(target, dir.join(name)).expect("symlink");
}

/// Fills `dir` with the three boot-critical links, all pointing at paths
/// derived from `generation`.
///
/// # Parameters
/// * `dir` - directory to fill; must exist.
/// * `generation` - discriminant baked into every target, so two calls with
///   different values produce two fully distinct closures.
fn fill(dir: &Path, generation: &str) {
    link(
        dir,
        "kernel",
        &format!("/nix/store/{generation}-linux/bzImage"),
    );
    link(
        dir,
        "kernel-modules",
        &format!("/nix/store/{generation}-modules"),
    );
    link(
        dir,
        "initrd",
        &format!("/nix/store/{generation}-initrd/initrd"),
    );
}

#[test]
fn identical_closures_need_no_reboot() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");
    fill(booted.path(), "aaa");
    fill(current.path(), "aaa");

    assert!(!required_between(booted.path(), current.path()));
}

#[test]
fn a_new_kernel_needs_a_reboot() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");
    fill(booted.path(), "aaa");
    fill(current.path(), "aaa");
    fs::remove_file(current.path().join("kernel")).expect("remove");
    link(current.path(), "kernel", "/nix/store/bbb-linux/bzImage");

    assert!(required_between(booted.path(), current.path()));
}

#[test]
fn new_modules_alone_need_a_reboot() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");
    fill(booted.path(), "aaa");
    fill(current.path(), "aaa");
    fs::remove_file(current.path().join("kernel-modules")).expect("remove");
    link(current.path(), "kernel-modules", "/nix/store/bbb-modules");

    assert!(required_between(booted.path(), current.path()));
}

#[test]
fn an_entry_on_one_side_only_needs_a_reboot() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");
    fill(booted.path(), "aaa");
    fill(current.path(), "aaa");
    fs::remove_file(current.path().join("initrd")).expect("remove");

    assert!(required_between(booted.path(), current.path()));
}

#[test]
fn nothing_on_either_side_needs_no_reboot() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");

    assert!(!required_between(booted.path(), current.path()));
}

#[test]
fn a_non_symlink_entry_counts_as_absent() {
    let booted = TempDir::new().expect("tempdir");
    let current = TempDir::new().expect("tempdir");
    fs::write(booted.path().join("kernel"), b"not a symlink").expect("write");
    fs::write(current.path().join("kernel"), b"not a symlink").expect("write");

    assert!(!required_between(booted.path(), current.path()));
}
