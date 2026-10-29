//! System update: refreshing the configuration's flake inputs and rebuilding
//! the system from them.
//!
//! On a flake-based NixOS system, "updating" is a single operation -
//! `nix flake update` followed by a rebuild - rather than a per-package
//! action, so this module has no notion of updating one input in isolation:
//! [`update`] always refreshes every input via `UpdateInput::UpdateAll`.
//!
//! The transaction it drives edits no file: there is nothing to change in
//! `flake.nix` itself. The refresh happens in the commit, which runs
//! `nix flake update` for any `UpdateInput` other than `Keep` even when no
//! tracked file moved, and then creates a commit and a rebuild only if that
//! refresh actually moved `flake.lock`.

pub use crate::core::transaction::transaction::BuildCommand;
use crate::core::transaction::transaction::{Transaction, TransactionPermission, UpdateInput};
use crate::mx;

/// Relative path, under a configuration directory, of the flake this module
/// refreshes.
const FILE_FLAKE_PATH: &str = "flake.nix";

/// Refreshes every flake input of a configuration and applies the result.
///
/// # Arguments
/// * `config_dir` - configuration repository to update; must be a git
///   repository holding a `flake.nix`.
/// * `build_command` - `Switch` to rebuild and switch immediately, `Boot` to
///   only prepare the next boot.
/// * `cores` - caps the rebuild's `nix` build to this many CPU cores
///   (`nixos-rebuild --cores`); `None` leaves the Nix default (all of them).
///
/// # Pre-conditions
/// `config_dir` must be committable: an uncommitted change that cannot be
/// stashed surfaces as [`mx::ErrorKind::GitNotCommitted`].
///
/// # Post-conditions
/// If `nix flake update` leaves `flake.lock` unchanged (every input was
/// already current), no commit is created and no rebuild runs. `flake.nix`
/// itself is never edited, only locked for the duration of the transaction.
/// Blocks for the whole `nix flake update` plus, if anything changed, the
/// whole rebuild, which is serialised against the other processes' rebuilds.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if `nix flake update` cannot be run,
/// [`mx::ErrorKind::BuildError`] with the rebuild's standard error if
/// `nixos-rebuild` exits non-zero, plus anything
/// [`Transaction::begin`] and [`Transaction::commit`] report. On any failure
/// the repository is back to its previous commit and no file lock is left
/// held.
pub fn update(config_dir: &str, build_command: BuildCommand, cores: Option<u32>) -> mx::Result<()> {
    let mut transaction = Transaction::new(
        config_dir,
        "update system inputs",
        build_command,
        TransactionPermission::Writtable,
    )?;
    transaction.set_cores(cores);
    transaction.add_file(FILE_FLAKE_PATH)?;
    transaction.begin()?;
    transaction.commit(UpdateInput::UpdateAll)
}
