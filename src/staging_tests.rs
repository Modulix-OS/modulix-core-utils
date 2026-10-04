/// Tests for [`crate::staging`].
///
/// # Structure
/// - `unit` – staging-area bookkeeping, no `nix` and no rebuild.
///
/// # Isolation
/// Every test redirects [`crate::cache_dir`] through `MX_CACHE_DIR` so it
/// operates on its own temporary directory. `std::env::set_var` is `unsafe` in
/// edition 2024 and the environment is process-wide, so the tests that touch it
/// are serialised by [`ENV_GUARD`] and restore the previous value before
/// releasing it.
///
/// # Dependencies
/// ```toml
/// [dev-dependencies]
/// tempfile = "3"
/// ```
use super::*;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

/// Serialises the tests that override `MX_CACHE_DIR`: the environment is
/// shared by the whole test binary, which runs its tests in parallel threads.
static ENV_GUARD: Mutex<()> = Mutex::new(());

/// Points [`crate::cache_dir`] at a fresh temporary directory for as long as
/// the returned values live.
///
/// # Returns
/// The guard holding [`ENV_GUARD`], and the temporary directory. Both must be
/// kept alive for the duration of the test: dropping the directory deletes the
/// staging area, dropping the guard lets another test overwrite the variable.
fn with_cache_dir() -> (MutexGuard<'static, ()>, TempDir) {
    let guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new().expect("temp dir");
    // SAFETY: the environment is only mutated while `ENV_GUARD` is held, so no
    // other test reads or writes `MX_CACHE_DIR` concurrently, and the value is
    // overwritten by the next test that takes the guard.
    unsafe {
        std::env::set_var("MX_CACHE_DIR", dir.path());
    }
    (guard, dir)
}

/// Creates a staging area as [`stage_update`] would leave it.
///
/// # Arguments
/// * `lock` – candidate lockfile text to record.
/// * `built` – value of the `built` flag, and whether the `result` marker
///   standing in for the garbage-collection root is created.
fn fake_staged(lock: &str, built: bool) {
    let dir = staging_dir();
    fs::create_dir_all(dir.join(STAGED_CONFIG_DIR)).expect("staged config dir");
    fs::write(dir.join(STAGED_LOCK_FILE), lock).expect("staged lock");
    if built {
        fs::write(dir.join(STAGED_RESULT_LINK), "/nix/store/fake").expect("result marker");
    }
    write_meta(&StagedUpdate {
        created_at: 42,
        built,
        inputs: Vec::new(),
    })
    .expect("meta");
}

mod unit {
    use super::*;

    /// Nothing staged reads back as nothing staged, not as an error.
    #[test]
    fn staged_status_none_when_empty() {
        let (_guard, _dir) = with_cache_dir();
        assert!(staged_status().expect("status").is_none());
    }

    /// A complete staging area reads back with its flag and its candidate.
    #[test]
    fn staged_status_reports_built_candidate() {
        let (_guard, _dir) = with_cache_dir();
        fake_staged("lock-text", true);

        let status = staged_status().expect("status").expect("some");
        assert!(status.built);
        assert_eq!(status.created_at, 42);
        assert_eq!(staged_lock().expect("lock").as_deref(), Some("lock-text"));
    }

    /// A staged update whose garbage-collection root is gone is reported as not
    /// built: activating it would have to build, so a caller must not be told
    /// otherwise.
    #[test]
    fn staged_status_forgets_built_without_gc_root() {
        let (_guard, _dir) = with_cache_dir();
        fake_staged("lock-text", true);
        fs::remove_file(staging_dir().join(STAGED_RESULT_LINK)).expect("drop root");

        let status = staged_status().expect("status").expect("some");
        assert!(!status.built);
    }

    /// A record left without its configuration copy is unusable, so it reads
    /// back as nothing staged.
    #[test]
    fn staged_status_none_without_config_copy() {
        let (_guard, _dir) = with_cache_dir();
        fake_staged("lock-text", true);
        fs::remove_dir_all(staging_dir().join(STAGED_CONFIG_DIR)).expect("drop config");

        assert!(staged_status().expect("status").is_none());
    }

    /// A truncated record is "nothing staged" rather than an error: the next
    /// staging starts over.
    #[test]
    fn staged_status_none_on_unparsable_meta() {
        let (_guard, _dir) = with_cache_dir();
        fake_staged("lock-text", true);
        fs::write(staging_dir().join(STAGED_META_FILE), "{ truncated").expect("truncate");

        assert!(staged_status().expect("status").is_none());
    }

    /// Discarding removes the whole area, root included, and is a no-op when
    /// there is nothing to discard.
    #[test]
    fn discard_staged_removes_area_and_is_idempotent() {
        let (_guard, _dir) = with_cache_dir();
        fake_staged("lock-text", true);

        discard_staged().expect("first discard");
        assert!(!staging_dir().exists());
        assert!(staged_status().expect("status").is_none());

        discard_staged().expect("second discard");
    }

    /// Applying with nothing staged reports "nothing done" instead of failing -
    /// that is what most shutdowns look like.
    #[test]
    fn apply_staged_false_when_nothing_staged() {
        let (_guard, dir) = with_cache_dir();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).expect("config dir");

        let applied = apply_staged(config.to_str().unwrap(), None).expect("apply");
        assert!(!applied);
    }

    /// A staged update that is no longer built is not applied: the caller asked
    /// for the cheap activation, and there is none to be had.
    #[test]
    fn apply_staged_false_when_not_built() {
        let (_guard, dir) = with_cache_dir();
        fake_staged("lock-text", false);
        let config = dir.path().join("config");
        fs::create_dir_all(&config).expect("config dir");

        let applied = apply_staged(config.to_str().unwrap(), None).expect("apply");
        assert!(!applied);
    }

    /// The staging area sits under the cache directory, so `MX_CACHE_DIR` moves
    /// it and nothing is ever written inside the configuration repository.
    #[test]
    fn staging_dir_follows_cache_dir() {
        let (_guard, dir) = with_cache_dir();
        assert_eq!(staging_dir(), dir.path().join(STAGING_DIR));
    }
}
