//! Staged system updates: resolved and built while the machine is in use,
//! activated when it shuts down.
//!
//! A system update is not a per-package action on NixOS - it moves every flake
//! input at once and rebuilds the whole system - so applying one while the
//! machine is being used replaces the running closure under the user's feet.
//! This module splits that in two: [`stage_update`] pays the network and CPU
//! cost up front and leaves a complete, ready-to-activate system in the store,
//! and [`apply_staged`] activates it from a shutdown unit, where no session is
//! disturbed.
//!
//! # Why the candidate stays out of the git tree
//!
//! The staged lockfile is written under [`crate::cache_dir`], never into the
//! configuration repository, which keeps one invariant true: *the committed
//! `flake.lock` is the one the running system was built from*. An install that
//! happens while an update waits (`install_package`, `add_mount`, …) commits
//! with `UpdateInput::Keep`, which touches no lockfile, so it rebuilds against
//! the revisions already running and cannot drag the pending update in. The
//! promotion of the candidate into the repository is [`apply_staged`]'s last
//! step, after the new system has been built and made the next boot's default.
//!
//! # Why the staged copy is also what gets activated
//!
//! [`crate::update::build_with_lock`] builds a `.git`-less checkout, which
//! `nix` reads as a `path:` flake where it reads the real repository as
//! `git+file://`. The flake *source* hashes differently, so the top-level
//! `nixos-system-*` derivation of a build staged that way is not the one a
//! rebuild of the real repository produces. [`apply_staged`] therefore points
//! `nixos-rebuild boot` at the very directory [`stage_update`] built, which
//! makes the activation a store lookup plus a bootloader write rather than a
//! second build. That is the whole point of pre-building.
//!
//! # Why the `result` symlink is kept
//!
//! `nixos-rebuild build` leaves a `result` symlink in its working directory,
//! and `nix` registers an indirect garbage-collection root for it. Keeping that
//! symlink is what makes a staged closure survive a `nix-collect-garbage`
//! between the pre-build and the shutdown that applies it; deleting it, as
//! [`crate::update::build_with_lock`] does, makes the closure collectable
//! immediately.

use std::fs;
use std::path;
use std::time;

use serde::{Deserialize, Serialize};

use crate::core::transaction::{
    build_queue::BuildQueue,
    make_transaction_commit_only,
    transaction::{BuildCommand, Transaction, UpdateInput},
};
use crate::error::io_error_at;
use crate::update::{
    FILE_FLAKE_LOCK_PATH, FILE_FLAKE_PATH, OutdatedInput, check_update, diff_locks,
    stage_head_tree, update_no_transaction,
};
use crate::{CONFIG_NAME, mx};

/// Directory, under [`crate::cache_dir`], holding the staged update.
///
/// Inside [`crate::CONFIG_DIRECTORY`] the cache directory is excluded from git
/// (`.git/info/exclude`), so nothing here can end up committed by accident.
const STAGING_DIR: &str = "pending-update";

/// Sub-directory of [`STAGING_DIR`] holding the configuration copy that is
/// built and later activated.
const STAGED_CONFIG_DIR: &str = "config";

/// File in [`STAGING_DIR`] holding the candidate lockfile verbatim, as the
/// canonical record of what is staged.
const STAGED_LOCK_FILE: &str = "lock";

/// File in [`STAGING_DIR`] holding the serialized [`StagedUpdate`].
const STAGED_META_FILE: &str = "meta.json";

/// Symlink `nixos-rebuild build` leaves in [`STAGING_DIR`], and the
/// garbage-collection root keeping the staged closure alive.
const STAGED_RESULT_LINK: &str = "result";

/// A system update that has been resolved, and possibly built, but not applied.
///
/// # Fields
/// * `created_at` - when the candidate was staged, Unix seconds. `0` when the
///   clock could not be read, which is not treated as an error.
/// * `built` - whether the closure is realised in the store *and* still
///   rooted. Read back from disk it is reported as `false` as soon as the
///   `result` symlink is gone, so a caller never promises a fast activation
///   that would in fact rebuild.
/// * `inputs` - what the update moves, as [`diff_locks`] describes it: a purely
///   local comparison of the committed lockfile against the candidate, so it is
///   consistent by construction with what an activation would apply.
#[derive(Serialize, Deserialize)]
pub struct StagedUpdate {
    pub created_at: u64,
    pub built: bool,
    pub inputs: Vec<OutdatedInput>,
}

/// Absolute path of the staging directory.
///
/// # Pre-conditions
/// None; the directory does not have to exist.
///
/// # Returns
/// [`crate::cache_dir`] joined with [`STAGING_DIR`]. Follows
/// `MX_CACHE_DIR` like every other cache path, so a test or the test-mode
/// daemon stages inside its own directory.
fn staging_dir() -> path::PathBuf {
    crate::cache_dir().join(STAGING_DIR)
}

/// Current Unix timestamp in seconds.
///
/// # Returns
/// Seconds since the Unix epoch, or `0` if the system clock is set before it.
/// A timestamp is informational here, so an unreadable clock is not worth
/// failing a staging over.
fn now_seconds() -> u64 {
    time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Writes `meta` as the staging area's `meta.json`.
///
/// # Arguments
/// * `meta` - the record to persist.
///
/// # Pre-conditions
/// The staging directory must exist.
///
/// # Post-conditions
/// Replaces any previous record. Not atomic: a crash during the write leaves a
/// truncated file, which [`staged_status`] reports as "nothing staged" rather
/// than as an error - the next [`stage_update`] then re-stages from scratch.
///
/// # Errors
/// [`mx::ErrorKind::ParseError`] if the record cannot be serialized, and
/// [`mx::ErrorKind::IOError`] if the file cannot be written.
fn write_meta(meta: &StagedUpdate) -> mx::Result<()> {
    let path = staging_dir().join(STAGED_META_FILE);
    let text = serde_json::to_string(meta).map_err(mx::ErrorKind::ParseError)?;
    fs::write(&path, text).map_err(|e| io_error_at(&path.to_string_lossy(), e))
}

/// Reads the staged update's record, if there is one.
///
/// # Pre-conditions
/// None.
///
/// # Post-conditions
/// Read-only: no process is spawned, nothing is written, and neither the
/// configuration repository nor the store is touched. A record whose `result`
/// symlink has disappeared (garbage-collected between the pre-build and now) is
/// reported with `built` forced to `false`, so the caller knows an activation
/// would have to build.
///
/// # Returns
/// `Some(status)` when a staged candidate is present, `None` when there is
/// none, when the record is missing, or when it cannot be parsed - all three
/// mean "nothing usable is staged", which is the same thing to every caller.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] only for a failure other than the file being
/// absent (a permission problem on the cache directory, typically).
pub fn staged_status() -> mx::Result<Option<StagedUpdate>> {
    let dir = staging_dir();
    let text = match fs::read_to_string(dir.join(STAGED_META_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(io_error_at(
                &dir.join(STAGED_META_FILE).to_string_lossy(),
                e,
            ));
        }
    };

    let mut meta: StagedUpdate = match serde_json::from_str(&text) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };

    if !dir.join(STAGED_CONFIG_DIR).is_dir() {
        return Ok(None);
    }
    if fs::symlink_metadata(dir.join(STAGED_RESULT_LINK)).is_err() {
        meta.built = false;
    }

    Ok(Some(meta))
}

/// Candidate lockfile currently staged, verbatim.
///
/// # Pre-conditions
/// None.
///
/// # Post-conditions
/// Read-only.
///
/// # Returns
/// `Some(lock)` with the exact text [`check_update`] produced, `None` when
/// nothing is staged.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] for a read failure other than the file being
/// absent.
fn staged_lock() -> mx::Result<Option<String>> {
    let path = staging_dir().join(STAGED_LOCK_FILE);
    match fs::read_to_string(&path) {
        Ok(lock) => Ok(Some(lock)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error_at(&path.to_string_lossy(), e)),
    }
}

/// Throws the staged update away, garbage-collection root included.
///
/// # Pre-conditions
/// No [`stage_update`] or [`apply_staged`] may be running: both work inside the
/// directory this removes.
///
/// # Post-conditions
/// The staging directory is gone, so the pre-built closure is collectable
/// again. The configuration repository and the running system are untouched - a
/// discarded candidate only means the next [`stage_update`] starts over. A
/// no-op when nothing is staged.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the directory exists but cannot be removed.
pub fn discard_staged() -> mx::Result<()> {
    let dir = staging_dir();
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error_at(&dir.to_string_lossy(), e)),
    }
}

/// Resolves the available update and builds it, without applying anything.
///
/// The expensive half of an update, meant to run in the background: it probes
/// the inputs ([`check_update`]), stages a copy of the configuration carrying
/// the candidate lockfile, and realises the whole closure
/// (`nixos-rebuild build`). What is left behind is a system ready for
/// [`apply_staged`] to make the next boot's default in seconds.
///
/// # Arguments
/// * `config_dir` - configuration repository to stage from. Its `HEAD` tree is
///   copied, not its working directory, because `nix` reads the repository as a
///   `git+file://` flake and so sees tracked files only.
/// * `cores` - caps the pre-build to this many CPU cores
///   (`nixos-rebuild --cores`); `None` leaves the Nix default. Worth setting
///   for a background build on a machine in use.
///
/// # Pre-conditions
/// Must be called from within a Tokio runtime: the pre-build runs on
/// [`tokio::task::spawn_blocking`], so a caller's worker thread is not held for
/// the whole build. `config_dir` must be a git repository with a resolvable
/// `HEAD`, and the flake must declare no relative-path input, which would not
/// resolve from the staged copy. Must not run concurrently with itself or with
/// [`apply_staged`]; serialising the callers is the caller's job (the daemon
/// holds one guard for every rebuild).
///
/// # Post-conditions
/// `config_dir` is **unchanged**: no commit, no lockfile write, no rebuild of
/// the running system, and nothing a concurrent install could pick up.
/// Idempotent: when the candidate is the one already staged and its closure is
/// still rooted, the existing record is returned and nothing is rebuilt. A
/// candidate that differs replaces the staging area wholesale, so the previous
/// pre-build's root is dropped. Blocks for the whole probe plus the whole
/// build, and takes a turn in the shared build queue, so it never runs
/// alongside a real rebuild.
///
/// # Returns
/// `Some(status)` describing what is now staged, or `None` when every input is
/// already current - in which case any previously staged candidate is discarded,
/// since it has been superseded.
///
/// # Errors
/// Anything [`check_update`] reports (a failed or timed-out probe),
/// [`mx::ErrorKind::IOError`] or [`mx::ErrorKind::GitError`] if the staging
/// area cannot be written, [`mx::ErrorKind::ParseError`] if the candidate or
/// the committed lockfile cannot be diffed, and
/// [`mx::ErrorKind::BuildError`] with the build's standard error if
/// `nixos-rebuild build` exits non-zero.
pub async fn stage_update(
    config_dir: &str,
    cores: Option<u32>,
) -> mx::Result<Option<StagedUpdate>> {
    let candidate = match check_update(config_dir).await? {
        Some(candidate) => candidate,
        None => {
            discard_staged()?;
            return Ok(None);
        }
    };

    stage_candidate(config_dir, candidate, cores).await.map(Some)
}

/// Stages an already resolved candidate, skipping the probe.
///
/// The probe is a full `nix flake update` over the network, minutes long, so a
/// caller that already has a candidate in hand must not pay for it twice. In
/// Modulix that caller is the daemon: `Store1.CheckUpdate` resolved the
/// candidate to answer "is there an update?", and the staging that follows
/// builds *that* lockfile - which is also what makes the revisions a user was
/// shown and the revisions they get the same.
///
/// # Arguments
/// * `config_dir` - configuration repository to stage from; its `HEAD` tree is
///   copied.
/// * `candidate` - full `flake.lock` text to stage, verbatim, as returned by
///   [`check_update`].
/// * `cores` - caps the pre-build to this many CPU cores; `None` leaves the Nix
///   Nix default.
///
/// # Pre-conditions
/// As [`stage_update`], plus: `candidate` must be a lockfile `nix` accepts for
/// this very `flake.nix`. It is **not** compared against the committed one, so
/// passing a stale or identical lockfile stages and builds it anyway - the
/// caller decides there is something to stage.
///
/// # Post-conditions
/// As [`stage_update`]: `config_dir` untouched, idempotent when `candidate`
/// matches what is already staged and still built, and the staging area is
/// replaced wholesale otherwise.
///
/// # Returns
/// What is now staged.
///
/// # Errors
/// As [`stage_update`], minus the probe's failure modes.
pub async fn stage_candidate(
    config_dir: &str,
    candidate: String,
    cores: Option<u32>,
) -> mx::Result<StagedUpdate> {
    if let Some(existing) = staged_status()?
        && existing.built
        && staged_lock()?.as_deref() == Some(candidate.as_str())
    {
        return Ok(existing);
    }

    let current_lock_path = path::Path::new(config_dir).join(FILE_FLAKE_LOCK_PATH);
    let current = fs::read_to_string(&current_lock_path)
        .map_err(|e| io_error_at(&current_lock_path.to_string_lossy(), e))?;
    let inputs = diff_locks(&current, &candidate)?;

    discard_staged()?;
    let dir = staging_dir();
    fs::create_dir_all(&dir).map_err(|e| io_error_at(&dir.to_string_lossy(), e))?;

    let staged_config = dir.join(STAGED_CONFIG_DIR);
    stage_head_tree(path::Path::new(config_dir), &staged_config)?;

    let staged_lock_path = staged_config.join(FILE_FLAKE_LOCK_PATH);
    fs::write(&staged_lock_path, &candidate)
        .map_err(|e| io_error_at(&staged_lock_path.to_string_lossy(), e))?;

    let record_path = dir.join(STAGED_LOCK_FILE);
    fs::write(&record_path, &candidate)
        .map_err(|e| io_error_at(&record_path.to_string_lossy(), e))?;

    let mut meta = StagedUpdate {
        created_at: now_seconds(),
        built: false,
        inputs,
    };
    write_meta(&meta)?;

    let build_dir = dir.clone();
    tokio::task::spawn_blocking(move || {
        run_staged_rebuild(&build_dir, &staged_config, BuildCommand::Build, cores)
    })
    .await
    .map_err(|_| mx::ErrorKind::BuildError("staged pre-build task was cancelled".to_string()))??;

    meta.built = true;
    write_meta(&meta)?;
    Ok(meta)
}

/// Applies the staged update by making it the next boot's system.
///
/// The cheap half of an update, meant to run from a shutdown unit: the closure
/// was realised by [`stage_update`], so `nixos-rebuild boot` only has to write
/// the bootloader entry and the system profile. **The running system is never
/// switched** - that is the point: a staged update is applied by rebooting into
/// it, not by replacing the closure of a live session.
///
/// Once the new system is the next boot's default, the candidate lockfile is
/// committed to `config_dir` with no rebuild attached
/// ([`make_transaction_commit_only`]), restoring the invariant that the
/// committed lockfile is the one the system boots.
///
/// # Arguments
/// * `config_dir` - configuration repository the candidate is promoted into.
///   Must be the repository the candidate was staged from.
/// * `cores` - caps the activation's residual build to this many CPU cores;
///   `None` leaves the Nix default.
///
/// # Pre-conditions
/// `/nix/store` and the bootloader's filesystem must still be mounted, so a
/// shutdown unit has to be ordered before `umount.target`. Must not run
/// concurrently with [`stage_update`].
///
/// # Post-conditions
/// On success the staging area is discarded and the configuration repository
/// carries a commit pinning the applied revisions; the running system is
/// unchanged until the machine reboots. On a failed activation nothing is
/// committed and the staging area is **kept**, so the next shutdown can retry
/// without re-downloading anything. In a debug build `BuildCommand::Boot` maps
/// to `build-vm`, so this activates nothing - same convention as every other
/// rebuild in this crate.
///
/// # Returns
/// `true` when a staged update was applied, `false` when there was nothing
/// usable to apply (nothing staged, or staged but no longer built) - which is
/// the normal case on most shutdowns and is not an error.
///
/// # Errors
/// [`mx::ErrorKind::BuildError`] with the activation's standard error if
/// `nixos-rebuild boot` exits non-zero, [`mx::ErrorKind::IOError`] if the
/// staged lockfile cannot be read, plus anything
/// [`make_transaction_commit_only`] reports for the promotion.
pub fn apply_staged(config_dir: &str, cores: Option<u32>) -> mx::Result<bool> {
    let Some(meta) = staged_status()? else {
        return Ok(false);
    };
    if !meta.built {
        return Ok(false);
    }
    let Some(lock) = staged_lock()? else {
        return Ok(false);
    };

    let dir = staging_dir();
    let staged_config = dir.join(STAGED_CONFIG_DIR);
    run_staged_rebuild(&dir, &staged_config, BuildCommand::Boot, cores)?;

    make_transaction_commit_only(
        "apply staged system update",
        config_dir,
        FILE_FLAKE_PATH,
        UpdateInput::UseLock(lock),
        update_no_transaction,
    )?;

    discard_staged()?;
    Ok(true)
}

/// Runs `nixos-rebuild <command>` against the staged configuration copy.
///
/// # Arguments
/// * `dir` - staging directory, used as the rebuild's working directory so the
///   `result` symlink - and the garbage-collection root with it - lands there.
/// * `staged_config` - the configuration copy inside `dir`, pointed at with
///   `--flake`.
/// * `command` - [`BuildCommand::Build`] to pre-build,
///   [`BuildCommand::Boot`] to make the result the next boot's system.
/// * `cores` - forwarded as `--cores`.
///
/// # Pre-conditions
/// `staged_config` must hold a complete configuration copy with the candidate
/// lockfile already written.
///
/// # Post-conditions
/// Takes a turn in the shared build queue first, so it never runs concurrently
/// with another process's rebuild, and blocks until the rebuild finishes.
///
/// # Errors
/// [`mx::ErrorKind::BuildError`] with the rebuild's standard error on a
/// non-zero exit, plus anything [`BuildQueue::enqueue`] reports.
fn run_staged_rebuild(
    dir: &path::Path,
    staged_config: &path::Path,
    command: BuildCommand,
    cores: Option<u32>,
) -> mx::Result<()> {
    let ticket = BuildQueue::enqueue()?;
    ticket.wait_turn()?;

    let mut stderr = String::new();
    let success = Transaction::rebuild_config(
        &staged_config.to_string_lossy(),
        CONFIG_NAME,
        command,
        Some(&mut stderr),
        cores,
        Some(&dir.to_string_lossy()),
    )?;

    if success {
        Ok(())
    } else {
        Err(mx::ErrorKind::BuildError(stderr))
    }
}

/// Recovers what an interrupted transaction left behind in `config_dir`.
///
/// Neither `Transaction` nor `NixFile` implements `Drop`, so a process killed
/// mid-transaction - which is what a `switch` that restarts the daemon that
/// asked for it does - leaves its auto-stash in the stash list with the
/// caller's uncommitted work inside, and nothing notices. This is the step that
/// does, for a daemon to run at start-up before serving anything.
///
/// # Arguments
/// * `config_dir` - configuration repository to inspect.
///
/// # Pre-conditions
/// No transaction may be open on `config_dir`, in this process or any other: a
/// stash pushed by a live transaction cannot be told apart from an abandoned
/// one. Start-up, before the first request is served, is the safe point.
///
/// # Post-conditions
/// Only the stash entries this crate created are touched, and only while they
/// sit on top of the stack; a stash the user pushed themselves is left alone.
/// `HEAD` never moves, and no rebuild runs - a repository committed ahead of
/// the running system is reported by the return value, not "fixed", since the
/// configuration is what the user asked for and only a rebuild could reconcile
/// it.
///
/// # Returns
/// How many stash entries were recovered; `0` in the normal case.
///
/// # Errors
/// [`mx::ErrorKind::GitError`] if `config_dir` cannot be opened or its stash
/// list cannot be walked.
pub fn repair_after_crash(config_dir: &str) -> mx::Result<usize> {
    Transaction::restore_orphan_stashes(config_dir)
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;
