/// Tests for the pure helpers behind [`super::outdated_inputs`] and for
/// [`super::diff_locks`]: rebuilding a flake reference from a `flake.lock`
/// node's `original` block, picking the revision to compare, diffing two
/// lockfiles, and deserializing the lockfile shape itself. No test here shells
/// out to `nix`, since that would require network access.
///
/// ```
/// cargo test --features system-update update
/// ```
use super::{
    BuildCommand, FlakeLock, LockedRef, OriginalRef, compare_rev, flake_ref_from_original,
    stage_head_tree,
};

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

fn lock_json(inputs: &[(&str, &str, &str, u64)]) -> String {
    let mut nodes = vec![format!(
        r#""root": {{ "inputs": {{ {} }} }}"#,
        inputs
            .iter()
            .map(|(name, _, _, _)| format!(r#""{name}": "{name}""#))
            .collect::<Vec<_>>()
            .join(", ")
    )];
    for (name, kind, rev, last_modified) in inputs {
        nodes.push(format!(
            r#""{name}": {{
                "locked": {{ "rev": "{rev}", "lastModified": {last_modified} }},
                "original": {{ "type": "{kind}", "owner": "Modulix-OS", "repo": "{name}" }}
            }}"#
        ));
    }
    format!(
        r#"{{ "root": "root", "nodes": {{ {} }} }}"#,
        nodes.join(", ")
    )
}

#[test]
fn diff_locks_reports_nothing_when_identical() {
    let lock = lock_json(&[
        ("mxpkgs", "github", "aaa", 10),
        ("nixpkgs", "github", "bbb", 20),
    ]);
    let diff = super::diff_locks(&lock, &lock).expect("diff failed");
    assert!(diff.is_empty());
}

#[test]
fn diff_locks_reports_the_moved_input_only() {
    let old = lock_json(&[
        ("mxpkgs", "github", "aaa", 10),
        ("nixpkgs", "github", "bbb", 20),
    ]);
    let new = lock_json(&[
        ("mxpkgs", "github", "ccc", 30),
        ("nixpkgs", "github", "bbb", 20),
    ]);

    let diff = super::diff_locks(&old, &new).expect("diff failed");
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].name, "mxpkgs");
    assert_eq!(diff[0].current_rev, "aaa");
    assert_eq!(diff[0].new_rev, "ccc");
    assert_eq!(diff[0].last_modified, 30);
}

#[test]
fn diff_locks_skips_path_inputs() {
    let old = lock_json(&[("local", "path", "aaa", 10)]);
    let new = lock_json(&[("local", "path", "ccc", 30)]);
    let diff = super::diff_locks(&old, &new).expect("diff failed");
    assert!(diff.is_empty());
}

#[test]
fn diff_locks_skips_inputs_absent_from_the_old_lock() {
    let old = lock_json(&[("mxpkgs", "github", "aaa", 10)]);
    let new = lock_json(&[
        ("mxpkgs", "github", "aaa", 10),
        ("nixpkgs", "github", "bbb", 20),
    ]);
    let diff = super::diff_locks(&old, &new).expect("diff failed");
    assert!(diff.is_empty());
}

#[test]
fn diff_locks_rejects_invalid_json() {
    let lock = lock_json(&[("mxpkgs", "github", "aaa", 10)]);
    assert!(super::diff_locks("not json", &lock).is_err());
    assert!(super::diff_locks(&lock, "not json").is_err());
}

#[test]
fn build_command_build_is_never_substituted() {
    assert_eq!(BuildCommand::Build.as_str(), "build");
}

#[test]
fn stage_head_tree_writes_tracked_files_only() {
    let root = std::env::temp_dir().join(format!(
        "mx-stage-head-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let src = root.join("src");
    let dst = root.join("dst");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(src.join("nested")).unwrap();

    let repo = git2::Repository::init(&src).unwrap();
    std::fs::write(src.join("flake.nix"), "{}").unwrap();
    std::fs::write(src.join("flake.lock"), "old").unwrap();
    std::fs::write(src.join("nested").join("configuration.nix"), "{}").unwrap();

    let mut index = repo.index().unwrap();
    for tracked in ["flake.nix", "flake.lock", "nested/configuration.nix"] {
        index.add_path(std::path::Path::new(tracked)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let who = git2::Signature::now("test", "test@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &who, &who, "initial", &tree, &[])
        .unwrap();

    // Untracked, and the reason this uses HEAD rather than a directory copy:
    // `nix` reads the real repository as `git+file://` and never sees this.
    std::fs::create_dir_all(src.join(".cache")).unwrap();
    std::fs::write(src.join(".cache").join("index.bin"), "huge").unwrap();

    stage_head_tree(&src, &dst).unwrap();

    assert!(dst.join("flake.nix").is_file());
    assert!(dst.join("nested").join("configuration.nix").is_file());
    assert_eq!(
        std::fs::read_to_string(dst.join("flake.lock")).unwrap(),
        "old"
    );
    assert!(!dst.join(".cache").exists());
    assert!(!dst.join(".git").exists());

    // The source repository is left alone - working directory and HEAD both.
    assert!(src.join(".cache").join("index.bin").is_file());
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap(),
        "initial"
    );

    let _ = std::fs::remove_dir_all(&root);
}
