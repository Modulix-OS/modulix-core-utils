//! Whether the running system still needs a reboot, and what moved since it
//! booted.
//!
//! A `nixos-rebuild switch` replaces the system closure under a live session,
//! but it cannot replace the kernel, the kernel modules or the initrd the
//! machine is *currently running*: those only change at the next boot. So an
//! `UpdateSystem("switch")` can succeed and leave the system genuinely
//! up to date while a reboot is still needed - and a caller that reports
//! "updated, restart to finish" unconditionally is as wrong as one that never
//! reports it.
//!
//! # Why symlinks and not the closure diff
//!
//! The question has an exact answer: NixOS keeps `/run/booted-system` pointing
//! at the closure the machine booted and `/run/current-system` at the one it
//! runs now, and both expose `kernel`, `kernel-modules` and `initrd` as
//! symlinks into the store. Comparing those three settles it with no list of
//! "reboot-worthy" package names to keep up to date, and so cannot miss a
//! driver nobody thought of. [`changed_since_boot`] runs
//! `nix store diff-closures` on top of that, but only to *describe* the change
//! - never to decide it.
//!
//! `systemd` is deliberately not among the three: `switch-to-configuration`
//! re-executes it in place, so a new systemd needs no reboot.
//!
//! # Relation to `mx-latest-update`
//!
//! `mx-latest-update` (mxpkgs) diffs the last two *system profile
//! generations*. [`changed_since_boot`] runs the same `nix store
//! diff-closures` but between the booted and the current system, which is a
//! deliberate difference: a generation diff describes the last activation,
//! whereas "what changed since boot" is the question a reboot prompt answers -
//! and it stays right after several switches in one session.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::core::nix_eval;
use crate::mx;

/// Closure the machine booted into. Immutable for the lifetime of the boot.
const BOOTED_SYSTEM: &str = "/run/booted-system";

/// Closure the machine runs now. Re-pointed by every `nixos-rebuild switch`.
const CURRENT_SYSTEM: &str = "/run/current-system";

/// The parts of a system closure that only take effect at boot, and so decide
/// whether a reboot is still needed.
///
/// `kernel` and `initrd` are what the bootloader hands control to;
/// `kernel-modules` is the tree the running kernel loads from, which must match
/// it. See the module documentation for why `systemd` is not listed.
const BOOT_CRITICAL: [&str; 3] = ["kernel", "kernel-modules", "initrd"];

/// Ceiling on the `nix store diff-closures` invocation.
///
/// Generous: the command only reads the local store database, but it walks two
/// full system closures and can be slow on a cold page cache.
const DIFF_TIMEOUT: Duration = Duration::from_secs(60);

/// Memo for [`reboot_status`], keyed on what `/run/current-system` points at.
///
/// That target changes on every `nixos-rebuild switch` and nothing else, so a
/// hit is exact rather than merely fresh. `None` means nothing has been
/// computed yet; the key is `None` when `/run/current-system` cannot be read,
/// which still memoises correctly (a system without it has nothing to diff).
static STATUS_MEMO: Mutex<Option<(Option<PathBuf>, RebootStatus)>> = Mutex::new(None);

/// What the running system still owes a reboot, and why.
///
/// # Fields
/// * `required` - the running kernel, kernel modules or initrd are not the ones
///   the current system closure declares, so a reboot is needed to pick them
///   up. Exact, and independent of `changed`.
/// * `changed` - one entry per line of `nix store diff-closures`, describing
///   what moved between the booted and the current system. Purely
///   informational, and **empty when the diff could not be run** - never read
///   it to infer `required`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebootStatus {
    pub required: bool,
    pub changed: Vec<String>,
}

/// Tells whether the running system needs a reboot to be fully in effect.
///
/// Compares `/run/booted-system` and `/run/current-system` on each of
/// [`BOOT_CRITICAL`].
///
/// # Pre-conditions
/// None. On a system without `/run/booted-system` (a container, a non-NixOS
/// host) every entry is missing on both sides and the answer is `false`.
///
/// # Post-conditions
/// Read-only: resolves symlinks, spawns nothing, and does not block.
///
/// # Returns
/// `true` as soon as one of [`BOOT_CRITICAL`] resolves differently on the two
/// sides, or exists on exactly one of them; `false` when all three match or are
/// absent from both.
pub fn reboot_required() -> bool {
    required_between(Path::new(BOOTED_SYSTEM), Path::new(CURRENT_SYSTEM))
}

/// Body of [`reboot_required`], with both system directories injected.
///
/// Split out so the comparison can be tested against temporary trees instead of
/// the host's `/run`.
///
/// # Parameters
/// * `booted` - directory standing in for `/run/booted-system`.
/// * `current` - directory standing in for `/run/current-system`.
///
/// # Post-conditions
/// Read-only, as [`reboot_required`].
///
/// # Returns
/// As [`reboot_required`], for the two directories given.
fn required_between(booted: &Path, current: &Path) -> bool {
    BOOT_CRITICAL.iter().any(|name| {
        let before = fs::read_link(booted.join(name)).ok();
        let after = fs::read_link(current.join(name)).ok();
        before != after
    })
}

/// Lists what moved between the booted system and the current one.
///
/// Runs `nix store diff-closures /run/booted-system /run/current-system` and
/// returns its output line by line, e.g. `linux: 6.6.1 -> 6.6.2`.
///
/// # Pre-conditions
/// `nix` must be on `PATH`. `nix-command` is requested explicitly, so the
/// system's configured experimental features do not have to include it.
///
/// # Post-conditions
/// Spawns one `nix` process and blocks on it for at most [`DIFF_TIMEOUT`];
/// writes nothing. Dropping the future kills the child
/// ([`nix_eval::run`]).
///
/// # Returns
/// One entry per non-empty output line, trimmed, in the order `nix` printed
/// them. Empty when the two closures are identical.
///
/// # Errors
/// Whatever [`nix_eval::run`] reports: [`mx::ErrorKind::NixCommandError`] on a
/// non-zero exit or a timeout, [`mx::ErrorKind::IOError`] when `nix` cannot be
/// spawned, [`mx::ErrorKind::FromUtf8Error`] on non-UTF-8 output.
pub async fn changed_since_boot() -> mx::Result<Vec<String>> {
    let stdout = nix_eval::run(
        &[
            "--extra-experimental-features",
            "nix-command",
            "store",
            "diff-closures",
            BOOTED_SYSTEM,
            CURRENT_SYSTEM,
        ],
        DIFF_TIMEOUT,
    )
    .await?;

    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Full reboot status: the exact verdict, plus a description of what changed.
///
/// [`reboot_required`] decides, [`changed_since_boot`] describes. A failure of
/// the latter degrades `changed` to an empty list - logged nowhere, since this
/// crate does not log - and never affects `required`.
///
/// # Pre-conditions
/// None. `nix` missing from `PATH` only empties `changed`.
///
/// # Post-conditions
/// Memoised on what `/run/current-system` points at, so the `nix` process runs
/// at most once per activation however often this is called. Nothing is
/// written.
///
/// # Returns
/// The status of the running system. Never fails: an unreadable `/run` yields
/// `RebootStatus { required: false, changed: vec![] }`, which is what a caller
/// would do with the error anyway.
pub async fn reboot_status() -> RebootStatus {
    let key = fs::read_link(CURRENT_SYSTEM).ok();

    if let Some(hit) = memo_get(&key) {
        return hit;
    }

    let status = RebootStatus {
        required: reboot_required(),
        changed: changed_since_boot().await.unwrap_or_default(),
    };

    memo_set(key, status.clone());
    status
}

/// Reads [`STATUS_MEMO`] if it was filled for this exact system closure.
///
/// # Parameters
/// * `key` - target of `/run/current-system`, or `None` when unreadable.
///
/// # Post-conditions
/// Holds [`STATUS_MEMO`] for the duration of the read only; never across an
/// `await`.
///
/// # Returns
/// The memoised status when it was recorded for `key`, `None` otherwise
/// (including when the lock is poisoned, which just means recomputing).
fn memo_get(key: &Option<PathBuf>) -> Option<RebootStatus> {
    let guard = STATUS_MEMO.lock().ok()?;
    guard
        .as_ref()
        .filter(|(stored, _)| stored == key)
        .map(|(_, status)| status.clone())
}

/// Records `status` in [`STATUS_MEMO`] for the closure `key` identifies.
///
/// # Parameters
/// * `key` - target of `/run/current-system`, or `None` when unreadable.
/// * `status` - what [`reboot_status`] computed for it.
///
/// # Post-conditions
/// [`STATUS_MEMO`] holds `(key, status)`, replacing any previous entry. A
/// poisoned lock is ignored: the memo is an optimisation, not state anyone
/// depends on.
fn memo_set(key: Option<PathBuf>, status: RebootStatus) {
    if let Ok(mut guard) = STATUS_MEMO.lock() {
        *guard = Some((key, status));
    }
}

#[cfg(test)]
#[path = "reboot_tests.rs"]
mod tests;
