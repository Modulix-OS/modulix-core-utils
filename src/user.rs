//! Declares and removes system users in the configuration's `users.nix`.
//!
//! Only the declarative side: NixOS creates, and stops creating, the account
//! on the next rebuild. Reading the users that already exist on the machine is
//! `crate::core::user`'s job.

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    mx,
};

/// Configuration file, relative to the config directory, holding
/// `users.users.*`.
pub(crate) const USER_FILE_PATH: &str = "users.nix";

/// Declares a user in an already-open `users.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `username` - login name, used as the `users.users` attribute key; it is
///   inserted unquoted, so it must be a valid Nix identifier.
/// * `initial_password` - plaintext password NixOS sets on first creation
///   only; it stays readable in the configuration and in the Nix store, so it
///   is meant to be changed after the first login.
/// * `description` - GECOS field, written as an indented Nix string.
/// * `shell` - login shell as a Nix expression path (e.g. `/run/current-system/sw/bin/bash`).
/// * `extra_groups` - supplementary groups added to `extraGroups`; `wheel` is
///   what grants sudo, so passing it makes the account an administrator.
/// * `is_normal_user` - value of `isNormalUser`: true for a human account
///   (home directory, normal UID range), false for a service account.
///
/// # Post-conditions
/// `createHome` is forced to true and the primary group to `users`. Groups are
/// only ever added: a group the user already had is kept, and one that is no
/// longer wanted is not removed. Re-declaring an existing user overwrites its
/// scalar options, `initialPassword` included.
pub fn add_no_transaction(
    file: &mut NixFile,
    username: &str,
    initial_password: &str,
    description: &str,
    shell: &str,
    extra_groups: &[&str],
    is_normal_user: bool,
) -> mx::Result<()> {
    let root_option = format!("users.users.{}", username);

    mxOption::new(&format!("{}.isNormalUser", root_option))
        .set(file, if is_normal_user { "true" } else { "false" })?;
    mxOption::new(&format!("{}.initialPassword", root_option))
        .set(file, &format!("\"{}\"", initial_password))?;
    mxOption::new(&format!("{}.createHome", root_option)).set(file, "true")?;
    mxOption::new(&format!("{}.group", root_option)).set(file, "\"users\"")?;
    mxOption::new(&format!("{}.description", root_option))
        .set(file, &format!("\'\'{}\'\'", description))?;
    mxOption::new(&format!("{}.shell", root_option)).set(file, &format!("\"{}\"", shell))?;

    let extra_group_name = &format!("{}.extraGroups", root_option);
    let extra_groups_list = mxList::new(extra_group_name, true);
    for group in extra_groups {
        extra_groups_list.add(file, &format!("\"{}\"", group))?;
    }

    Ok(())
}

/// Removes a user's declaration from an already-open `users.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `username` - login name whose `users.users` entry must go.
///
/// # Returns
/// `true` if at least one declaration was removed, `false` if the user was not
/// declared - which is not an error.
///
/// # Post-conditions
/// The declaration goes, with every sub-option. The account's home directory
/// and its files are left on disk: NixOS stops managing the user, it does not
/// erase it.
pub fn remove_no_transaction(file: &mut NixFile, username: &str) -> mx::Result<bool> {
    let root_option = format!("users.users.{}", username);
    mxOption::new(&root_option).set_option_all_instance_to_default(file)
}

/// Declares a user and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `username`, `initial_password`, `description`, `shell`, `extra_groups`,
///   `is_normal_user` - as in [`add_no_transaction`].
///
/// # Post-conditions
/// On success the account exists on the active generation; on error the
/// configuration is rolled back. Blocks for the whole `nixos-rebuild switch`.
pub fn add(
    config_dir: &str,
    username: &str,
    initial_password: &str,
    description: &str,
    shell: &str,
    extra_groups: &[&str],
    is_normal_user: bool,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add user {}", username),
        config_dir,
        USER_FILE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| {
            add_no_transaction(
                file,
                username,
                initial_password,
                description,
                shell,
                extra_groups,
                is_normal_user,
            )
        },
    )
}

/// Removes a user's declaration and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `username` - login name to undeclare.
///
/// # Returns
/// `true` if a declaration was removed, `false` if there was none - the
/// rebuild runs either way.
///
/// # Post-conditions
/// As in [`remove_no_transaction`]; the home directory survives the rebuild.
pub fn remove(config_dir: &str, username: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove user {}", username),
        config_dir,
        USER_FILE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_no_transaction(file, username),
    )
}
