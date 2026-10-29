//! Whether the running system still needs a reboot to be fully in effect.
//!
//! A `nixos-rebuild switch` replaces the system closure under a live session,
//! but it cannot replace the kernel, the kernel modules or the initrd the
//! machine is *currently running*: those only change at the next boot. So an
//! update can succeed and leave the system genuinely up to date while a reboot
//! is still needed - and a caller that reports "updated, restart to finish"
//! unconditionally is as wrong as one that never reports it.
//!
//! # Why symlinks and not a closure diff
//!
//! The question has an exact answer: NixOS keeps `/run/booted-system` pointing
//! at the closure the machine booted and `/run/current-system` at the one it
//! runs now, and both expose `kernel`, `kernel-modules` and `initrd` as
//! symlinks into the store. Comparing those three settles it with no list of
//! "reboot-worthy" package names to keep up to date, and so cannot miss a
//! driver nobody thought of. It also spawns nothing: describing *what* moved
//! is a separate question, and not this module's.
//!
//! `systemd` is deliberately not among the three: `switch-to-configuration`
//! re-executes it in place, so a new systemd needs no reboot.

use std::fs;
use std::path::Path;

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

#[cfg(test)]
#[path = "reboot_tests.rs"]
mod tests;
