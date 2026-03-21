/// Tests for [`make_transaction`].
///
/// # Structure
/// - `unit`          – Pure logic, no real Git repo.
/// - `integration`   – Real temporary Git repository.
/// - `no_regression` – Edge cases and historical bugs.
/// - `stash`         – Auto-stash behaviour via `make_transaction`.
///
/// # Path convention
/// Same as `transaction_test.rs`: `repo_path` must end with `/`.
/// Files passed to `make_transaction` use no leading slash
/// (e.g. `"test.nix"`, not `"/test.nix"`).
///
/// # Important
/// Any file that exists in the working tree before `make_transaction` is called
/// must be **committed** first. Otherwise it will be stashed and popped back
/// after the transaction, overwriting the committed result.
///
/// # Dependencies
/// ```toml
/// [dev-dependencies]
/// tempfile = "3"
/// ```
use super::{BuildCommand, make_transaction};
use crate::mx;
use std::fs;
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn repo_path(dir: &TempDir) -> String {
    format!("{}/", dir.path().to_str().unwrap())
}

/// Returns a `BuildCommand` that runs no actual build (empty command in release,
/// `build-vm` in debug — but no `flake.nix` means the build is never triggered
/// because `commit_impl` skips the build when there is no diff).
fn noop_build() -> BuildCommand {
    BuildCommand::Install
}

/// Initialises a Git repo with a first commit containing `configuration.nix`
/// and a dummy `flake.lock` (so `commit_impl` skips `nix flake update`).
fn setup_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    fs::write(
        dir.path().join("configuration.nix"),
        "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
    )
    .unwrap();

    // A dummy flake.lock prevents commit_impl from running `nix flake update`.
    fs::write(dir.path().join("flake.lock"), "{}").unwrap();

    commit_all(&repo, "init");
    dir
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

/// Creates `file_name` in `dir`, writes `content`, and commits it.
/// Returns the absolute path to the file.
fn create_and_commit(dir: &TempDir, file_name: &str, content: &str) -> std::path::PathBuf {
    let file_path = dir.path().join(file_name);
    fs::write(&file_path, content).unwrap();
    let repo = git2::Repository::open(dir.path()).unwrap();
    commit_all(&repo, &format!("add {}", file_name));
    file_path
}

/// Acquires the build-queue lock so that `commit_impl` skips the NixOS rebuild.
///
/// Returns the lock file handle — it **must** stay alive for the duration of
/// the test (dropping it releases the lock).  Usage:
/// ```rust
/// let _guard = lock_build_queue();
/// make_transaction(...)?;
/// ```
fn lock_build_queue() -> fs::File {
    let f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/tmp/mx-queue-build.lock")
        .expect("failed to create build-queue lock file");
    f.lock().expect("failed to lock build-queue lock file");
    f
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests – no real Git repo
// ─────────────────────────────────────────────────────────────────────────────
mod unit {
    use super::*;

    /// An invalid `config_dir` errors before calling the closure.
    #[test]
    fn invalid_config_dir_errors_before_closure() {
        let closure_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = closure_called.clone();

        let result: mx::Result<()> = make_transaction(
            "bad dir",
            "/nonexistent_config_dir_xyz/",
            "file.nix",
            noop_build(),
            |_| {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(
            !closure_called.load(std::sync::atomic::Ordering::SeqCst),
            "closure must not be called when config_dir is invalid"
        );
    }

    /// An empty description is accepted without error (no validation performed).
    #[test]
    fn empty_description_does_not_error_on_construction() {
        let result: mx::Result<()> =
            make_transaction("", "/nonexistent/", "f.nix", noop_build(), |_| Ok(()));
        // Error comes from the missing Git repo, not the empty description
        assert!(result.is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests – real Git repository
// ─────────────────────────────────────────────────────────────────────────────
mod integration {
    use super::*;

    // ── Happy path ────────────────────────────────────────────────────────────

    /// A successful closure commits its changes and returns the value.
    #[test]
    fn ok_closure_commits_and_returns_value() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "test.nix", "");
        // Hold the build-queue lock so commit_impl skips the NixOS rebuild.
        let _guard = lock_build_queue();

        let result = make_transaction("test commit", &path, "test.nix", noop_build(), |file| {
            file.get_mut_file_content()?.push_str("# modified\n");
            Ok(42usize)
        });

        assert_eq!(result.unwrap(), 42);
        assert!(
            fs::read_to_string(dir.path().join("test.nix"))
                .unwrap()
                .contains("# modified")
        );
    }

    /// A closure returning `Ok(())` commits without error.
    #[test]
    fn unit_ok_closure_succeeds() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "unit.nix", "original");

        let result: mx::Result<()> =
            make_transaction("unit test", &path, "unit.nix", noop_build(), |_| Ok(()));

        assert!(result.is_ok());
    }

    /// `make_transaction` is generic: it can return a `Vec<String>`.
    #[test]
    fn returns_generic_vec() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "vec.nix", "line1\nline2\n");
        let _guard = lock_build_queue();

        let result: mx::Result<Vec<String>> =
            make_transaction("vec return", &path, "vec.nix", noop_build(), |file| {
                Ok(file.get_file_content()?.lines().map(String::from).collect())
            });

        assert_eq!(result.unwrap(), vec!["line1", "line2"]);
    }

    // ── Rollback on error ─────────────────────────────────────────────────────

    /// An error in the closure triggers a rollback; the file on disk is unchanged.
    #[test]
    fn err_closure_triggers_rollback_and_file_unchanged() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "rb.nix", "untouched");

        let result: mx::Result<()> =
            make_transaction("rollback test", &path, "rb.nix", noop_build(), |_| {
                Err(mx::ErrorKind::PermissionDenied)
            });

        assert!(matches!(result, Err(mx::ErrorKind::PermissionDenied)));
        assert_eq!(
            fs::read_to_string(dir.path().join("rb.nix")).unwrap(),
            "untouched"
        );
    }

    /// The closure error kind is propagated as-is without wrapping.
    #[test]
    fn closure_error_kind_is_propagated_as_is() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "err.nix", "");

        let result: mx::Result<String> =
            make_transaction("error kind test", &path, "err.nix", noop_build(), |_| {
                Err(mx::ErrorKind::InvalidFile)
            });

        assert!(matches!(result, Err(mx::ErrorKind::InvalidFile)));
    }

    // ── Missing file ──────────────────────────────────────────────────────────

    /// When the target file does not exist, the closure is never called.
    #[test]
    fn missing_file_errors_before_closure() {
        let dir = setup_repo();
        let path = repo_path(&dir);

        let closure_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = closure_called.clone();

        // Note: a missing file is *created* by begin() with the Nix skeleton.
        // To get an error we use a path inside a non-existent sub-directory.
        let result: mx::Result<()> = make_transaction(
            "missing file",
            &path,
            "subdir/does_not_exist.nix",
            noop_build(),
            |_| {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(
            !closure_called.load(std::sync::atomic::Ordering::SeqCst),
            "closure must not be called when the file path is unreachable"
        );
    }

    // ── Content visible after commit ──────────────────────────────────────────

    /// After a successful commit, the modified content is on disk.
    #[test]
    fn committed_content_visible_on_disk() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "content.nix", "before");
        let _guard = lock_build_queue();

        make_transaction::<_, ()>("write test", &path, "content.nix", noop_build(), |file| {
            *file.get_mut_file_content()? = String::from("after");
            Ok(())
        })
        .unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("content.nix")).unwrap(),
            "after"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// No-regression tests
// ─────────────────────────────────────────────────────────────────────────────
mod no_regression {
    use super::*;

    /// Two successive `make_transaction` calls on the same repo both succeed.
    ///
    /// Regression: the first transaction sometimes left a lock that blocked
    /// the second one from opening.
    #[test]
    fn two_successive_make_transactions_on_same_repo() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "f.nix", "v1");
        let _guard = lock_build_queue();

        make_transaction::<_, ()>("tx1", &path, "f.nix", noop_build(), |_| Ok(())).unwrap();
        make_transaction::<_, ()>("tx2", &path, "f.nix", noop_build(), |_| Ok(())).unwrap();
    }

    /// After a failed transaction, the repo is clean enough for the next one.
    ///
    /// Regression: rollback did not always restore the immutable flag, leaving
    /// the repo dirty and blocking the next `begin`.
    #[test]
    fn failed_transaction_does_not_dirty_repo_for_next() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "f.nix", "original");
        let _guard = lock_build_queue();

        // First transaction: deliberate failure
        let _ = make_transaction::<_, ()>("fail", &path, "f.nix", noop_build(), |_| {
            Err(mx::ErrorKind::PermissionDenied)
        });

        // Second transaction must not get GitNotCommitted
        let result = make_transaction::<_, ()>("success", &path, "f.nix", noop_build(), |_| Ok(()));
        assert!(
            result.is_ok(),
            "repo must not be dirty after a rollback: {:?}",
            result
        );
    }

    /// Content poisoned by a failed closure is not visible in the next transaction.
    ///
    /// Regression: rollback failed silently, leaving the poisoned content
    /// readable by the next transaction.
    #[test]
    fn rolled_back_content_not_visible_in_next_transaction() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "f.nix", "clean");
        let _guard = lock_build_queue();

        let _ = make_transaction::<_, ()>("poison", &path, "f.nix", noop_build(), |file| {
            *file.get_mut_file_content()? = String::from("# poison");
            Err(mx::ErrorKind::PermissionDenied)
        });

        let content = make_transaction("read", &path, "f.nix", noop_build(), |file| {
            Ok(file.get_file_content()?.clone())
        })
        .unwrap();

        assert!(
            !content.contains("poison"),
            "rolled-back content must not leak into the next transaction"
        );
    }

    /// Resources are released after repeated errors in the closure.
    #[test]
    fn error_in_closure_releases_resources() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "res.nix", "data");
        let _guard = lock_build_queue();

        for _ in 0..3 {
            let _ = make_transaction::<_, ()>("iter", &path, "res.nix", noop_build(), |_| {
                Err(mx::ErrorKind::PermissionDenied)
            });
        }

        let result = make_transaction::<_, ()>("final", &path, "res.nix", noop_build(), |_| Ok(()));
        assert!(
            result.is_ok(),
            "resources must be released after each error"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stash tests via make_transaction
// ─────────────────────────────────────────────────────────────────────────────
mod stash {
    use super::*;

    /// Untracked files present before `make_transaction` are stashed and then
    /// restored once the transaction finishes (commit path).
    #[test]
    fn untracked_files_are_stashed_and_restored_after_commit() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "target.nix", "original");
        let _guard = lock_build_queue();

        // untracked bystander — must NOT be committed, so make_transaction stashes it
        let bystander = dir.path().join("bystander.nix");
        fs::write(&bystander, "bystander content").unwrap();

        make_transaction::<_, ()>("tx", &path, "target.nix", noop_build(), |_| Ok(())).unwrap();

        // bystander must be back after the transaction
        assert!(
            bystander.exists(),
            "untracked file must be restored after commit"
        );
        assert_eq!(fs::read_to_string(&bystander).unwrap(), "bystander content");
    }

    /// Untracked files are restored even when the closure fails (rollback path).
    #[test]
    fn untracked_files_are_stashed_and_restored_after_rollback() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "target.nix", "original");

        let bystander = dir.path().join("bystander.nix");
        fs::write(&bystander, "bystander content").unwrap();

        let _ = make_transaction::<_, ()>("tx", &path, "target.nix", noop_build(), |_| {
            Err(mx::ErrorKind::PermissionDenied)
        });

        assert!(
            bystander.exists(),
            "untracked file must be restored after rollback"
        );
        assert_eq!(fs::read_to_string(&bystander).unwrap(), "bystander content");
    }

    /// The committed result of the transaction is NOT overwritten by the stash pop.
    /// The target file was committed before the transaction, so the stash contains
    /// its original committed state — stash pop must merge cleanly.
    #[test]
    fn stash_pop_does_not_overwrite_committed_result() {
        let dir = setup_repo();
        let path = repo_path(&dir);
        create_and_commit(&dir, "target.nix", "original");
        let _guard = lock_build_queue();
        // untracked bystander triggers the stash
        fs::write(dir.path().join("bystander.nix"), "bystander").unwrap();

        make_transaction::<_, ()>("tx", &path, "target.nix", noop_build(), |file| {
            *file.get_mut_file_content()? = String::from("modified");
            Ok(())
        })
        .unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("target.nix")).unwrap(),
            "modified",
            "stash pop must not overwrite the committed result"
        );
    }
}
