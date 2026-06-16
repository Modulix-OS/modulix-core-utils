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
/// * `f`               – Closure receiving the open [`NixFile`]; must return `mx::Result<R>`.
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
