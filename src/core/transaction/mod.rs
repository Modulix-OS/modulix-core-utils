//! The transactional core: a configuration edit and the `nixos-rebuild` that
//! applies it, as one revertible unit.
//!
//! Three layers stack up here: [`file_lock::NixFile`] makes a single file's
//! rewrite atomic, [`Transaction`] turns a set of files plus a rebuild into one
//! git-versioned step, and `build_queue` serialises the rebuilds of concurrent
//! processes. Domain modules do not drive them by hand: they call
//! [`make_transaction`] (or [`make_transaction_read_only`]) with a closure.

mod build_queue;
pub mod file_lock;
pub mod transaction;

use crate::{
    core::transaction::transaction::{BuildCommand, TransactionPermission, UpdateInput},
    mx,
};
use file_lock::NixFile;
pub use transaction::Transaction;

/// High-level entry point for performing an operation on a Nix file within an
/// atomic transaction.
///
/// This function orchestrates the entire transaction lifecycle: creation,
/// adding the target file, opening, running the caller-provided business logic,
/// then automatic commit or rollback depending on the result.
///
/// # Behavior
/// 1. Creates a new [`Transaction`] with the given description and build command.
/// 2. Adds `file_path` to the transaction and opens it (`begin`).
/// 3. Passes the corresponding [`NixFile`] to the closure `f`.
/// 4. If `f` returns `Ok` → [`Transaction::commit`] is called.
/// 5. If `f` returns `Err`, or if `get_file` fails → [`Transaction::rollback`] is called.
///
/// # Arguments
/// * `description`     – Human-readable label of the transaction (used for logs / history).
/// * `config_dir`      – Root directory of the NixOS configuration.
/// * `file_path`       – Relative path of the Nix file to edit.
/// * `build_command`   – Command to run after the commit (e.g. `nixos-rebuild switch`).
/// * `updated_input`   – How `flake.lock` is refreshed by the commit: keep every
///   input pinned, update them all, or update the named ones only.
/// * `f`               – Closure receiving the open [`NixFile`]; must return `mx::Result<R>`.
///
/// # Type parameters
/// * `F` – the closure type.
/// * `R` – value the closure produces, handed back on success.
///
/// # Post-conditions
/// Blocks for the whole rebuild, which can take minutes and is serialised
/// against the other processes' rebuilds. On any error the configuration is
/// back to its previous commit, and no file lock is left held.
///
/// # Returns
/// Returns `Ok(R)` if the transaction completed successfully, or an
/// `mx::ErrorKind` on failure at any step.
///
/// # Example
/// ```ignore
/// make_transaction(
///     "enable nginx",
///     "/etc/nixos",
///     "/services/nginx.nix",
///     BuildCommand::Switch,
///     |file| {
///         let content = file.get_mut_file_content()?;
///         content.push_str("  services.nginx.enable = true;\n");
///         Ok(())
///     },
/// )?;
/// ```
pub fn make_transaction<F, R>(
    description: &str,
    config_dir: &str,
    file_path: &str,
    build_command: BuildCommand,
    updated_input: UpdateInput,
    f: F,
) -> mx::Result<R>
where
    F: FnOnce(&mut NixFile) -> mx::Result<R>,
{
    let mut transaction = Transaction::new(
        config_dir,
        description,
        build_command,
        TransactionPermission::Writtable,
    )?;
    transaction.add_file(file_path)?;
    transaction.begin()?;

    let file = match transaction.get_file_mut(file_path) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };
    match f(file) {
        Ok(ret) => {
            transaction.commit(updated_input)?;
            Ok(ret)
        }
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

/// Read-only counterpart of [`make_transaction`]: opens the file without the
/// right to modify it, for a caller that only needs to read the configuration
/// under the transaction's locks.
///
/// # Arguments
/// * `description` – Human-readable label of the transaction.
/// * `config_dir` – Root directory of the NixOS configuration.
/// * `file_path` – Relative path of the Nix file to read.
/// * `build_command` – Command the transaction is built with; no rebuild
///   actually runs, since a read-only transaction produces no change to commit.
/// * `f` – Closure receiving the open [`NixFile`] by shared reference.
///
/// # Type parameters
/// * `F` – the closure type.
/// * `R` – value the closure produces, handed back on success.
///
/// # Returns
/// `Ok(R)` with the closure's value, or the first error met.
///
/// # Post-conditions
/// The file is left untouched: an attempt to edit it through
/// `get_mut_file_content` fails with [`mx::ErrorKind::PermissionDenied`]. The
/// file lock is released either way.
pub fn make_transaction_read_only<F, R>(
    description: &str,
    config_dir: &str,
    file_path: &str,
    build_command: BuildCommand,
    f: F,
) -> mx::Result<R>
where
    F: FnOnce(&NixFile) -> mx::Result<R>,
{
    let mut transaction = Transaction::new(
        config_dir,
        description,
        build_command,
        TransactionPermission::ReadOnly,
    )?;
    transaction.add_file(file_path)?;
    transaction.begin()?;

    let file = match transaction.get_file(file_path) {
        Ok(file) => file,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };
    match f(file) {
        Ok(ret) => {
            transaction.commit(UpdateInput::Keep)?;
            Ok(ret)
        }
        Err(e) => {
            transaction.rollback()?;
            Err(e)
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
