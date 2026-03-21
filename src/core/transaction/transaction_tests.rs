/// Tests for [`Transaction`].
///
/// # Structure
/// - `unit`           – No I/O or Git (pure logic).
/// - `integration`    – Real temporary Git repository.
/// - `no_regression` – Edge cases and historical bugs.
/// - `stash`          – Auto-stash behaviour when the repo is dirty on `begin`.
///
/// # Path convention
/// `NixFile::new(repo_path, relative_path)` concatenates directly:
/// `repo_path + relative_path`. Therefore `repo_path` **must** end with `/`
/// and `relative_path` must **not** start with `/` so that the resulting
/// absolute path and the git-relative path (used for `git add` / status) are
/// both correct.
///
/// All helpers in this file append a trailing `/` to the temp-dir path.
///
/// # Dependencies
/// ```toml
/// [dev-dependencies]
/// tempfile = "3"
/// ```
use super::{BuildCommand, Transaction};
use crate::mx;
use std::fs;
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the temp-dir path **with a trailing `/`** (required by NixFile).
fn repo_path(dir: &TempDir) -> String {
    format!("{}/", dir.path().to_str().unwrap())
}

/// Initialises a Git repo with a first commit containing `configuration.nix`.
/// Returns `(TempDir, git2::Repository)`.  `TempDir` must stay alive for the
/// duration of the test.
fn setup_repo() -> (TempDir, git2::Repository) {
    let dir = TempDir::new().expect("failed to create temporary directory");

    let repo = git2::Repository::init(dir.path()).expect("git init failed");

    fs::write(
        dir.path().join("configuration.nix"),
        "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
    )
    .expect("failed to write configuration.nix");

    commit_all(&repo, "init");
    (dir, repo)
}

/// Stages and commits every file currently in the working tree.
fn commit_all(repo: &git2::Repository, message: &str) {
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let parent = repo.head().and_then(|h| h.peel_to_commit()).ok();
    {
        let tree = repo.find_tree(tree_oid).unwrap();
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests – no I/O
// ─────────────────────────────────────────────────────────────────────────────
mod unit {
    use super::*;

    /// `new` always succeeds (no I/O performed).
    #[test]
    fn new_always_succeeds() {
        assert!(Transaction::new("/some/path/", "description", BuildCommand::Install).is_ok());
    }

    /// After `new`, the transaction is not active.
    #[test]
    fn new_transaction_not_begun() {
        let t = Transaction::new("/some/path/", "desc", BuildCommand::Install).unwrap();
        assert!(!t.as_begin());
    }

    /// `new` accepts empty strings without panicking.
    #[test]
    fn new_accepts_empty_strings() {
        assert!(Transaction::new("", "", BuildCommand::Install).is_ok());
    }

    /// `add_file` succeeds before `begin`.
    #[test]
    fn add_file_before_begin_ok() {
        let mut t = Transaction::new("/path/", "desc", BuildCommand::Install).unwrap();
        assert!(t.add_file("some.nix").is_ok());
    }

    /// Multiple `add_file` calls before `begin` are all accepted.
    #[test]
    fn add_file_multiple_before_begin_ok() {
        let mut t = Transaction::new("/path/", "desc", BuildCommand::Install).unwrap();
        assert!(t.add_file("a.nix").is_ok());
        assert!(t.add_file("b.nix").is_ok());
        assert!(t.add_file("c.nix").is_ok());
    }

    /// `get_file` without `begin` returns `TransactionNotBegin`.
    #[test]
    fn get_file_without_begin_errors() {
        let mut t = Transaction::new("/path/", "desc", BuildCommand::Install).unwrap();
        assert!(matches!(
            t.get_file("configuration.nix"),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// `rollback` without `begin` returns `TransactionNotBegin`.
    #[test]
    fn rollback_without_begin_errors() {
        let mut t = Transaction::new("/path/", "desc", BuildCommand::Install).unwrap();
        assert!(matches!(
            t.rollback(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// `commit` without `begin` returns an error.
    #[test]
    fn commit_without_begin_errors() {
        let mut t = Transaction::new("/path/", "desc", BuildCommand::Install).unwrap();
        assert!(t.commit().is_err());
    }

    /// In debug mode all `BuildCommand` variants return `"build-vm"`.
    #[test]
    #[cfg(debug_assertions)]
    fn build_command_debug_all_return_build_vm() {
        assert_eq!(BuildCommand::Switch.as_str(), "build-vm");
        assert_eq!(BuildCommand::Boot.as_str(), "build-vm");
        assert_eq!(BuildCommand::Install.as_str(), "build-vm");
    }

    /// In release mode each variant returns its expected string.
    #[test]
    #[cfg(not(debug_assertions))]
    fn build_command_release_correct_values() {
        assert_eq!(BuildCommand::Switch.as_str(), "switch");
        assert_eq!(BuildCommand::Boot.as_str(), "boot");
        assert_eq!(BuildCommand::Install.as_str(), "");
    }

    /// `BuildCommand` is clonable without panicking.
    #[test]
    fn build_command_clone_ok() {
        let _ = BuildCommand::Switch.clone();
        let _ = BuildCommand::Boot.clone();
        let _ = BuildCommand::Install.clone();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests – real Git repository
// ─────────────────────────────────────────────────────────────────────────────
mod integration {
    use super::*;

    // ── begin ─────────────────────────────────────────────────────────────────

    /// `begin` succeeds on a clean Git repo.
    #[test]
    fn begin_on_clean_repo_ok() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        assert!(t.begin().is_ok());
        assert!(t.as_begin());
        t.rollback().unwrap();
    }

    /// `begin` fails when the directory is not a Git repository.
    #[test]
    fn begin_not_a_git_repo_errors() {
        let dir = TempDir::new().unwrap();
        // create configuration.nix so NixFile::begin does not fail first
        fs::write(dir.path().join("configuration.nix"), "").unwrap();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        assert!(matches!(t.begin(), Err(mx::ErrorKind::GitError(_))));
    }

    /// After `begin`, `configuration.nix` is accessible via `get_file`.
    #[test]
    fn begin_makes_configuration_nix_available() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        assert!(t.get_file("configuration.nix").is_ok());
        t.rollback().unwrap();
    }

    /// `add_file` after `begin` returns `TransactionAlreadyBegin`.
    #[test]
    fn add_file_after_begin_errors() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        assert!(matches!(
            t.add_file("new.nix"),
            Err(mx::ErrorKind::TransactionAlreadyBegin)
        ));
        t.rollback().unwrap();
    }

    // ── get_file ──────────────────────────────────────────────────────────────

    /// `get_file` on an unregistered path returns `FileNotFound`.
    #[test]
    fn get_file_unknown_path_errors() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        assert!(matches!(
            t.get_file("nonexistent.nix"),
            Err(mx::ErrorKind::FileNotFound)
        ));
        t.rollback().unwrap();
    }

    // ── rollback ──────────────────────────────────────────────────────────────

    /// `rollback` after `begin` succeeds and ends the transaction.
    #[test]
    fn rollback_after_begin_ok() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        assert!(t.rollback().is_ok());
        assert!(!t.as_begin());
    }

    /// After `rollback`, `get_file` returns `TransactionNotBegin`.
    #[test]
    fn rollback_ends_transaction() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.rollback().unwrap();
        assert!(matches!(
            t.get_file("configuration.nix"),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// `rollback` restores the original file content on disk.
    #[test]
    fn rollback_restores_file_content() {
        let (dir, _repo) = setup_repo();
        let config_path = dir.path().join("configuration.nix");
        let original = fs::read_to_string(&config_path).unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        *t.get_file("configuration.nix")
            .unwrap()
            .get_mut_file_content()
            .unwrap() = String::from("# modified content\n");
        t.rollback().unwrap();

        assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    }

    // ── commit ────────────────────────────────────────────────────────────────

    /// A commit with no diff does not create a new Git commit.
    #[test]
    fn commit_no_diff_does_not_create_git_commit() {
        let (dir, repo) = setup_repo();
        let commit_before = repo.head().unwrap().peel_to_commit().unwrap().id();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.commit().unwrap();

        assert_eq!(
            repo.head().unwrap().peel_to_commit().unwrap().id(),
            commit_before
        );
    }

    /// After `commit`, the transaction is closed.
    #[test]
    fn commit_ends_transaction() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.commit().unwrap();
        assert!(!t.as_begin());
    }

    // ── Dynamically created files ─────────────────────────────────────────────

    /// A missing file is created during `begin` and removed by `rollback`.
    #[test]
    fn begin_creates_missing_file_rollback_removes_it() {
        let (dir, _repo) = setup_repo();
        let path = repo_path(&dir);
        // The NixFile path is repo_path + "new_module.nix" (direct concat)
        let new_file = std::path::Path::new(&path).join("new_module.nix");

        assert!(!new_file.exists());

        let mut t = Transaction::new(&path, "desc", BuildCommand::Install).unwrap();
        t.add_file("new_module.nix").unwrap();
        t.begin().unwrap();

        assert!(
            new_file.exists(),
            "begin should have created the missing file"
        );

        t.rollback().unwrap();

        assert!(
            !new_file.exists(),
            "rollback should have removed the created file"
        );
    }

    /// A file created by `begin` contains the NixOS skeleton.
    #[test]
    fn begin_created_file_has_nix_skeleton() {
        let (dir, _repo) = setup_repo();
        let path = repo_path(&dir);

        let mut t = Transaction::new(&path, "desc", BuildCommand::Install).unwrap();
        t.add_file("new_module.nix").unwrap();
        t.begin().unwrap();

        let content = t
            .get_file("new_module.nix")
            .unwrap()
            .get_file_content()
            .unwrap()
            .clone();

        t.rollback().unwrap();

        assert_eq!(content, "{config, lib, pkgs, ...}:\n{\n}\n");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Non-regression tests
// ─────────────────────────────────────────────────────────────────────────────
mod no_regression {
    use super::*;

    /// Two successive transactions on the same repo both open without error.
    ///
    /// Regression: the first transaction sometimes left the repo locked.
    #[test]
    fn two_successive_transactions_on_same_repo() {
        let (dir, _repo) = setup_repo();
        let path = repo_path(&dir);

        let mut t1 = Transaction::new(&path, "tx1", BuildCommand::Install).unwrap();
        t1.begin().unwrap();
        t1.rollback().unwrap();

        let mut t2 = Transaction::new(&path, "tx2", BuildCommand::Install).unwrap();
        assert!(
            t2.begin().is_ok(),
            "second transaction should open without error"
        );
        t2.rollback().unwrap();
    }

    /// Double `rollback` returns `TransactionNotBegin` without panicking.
    ///
    /// Regression: second rollback could `unwrap` on `None`.
    #[test]
    fn double_rollback_does_not_panic() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.rollback().unwrap();

        assert!(matches!(
            t.rollback(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// Double `commit` returns an error without panicking.
    ///
    /// Regression: second commit could `unwrap` on `None`.
    #[test]
    fn double_commit_does_not_panic() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.commit().unwrap();

        assert!(t.commit().is_err(), "second commit should return an error");
    }

    /// `rollback` after `commit` returns `TransactionNotBegin` without panicking.
    #[test]
    fn rollback_after_commit_does_not_panic() {
        let (dir, _repo) = setup_repo();
        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.commit().unwrap();

        assert!(matches!(
            t.rollback(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// Rolled-back content is not visible in the next transaction.
    ///
    /// Regression: `file_content` was not reset by `close`, leaking content
    /// between transactions.
    #[test]
    fn rolled_back_content_not_visible_in_next_transaction() {
        let (dir, _repo) = setup_repo();
        let path = repo_path(&dir);

        let mut t1 = Transaction::new(&path, "tx1", BuildCommand::Install).unwrap();
        t1.begin().unwrap();
        *t1.get_file("configuration.nix")
            .unwrap()
            .get_mut_file_content()
            .unwrap() = String::from("# poison\n");
        t1.rollback().unwrap();

        let mut t2 = Transaction::new(&path, "tx2", BuildCommand::Install).unwrap();
        t2.begin().unwrap();
        let content = t2
            .get_file("configuration.nix")
            .unwrap()
            .get_file_content()
            .unwrap()
            .clone();
        t2.rollback().unwrap();

        assert!(
            !content.contains("poison"),
            "rolled-back content must not leak into the next transaction"
        );
    }

    /// `begin` on an empty repo (no commits) succeeds without error.
    ///
    /// Regression: `head()` on an empty repo returned `UnbornBranch` which
    /// was not correctly distinguished from a real Git error.
    #[test]
    fn begin_on_empty_repo_ok() {
        let dir = TempDir::new().unwrap();

        git2::Repository::init(dir.path()).unwrap();
        fs::write(
            dir.path().join("configuration.nix"),
            "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
        )
        .unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        assert!(
            t.begin().is_ok(),
            "begin should succeed on a repo with no commits"
        );
        t.rollback().unwrap();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stash tests
// ─────────────────────────────────────────────────────────────────────────────
mod stash {
    use super::*;

    /// `begin` on a repo with untracked files stashes them instead of failing.
    #[test]
    fn begin_stashes_untracked_files() {
        let (dir, repo) = setup_repo();

        // Create an untracked file — would have caused GitNotCommitted before
        fs::write(dir.path().join("untracked.nix"), "untracked content").unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        assert!(t.begin().is_ok(), "begin should stash untracked files");

        // The untracked file should be invisible during the transaction
        let statuses = repo
            .statuses(Some(git2::StatusOptions::new().include_untracked(true)))
            .unwrap();
        // Only files opened by the transaction (configuration.nix) may appear
        assert!(
            statuses
                .iter()
                .all(|s| s.path() == Some("configuration.nix")
                    || s.status() == git2::Status::CURRENT),
            "untracked file should be stashed away during the transaction"
        );

        t.rollback().unwrap();
    }

    /// After `rollback`, stashed files are restored to the working tree.
    #[test]
    fn rollback_restores_stash() {
        let (dir, _repo) = setup_repo();
        let stashed_file = dir.path().join("stashed.nix");

        fs::write(&stashed_file, "stashed content").unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();

        assert!(
            !stashed_file.exists(),
            "stashed file should not be visible during the transaction"
        );

        t.rollback().unwrap();

        assert!(
            stashed_file.exists(),
            "stashed file should be restored after rollback"
        );
        assert_eq!(
            fs::read_to_string(&stashed_file).unwrap(),
            "stashed content"
        );
    }

    /// After `commit`, stashed files are restored to the working tree.
    #[test]
    fn commit_restores_stash() {
        let (dir, _repo) = setup_repo();
        let stashed_file = dir.path().join("stashed.nix");

        fs::write(&stashed_file, "stashed content").unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        t.begin().unwrap();
        t.commit().unwrap();

        assert!(
            stashed_file.exists(),
            "stashed file should be restored after commit"
        );
        assert_eq!(
            fs::read_to_string(&stashed_file).unwrap(),
            "stashed content"
        );
    }

    /// `begin` on a dirty repo with staged changes stashes them correctly.
    #[test]
    fn begin_stashes_staged_changes() {
        let (dir, repo) = setup_repo();

        // Stage a change to configuration.nix without committing
        fs::write(
            dir.path().join("extra.nix"),
            "{config, lib, pkgs, ...}:\n{\n}\n",
        )
        .unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("extra.nix")).unwrap();
        index.write().unwrap();

        let mut t = Transaction::new(&repo_path(&dir), "desc", BuildCommand::Install).unwrap();
        assert!(
            t.begin().is_ok(),
            "begin should stash staged changes without error"
        );

        t.rollback().unwrap();

        // After rollback, staged file is restored
        assert!(
            dir.path().join("extra.nix").exists(),
            "staged file should be restored after rollback"
        );
    }

    /// A clean repo triggers no stash — `stash_oid` stays `None` internally.
    /// Verified indirectly: two consecutive transactions work without leftover stash.
    #[test]
    fn clean_repo_no_stash_side_effects() {
        let (dir, _repo) = setup_repo();
        let path = repo_path(&dir);

        let mut t1 = Transaction::new(&path, "tx1", BuildCommand::Install).unwrap();
        t1.begin().unwrap();
        t1.rollback().unwrap();

        // Second transaction: if a phantom stash existed it would pop the wrong entry
        let mut t2 = Transaction::new(&path, "tx2", BuildCommand::Install).unwrap();
        assert!(t2.begin().is_ok());
        t2.rollback().unwrap();
    }
}
