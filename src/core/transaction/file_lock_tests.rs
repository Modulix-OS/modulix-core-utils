/// Tests for `NixFile`.
///
/// # Structure
/// - `unit`           – Pure logic, no real I/O.
/// - `integration`    – Operate on real temporary files.
/// - `no_regression` – Edge cases and historical bugs.
///
/// # Prerequisites for integration tests
/// Tests manipulating `make_immutable` / `make_mutable` via `ioctl` must be
/// run as root on a filesystem supporting `FS_IOC_SETFLAGS`
/// (ext2/ext3/ext4). On unsupported filesystems (tmpfs, overlayfs…) the
/// immutable flag is silently ignored by `is_owned_by_root`.
///
/// ```
/// cargo test unit          # tests with no I/O
/// sudo cargo test          # all tests
/// ```
use super::NixFile;
use crate::mx;

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests – no real I/O
// ─────────────────────────────────────────────────────────────────────────────
mod unit {
    use super::*;

    // ── new() ─────────────────────────────────────────────────────────────────

    /// `new` correctly concatenates `repo_path` and `relative_path`.
    #[test]
    fn new_builds_correct_path() {
        let f = NixFile::new("/etc/nixos", "/hardware.nix");
        assert_eq!(f.get_file_path(), "/etc/nixos/hardware.nix");
    }

    /// `new` with empty paths produces an empty path.
    #[test]
    fn new_empty_paths() {
        let f = NixFile::new("", "");
        assert_eq!(f.get_file_path(), "");
    }

    /// After construction, `was_created` is `false`.
    #[test]
    fn new_was_created_is_false() {
        let f = NixFile::new("/repo", "/file.nix");
        assert!(!f.was_created());
    }

    /// `new` with a path that has no leading slash is accepted without panic.
    #[test]
    fn new_accepts_relative_like_path() {
        let f = NixFile::new("/repo", "file.nix");
        assert_eq!(f.get_file_path(), "/repofile.nix");
    }

    // ── get_file_content / get_mut_file_content ───────────────────────────────

    /// Reading content without an active transaction returns `TransactionNotBegin`.
    #[test]
    fn get_file_content_without_transaction_errors() {
        let f = NixFile::new("/repo", "/file.nix");
        assert!(matches!(
            f.get_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// Getting a mutable reference without a transaction returns `TransactionNotBegin`.
    #[test]
    fn get_mut_file_content_without_transaction_errors() {
        let mut f = NixFile::new("/repo", "/file.nix");
        assert!(matches!(
            f.get_mut_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    // ── begin() with no file ──────────────────────────────────────────────────

    /// `begin` on a non-existent path returns `FileNotFound`.
    #[test]
    fn begin_nonexistent_file_returns_file_not_found() {
        let mut f = NixFile::new("/nonexistent_repo_xyz", "/ghost.nix");
        assert!(
            matches!(f.begin(), Err(mx::ErrorKind::FileNotFound)),
            "expected FileNotFound"
        );
    }

    // ── commit() / close() without a transaction ──────────────────────────────

    /// `commit` without a prior transaction returns `InvalidFile`.
    #[test]
    fn commit_without_begin_returns_invalid_file() {
        let mut f = NixFile::new("/repo", "/file.nix");
        assert!(matches!(f.commit(), Err(mx::ErrorKind::InvalidFile)));
    }

    /// `was_created` stays `false` if `create_file` is never called.
    #[test]
    fn was_created_stays_false_without_create_file() {
        let f = NixFile::new("/repo", "/file.nix");
        assert!(!f.was_created());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests – real temporary files
// ─────────────────────────────────────────────────────────────────────────────
mod integration {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp_dir() -> TempDir {
        tempfile::tempdir().expect("failed to create temporary directory")
    }

    // ── create_file ───────────────────────────────────────────────────────────

    /// `create_file` creates the file with the expected NixOS skeleton.
    #[test]
    fn create_file_writes_nix_skeleton() {
        let dir = tmp_dir();
        let mut f = NixFile::new(dir.path().to_str().unwrap(), "/module.nix");
        f.create_file().expect("create_file failed");
        let content = fs::read_to_string(f.get_file_path()).unwrap();
        assert_eq!(content, "{config, lib, pkgs, ...}:\n{\n}\n");
    }

    /// `was_created` becomes `true` after `create_file`.
    #[test]
    fn create_file_sets_was_created() {
        let dir = tmp_dir();
        let mut f = NixFile::new(dir.path().to_str().unwrap(), "/module.nix");
        f.create_file().unwrap();
        assert!(f.was_created());
    }

    /// `create_file` on a non-existent directory returns an I/O error.
    #[test]
    fn create_file_invalid_dir_errors() {
        let mut f = NixFile::new("/nonexistent_dir_xyz_abc", "/module.nix");
        assert!(matches!(f.create_file(), Err(mx::ErrorKind::IOError(_))));
    }

    /// Calling `create_file` twice on the same path succeeds (overwrites).
    #[test]
    fn create_file_twice_overwrites() {
        let dir = tmp_dir();
        let mut f = NixFile::new(dir.path().to_str().unwrap(), "/module.nix");
        f.create_file().unwrap();
        // make_mutable is needed because create_file sets the immutable flag (root only).
        // On tmpfs/non-root, make_mutable is a no-op so we can overwrite directly.
        let result = f.create_file();
        assert!(result.is_ok());
    }

    // ── begin → get_file_content → close ─────────────────────────────────────

    /// After `begin`, `get_file_content` returns the exact content of the file.
    #[test]
    fn begin_loads_file_content() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/test.nix", path), "hello nix").unwrap();

        let mut f = NixFile::new(path, "/test.nix");
        f.begin().unwrap();
        assert_eq!(f.get_file_content().unwrap(), "hello nix");
        f.close().unwrap();
    }

    /// `get_file_content` succeeds only during an active transaction.
    #[test]
    fn get_file_content_only_inside_transaction() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/test.nix", path), "data").unwrap();

        let mut f = NixFile::new(path, "/test.nix");

        assert!(matches!(
            f.get_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));

        f.begin().unwrap();
        assert!(f.get_file_content().is_ok());
        f.close().unwrap();

        assert!(matches!(
            f.get_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// `begin` on an empty file loads an empty string without error.
    #[test]
    fn begin_empty_file_loads_empty_string() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/empty.nix", path), "").unwrap();

        let mut f = NixFile::new(path, "/empty.nix");
        f.begin().unwrap();
        assert_eq!(f.get_file_content().unwrap(), "");
        f.close().unwrap();
    }

    // ── begin → modification → commit ─────────────────────────────────────────

    /// In-memory modifications are persisted by `commit`.
    #[test]
    fn commit_persists_modifications() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/config.nix", path), "original content").unwrap();

        let mut f = NixFile::new(path, "/config.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from("modified content");
        f.commit().unwrap();

        assert_eq!(
            fs::read_to_string(format!("{}/config.nix", path)).unwrap(),
            "modified content"
        );
    }

    /// `commit` correctly truncates when the new content is shorter.
    #[test]
    fn commit_truncates_when_content_is_shorter() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/long.nix", path), "a".repeat(200)).unwrap();

        let mut f = NixFile::new(path, "/long.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from("short");
        f.commit().unwrap();

        assert_eq!(
            fs::read_to_string(format!("{}/long.nix", path)).unwrap(),
            "short"
        );
    }

    /// Writing empty content via `commit` empties the file on disk.
    #[test]
    fn commit_empty_content_empties_file() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/config.nix", path), "some data").unwrap();

        let mut f = NixFile::new(path, "/config.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::new();
        f.commit().unwrap();

        assert!(
            fs::read_to_string(format!("{}/config.nix", path))
                .unwrap()
                .is_empty()
        );
    }

    /// `commit` correctly preserves multi-line content with indentation.
    #[test]
    fn commit_preserves_multiline_content() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/ml.nix", path), "").unwrap();

        let multiline = "{ config, lib, pkgs, ... }:\n{\n  boot.loader.grub.device = \"/dev/sda\";\n  networking.hostName = \"nixos\";\n}\n";

        let mut f = NixFile::new(path, "/ml.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from(multiline);
        f.commit().unwrap();

        assert_eq!(
            fs::read_to_string(format!("{}/ml.nix", path)).unwrap(),
            multiline
        );
    }

    /// `commit` correctly preserves UTF-8 characters (accents, emojis…).
    #[test]
    fn commit_preserves_utf8_content() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/utf8.nix", path), "").unwrap();

        let utf8_content = "# Comment with accents: éàü and emoji 🦀\n";

        let mut f = NixFile::new(path, "/utf8.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from(utf8_content);
        f.commit().unwrap();

        assert_eq!(
            fs::read_to_string(format!("{}/utf8.nix", path)).unwrap(),
            utf8_content
        );
    }

    /// After `commit`, the transaction is closed.
    #[test]
    fn commit_ends_transaction() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/config.nix", path), "data").unwrap();

        let mut f = NixFile::new(path, "/config.nix");
        f.begin().unwrap();
        f.commit().unwrap();

        assert!(matches!(
            f.get_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    // ── begin → close (discard) ───────────────────────────────────────────────

    /// After `close`, in-memory modifications are not persisted.
    #[test]
    fn close_does_not_persist_modifications() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/config.nix", path), "original").unwrap();

        let mut f = NixFile::new(path, "/config.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from("should not appear on disk");
        f.close().unwrap();

        assert_eq!(
            fs::read_to_string(format!("{}/config.nix", path)).unwrap(),
            "original"
        );
    }

    /// After `close`, the transaction is closed.
    #[test]
    fn close_ends_transaction() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/config.nix", path), "data").unwrap();

        let mut f = NixFile::new(path, "/config.nix");
        f.begin().unwrap();
        f.close().unwrap();

        assert!(matches!(
            f.get_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    // ── Full lifecycle ────────────────────────────────────────────────────────

    /// Full lifecycle: creation followed by two successive transactions.
    #[test]
    fn full_lifecycle_create_then_two_transactions() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();

        let mut f = NixFile::new(path, "/full.nix");
        f.create_file().unwrap();
        assert!(f.was_created());

        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() =
            String::from("{ config, lib, pkgs, ... }:\n{ services.nginx.enable = true; }\n");
        f.commit().unwrap();

        f.begin().unwrap();
        let content = f.get_file_content().unwrap().clone();
        f.close().unwrap();

        assert!(content.contains("services.nginx.enable = true"));
    }

    /// Two distinct `NixFile` instances on different files are independent.
    #[test]
    fn two_nix_files_are_independent() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/a.nix", path), "content_a").unwrap();
        fs::write(format!("{}/b.nix", path), "content_b").unwrap();

        let mut fa = NixFile::new(path, "/a.nix");
        let mut fb = NixFile::new(path, "/b.nix");

        fa.begin().unwrap();
        fb.begin().unwrap();

        assert_eq!(fa.get_file_content().unwrap(), "content_a");
        assert_eq!(fb.get_file_content().unwrap(), "content_b");

        fa.close().unwrap();
        fb.close().unwrap();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Non-regression tests
// ─────────────────────────────────────────────────────────────────────────────
mod no_regression {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp_dir() -> TempDir {
        tempfile::tempdir().expect("failed to create temporary directory")
    }

    /// Two successive transactions on the same `NixFile` work correctly.
    ///
    /// Regression: `file_content` was not reset between two `begin` calls,
    /// which could accumulate content instead of replacing it.
    #[test]
    fn two_successive_transactions_on_same_file() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/f.nix", path), "v1").unwrap();

        let mut f = NixFile::new(path, "/f.nix");

        f.begin().unwrap();
        assert_eq!(f.get_file_content().unwrap(), "v1");
        f.close().unwrap();

        // Modify the file on disk between transactions
        fs::write(format!("{}/f.nix", path), "v2").unwrap();

        f.begin().unwrap();
        assert_eq!(
            f.get_file_content().unwrap(),
            "v2",
            "second transaction should read v2, not accumulate v1+v2"
        );
        f.close().unwrap();
    }

    /// `commit` after `close` returns `InvalidFile` without panicking.
    ///
    /// Regression: `unwrap()` was called on `self.file` which was `None`.
    #[test]
    fn commit_after_close_does_not_panic() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/f.nix", path), "data").unwrap();

        let mut f = NixFile::new(path, "/f.nix");
        f.begin().unwrap();
        f.close().unwrap();

        assert!(matches!(f.commit(), Err(mx::ErrorKind::InvalidFile)));
    }

    /// `get_mut_file_content` after `commit` returns `TransactionNotBegin`.
    ///
    /// Regression: after commit, `self.file` was `None` but `file_content`
    /// could still hold the previous content.
    #[test]
    fn get_mut_content_after_commit_errors() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/f.nix", path), "data").unwrap();

        let mut f = NixFile::new(path, "/f.nix");
        f.begin().unwrap();
        f.commit().unwrap();

        assert!(matches!(
            f.get_mut_file_content(),
            Err(mx::ErrorKind::TransactionNotBegin)
        ));
    }

    /// `commit` leaves no residual bytes when the new content is shorter.
    ///
    /// Regression: without prior truncation (`set_len(0)`), trailing bytes
    /// from the old content remained on disk.
    #[test]
    fn commit_no_leftover_bytes_when_shorter() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        let long = "A".repeat(500);
        fs::write(format!("{}/f.nix", path), &long).unwrap();

        let mut f = NixFile::new(path, "/f.nix");
        f.begin().unwrap();
        *f.get_mut_file_content().unwrap() = String::from("tiny");
        f.commit().unwrap();

        let on_disk = fs::read_to_string(format!("{}/f.nix", path)).unwrap();
        assert_eq!(on_disk, "tiny", "no residual bytes should remain on disk");
        assert_eq!(on_disk.len(), 4);
    }

    /// `begin` re-reads disk content when it has changed between transactions.
    ///
    /// Regression: content was cached and not re-read from disk.
    #[test]
    fn begin_rereads_disk_content_after_external_change() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/f.nix", path), "before").unwrap();

        let mut f = NixFile::new(path, "/f.nix");
        f.begin().unwrap();
        f.close().unwrap();

        // External modification between two transactions
        fs::write(format!("{}/f.nix", path), "after").unwrap();

        f.begin().unwrap();
        assert_eq!(
            f.get_file_content().unwrap(),
            "after",
            "begin must re-read from disk, not use a stale cache"
        );
        f.close().unwrap();
    }

    /// `get_file_path` returns the same path across multiple transaction cycles.
    #[test]
    fn get_file_path_stable_across_transactions() {
        let dir = tmp_dir();
        let path = dir.path().to_str().unwrap();
        fs::write(format!("{}/stable.nix", path), "").unwrap();

        let mut f = NixFile::new(path, "/stable.nix");
        let expected = f.get_file_path().to_string();

        f.begin().unwrap();
        assert_eq!(f.get_file_path(), expected);
        f.close().unwrap();

        assert_eq!(f.get_file_path(), expected);
    }
}
