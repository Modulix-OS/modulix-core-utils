/// Tests for the pure helpers behind [`super::outdated_inputs`]: rebuilding a
/// flake reference from a `flake.lock` node's `original` block, picking the
/// revision to compare, and deserializing the lockfile shape itself. No test
/// here shells out to `nix`, since that would require network access.
///
/// ```
/// cargo test --features system-update update
/// ```
use super::{FlakeLock, LockedRef, OriginalRef, compare_rev, flake_ref_from_original};

fn original(
    kind: &str,
    owner: Option<&str>,
    repo: Option<&str>,
    git_ref: Option<&str>,
    url: Option<&str>,
    id: Option<&str>,
) -> OriginalRef {
    OriginalRef {
        kind: kind.to_string(),
        owner: owner.map(String::from),
        repo: repo.map(String::from),
        git_ref: git_ref.map(String::from),
        url: url.map(String::from),
        id: id.map(String::from),
    }
}

#[test]
fn flake_ref_from_original_github_without_ref() {
    let o = original("github", Some("NixOS"), Some("nixpkgs"), None, None, None);
    assert_eq!(
        flake_ref_from_original(&o).as_deref(),
        Some("github:NixOS/nixpkgs")
    );
}

#[test]
fn flake_ref_from_original_github_with_ref() {
    let o = original(
        "github",
        Some("nix-community"),
        Some("home-manager"),
        Some("release-26.05"),
        None,
        None,
    );
    assert_eq!(
        flake_ref_from_original(&o).as_deref(),
        Some("github:nix-community/home-manager/release-26.05")
    );
}

#[test]
fn flake_ref_from_original_git_with_ref() {
    let o = original(
        "git",
        None,
        None,
        Some("main"),
        Some("https://example.com/repo.git"),
        None,
    );
    assert_eq!(
        flake_ref_from_original(&o).as_deref(),
        Some("git+https://example.com/repo.git?ref=main")
    );
}

#[test]
fn flake_ref_from_original_tarball_is_the_url() {
    let o = original(
        "tarball",
        None,
        None,
        None,
        Some("https://channels.nixos.org/nixos-unstable/nixexprs.tar.xz"),
        None,
    );
    assert_eq!(
        flake_ref_from_original(&o).as_deref(),
        Some("https://channels.nixos.org/nixos-unstable/nixexprs.tar.xz")
    );
}

#[test]
fn flake_ref_from_original_indirect_is_the_id() {
    let o = original("indirect", None, None, None, None, Some("nixpkgs"));
    assert_eq!(flake_ref_from_original(&o).as_deref(), Some("nixpkgs"));
}

#[test]
fn flake_ref_from_original_path_is_none() {
    let o = original("path", None, None, None, None, None);
    assert_eq!(flake_ref_from_original(&o), None);
}

#[test]
fn flake_ref_from_original_missing_required_field_is_none() {
    let o = original("github", Some("NixOS"), None, None, None, None);
    assert_eq!(flake_ref_from_original(&o), None);
}

#[test]
fn compare_rev_prefers_rev_over_nar_hash() {
    let locked = LockedRef {
        rev: Some("abc123".to_string()),
        nar_hash: Some("sha256-xyz".to_string()),
        last_modified: 0,
    };
    assert_eq!(compare_rev(&locked), "abc123");
}

#[test]
fn compare_rev_falls_back_to_nar_hash() {
    let locked = LockedRef {
        rev: None,
        nar_hash: Some("sha256-xyz".to_string()),
        last_modified: 0,
    };
    assert_eq!(compare_rev(&locked), "sha256-xyz");
}

#[test]
fn compare_rev_empty_when_neither_present() {
    let locked = LockedRef {
        rev: None,
        nar_hash: None,
        last_modified: 0,
    };
    assert_eq!(compare_rev(&locked), "");
}

#[test]
fn flake_lock_fixture_deserializes() {
    let content = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/test/flake.lock"))
        .expect("failed to read test fixture");
    let lock: FlakeLock = serde_json::from_str(&content).expect("failed to parse fixture");

    assert_eq!(lock.root, "root");
    let root_node = lock.nodes.get(&lock.root).expect("root node missing");
    assert!(root_node.inputs.contains_key("mxpkgs"));
    assert!(root_node.inputs.contains_key("nixos-hardware"));

    let mxpkgs = lock.nodes.get("mxpkgs").expect("mxpkgs node missing");
    let original = mxpkgs.original.as_ref().expect("mxpkgs has no original");
    assert_eq!(original.kind, "github");
    assert_eq!(original.owner.as_deref(), Some("Modulix-OS"));
    assert_eq!(original.repo.as_deref(), Some("mxpkgs"));

    let locked = mxpkgs.locked.as_ref().expect("mxpkgs has no locked");
    assert_eq!(
        locked.rev.as_deref(),
        Some("04000fb0d1de982e3bee23fee62228e20f84e50b")
    );
}
