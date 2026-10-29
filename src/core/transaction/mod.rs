//! The transactional core: a configuration edit and the `nixos-rebuild` that
//! applies it, as one revertible unit.
//!
//! Three layers stack up here: [`file_lock::NixFile`] makes a single file's
//! rewrite atomic, [`Transaction`] turns a set of files plus a rebuild into one
//! git-versioned step, and `build_queue` serialises the rebuilds of concurrent
//! processes. Domain modules do not drive them by hand: they call
//! [`make_transaction`] (or `make_transaction_read_only`) with a closure, and a
//! caller that has to change several files at once calls
//! `make_group_transaction`, which packs one operation per edit into a single
//! commit and a single rebuild.

pub(crate) mod build_queue;
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
#[cfg(any(feature = "install-module", feature = "install-package"))]
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
/// Subject line prefix of the commit a multi-operation group creates.
#[cfg(any(feature = "init", feature = "hardware-config", test))]
const GROUP_SUBJECT_PREFIX: &str = "modulix:";

/// Edit a [`TransactionOp`] applies to the file it names, boxed so operations
/// built from unrelated closures can share one vector.
///
/// # Type parameters
/// * `'f` – lifetime of what the closure borrows from its caller.
#[cfg(any(feature = "init", feature = "hardware-config", test))]
type NixFileEdit<'f> = Box<dyn FnOnce(&mut NixFile) -> mx::Result<()> + 'f>;

/// One edit of a grouped transaction: what to say about it, which file it
/// touches, and the edit itself.
///
/// Built with [`TransactionOp::new`] and handed to [`make_group_transaction`],
/// which packs a whole vector of them into a single git commit and a single
/// rebuild.
///
/// # Fields
/// * `description` – human-readable label of this single edit; it becomes one
///   bullet of the group's commit message.
/// * `file_path` – path, relative to the configuration repository, of the file
///   the edit applies to. Several operations may name the same file.
/// * `edit` – the edit itself, run on the open [`NixFile`] once the
///   transaction has begun.
///
/// # Type parameters
/// * `'f` – lifetime of what the edit closure borrows, so an operation can
///   capture the caller's data by reference.
#[cfg(any(feature = "init", feature = "hardware-config", test))]
pub struct TransactionOp<'f> {
    description: String,
    file_path: String,
    edit: NixFileEdit<'f>,
}

#[cfg(any(feature = "init", feature = "hardware-config", test))]
impl<'f> TransactionOp<'f> {
    /// Describes one edit of a grouped transaction.
    ///
    /// # Arguments
    /// * `description` – label of this edit, used as one bullet of the group's
    ///   commit message.
    /// * `file_path` – path of the file to edit, relative to the configuration
    ///   repository (e.g. `"firewall.nix"`).
    /// * `edit` – closure applied to the open [`NixFile`]; it edits the
    ///   in-memory buffer only, the group decides when it reaches the disk.
    ///
    /// # Type parameters
    /// * `F` – the closure type.
    ///
    /// # Returns
    /// The operation, inert until [`make_group_transaction`] runs it.
    pub fn new<F>(description: &str, file_path: &str, edit: F) -> Self
    where
        F: FnOnce(&mut NixFile) -> mx::Result<()> + 'f,
    {
        TransactionOp {
            description: description.to_string(),
            file_path: file_path.to_string(),
            edit: Box::new(edit),
        }
    }
}

/// Builds the commit message of a group from its operations' descriptions.
///
/// # Arguments
/// * `descriptions` – one description per operation, in the order they run.
///
/// # Returns
/// The lone description verbatim for a single operation, so a one-operation
/// group commits exactly like [`make_transaction`]; otherwise a counted
/// subject followed by one bullet per description. The empty string for an
/// empty slice, which [`make_group_transaction`] never commits.
#[cfg(any(feature = "init", feature = "hardware-config", test))]
fn group_message(descriptions: &[&str]) -> String {
    match descriptions {
        [] => String::new(),
        [only] => (*only).to_string(),
        many => {
            let mut message = format!(
                "{} {} configuration changes\n",
                GROUP_SUBJECT_PREFIX,
                many.len()
            );
            for description in many {
                message.push_str("\n- ");
                message.push_str(description);
            }
            message.push('\n');
            message
        }
    }
}

/// Applies several edits, over as many files as needed, as **one** git commit
/// and **one** rebuild.
///
/// The grouped counterpart of [`make_transaction`]: instead of one file and one
/// closure, it takes a vector of [`TransactionOp`]s, each carrying its own
/// description, target file and edit. Every file is locked for the whole group,
/// every edit is applied in memory, and the commit that follows records all of
/// them at once — so a caller that changes several configuration domains pays
/// for a single `nixos-rebuild` instead of one per domain.
///
/// # Behavior
/// 1. Registers each distinct `file_path`, in first-appearance order, then
///    opens the transaction (`begin`).
/// 2. Runs each operation in the order given, on the [`NixFile`] it names.
///    Several operations on one file see each other's edits, later ones
///    building on earlier ones.
/// 3. On success → [`Transaction::commit`]; on the first error →
///    [`Transaction::rollback`], and that error is returned.
///
/// # Arguments
/// * `config_dir` – root directory of the NixOS configuration.
/// * `build_command` – command to run after the commit (e.g. `nixos-rebuild
///   switch`), run once for the whole group.
/// * `updated_input` – how `flake.lock` is refreshed by the commit.
/// * `ops` – the operations to apply, in order. May name the same file more
///   than once, and may name `configuration.nix`, which
///   [`Transaction::begin`] registers anyway.
///
/// # Pre-conditions
/// `config_dir` must be a committable git repository, as for
/// [`make_transaction`].
///
/// # Post-conditions
/// An empty `ops` performs no I/O at all: no repository is opened, no file is
/// locked and nothing is committed. Otherwise either every edit is committed,
/// or none is: an error rolls the configuration back to its previous commit and
/// leaves no file lock held. Blocks for the whole rebuild, which is serialised
/// against the other processes' rebuilds.
///
/// # Returns
/// `Ok(())` once the group is committed (and rebuilt, unless nothing actually
/// changed), or the first `mx::ErrorKind` met.
///
/// # Example
/// ```ignore
/// make_group_transaction(
///     "/etc/nixos",
///     BuildCommand::Switch,
///     UpdateInput::Keep,
///     vec![
///         TransactionOp::new("open port 22", "firewall.nix", |file| {
///             firewall::add_port_no_transaction(file, 22)
///         }),
///         TransactionOp::new("install vim", "package.nix", |file| {
///             install_package::install_no_transaction(file, &["vim"])
///         }),
///     ],
/// )?;
/// ```
#[cfg(any(feature = "init", feature = "hardware-config", test))]
pub fn make_group_transaction(
    config_dir: &str,
    build_command: BuildCommand,
    updated_input: UpdateInput,
    ops: Vec<TransactionOp<'_>>,
) -> mx::Result<()> {
    if ops.is_empty() {
        return Ok(());
    }

    let message = {
        let descriptions: Vec<&str> = ops.iter().map(|op| op.description.as_str()).collect();
        group_message(&descriptions)
    };

    let mut transaction = Transaction::new(
        config_dir,
        &message,
        build_command,
        TransactionPermission::Writtable,
    )?;

    {
        let mut registered: Vec<&str> = Vec::new();
        for op in &ops {
            if !registered.contains(&op.file_path.as_str()) {
                transaction.add_file(&op.file_path)?;
                registered.push(op.file_path.as_str());
            }
        }
    }

    transaction.begin()?;

    for op in ops {
        let file = match transaction.get_file_mut(&op.file_path) {
            Ok(file) => file,
            Err(e) => {
                transaction.rollback()?;
                return Err(e);
            }
        };
        if let Err(e) = (op.edit)(file) {
            transaction.rollback()?;
            return Err(e);
        }
    }

    transaction.commit(updated_input)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
