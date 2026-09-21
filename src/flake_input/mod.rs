//! Declares and edits the flake inputs of the configuration's `flake.nix`.
//!
//! As elsewhere in the crate, each operation exists in a `*_no_transaction`
//! form and in a wrapper opening its own transaction. The wrappers commit with
//! `UpdateInput::UpdateSelected`, so only the input they touched is refreshed
//! in `flake.lock`.

use crate::{
    core::{
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    mx,
};

/// An input another input can be made to follow.
///
/// # Variants
/// * `Nixpkgs` - the configuration's own `nixpkgs` input.
/// * `Modulix` - the nixpkgs Modulix itself pins (`modulix-os/nixpkgs`), so an
///   input shares the distribution's package set instead of its own.
/// * `Other` - any other follow target, as its input path.
pub enum FlakeInput {
    Nixpkgs,
    Modulix,
    Other(String),
}

impl FlakeInput {
    /// Follow target as it must be written in `flake.nix`.
    ///
    /// # Returns
    /// The input path, borrowed from `self` for `Other`.
    pub fn as_str(&self) -> &str {
        match self {
            FlakeInput::Nixpkgs => "nixpkgs",
            FlakeInput::Modulix => "modulix-os/nixpkgs",
            FlakeInput::Other(url) => url,
        }
    }
}

/// File, relative to the config directory, holding the flake's `inputs`.
const FLAKE_INPUT_FILE: &str = "flake.nix";

/// Declares a flake input in an already-open `flake.nix`.
///
/// # Parameters
/// * `file` - the open flake file to edit.
/// * `input_name` - attribute name the input gets under `inputs`.
/// * `input` - the flake reference (URL) to set as `inputs.<name>.url`;
///   quoting is added here, so pass it unquoted.
/// * `follower` - when `Some`, also sets `inputs.<name>.follows` so the input
///   reuses that target's revision instead of pinning its own.
///
/// # Post-conditions
/// An input already declared under the same name has its URL overwritten.
pub fn add_input_no_transaction(
    file: &mut NixFile,
    input_name: &str,
    input: &str,
    follower: Option<FlakeInput>,
) -> mx::Result<()> {
    mxOption::new(&format!("inputs.{}.url", input_name)).set(file, &format!("\"{}\"", input))?;
    if let Some(follower) = follower {
        mxOption::new(&format!("inputs.{}.follows", input_name))
            .set(file, &format!("\"{}\"", follower.as_str()))?;
    }
    Ok(())
}

/// Declares a flake input, locks it, and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `input_name`, `input`, `follower` - as in [`add_input_no_transaction`].
///
/// # Post-conditions
/// The commit refreshes `flake.lock` for `input_name` only, leaving the other
/// inputs pinned where they were. On error the configuration and the lock file
/// are rolled back. Blocks for the whole `nixos-rebuild switch`.
pub fn add_input(
    config_dir: &str,
    input_name: &str,
    input: &str,
    follower: Option<FlakeInput>,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        UpdateInput::UpdateSelected(vec![input_name.to_string()]),
        |file| add_input_no_transaction(file, input_name, input, follower),
    )
}

/// Makes an existing input follow another, in an already-open `flake.nix`.
///
/// # Parameters
/// * `file` - the open flake file to edit.
/// * `input_name` - input to constrain.
/// * `follower` - target it must follow.
///
/// # Post-conditions
/// A `follows` already set is overwritten. Nothing checks that `input_name` is
/// declared: the attribute is created if it is not.
pub fn set_follower_no_transaction(
    file: &mut NixFile,
    input_name: &str,
    follower: FlakeInput,
) -> mx::Result<()> {
    mxOption::new(&format!("inputs.{}.follows", input_name))
        .set(file, &format!("\"{}\"", follower.as_str()))?;
    Ok(())
}

/// Makes an input follow another, relocks it, and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `input_name`, `follower` - as in [`set_follower_no_transaction`].
///
/// # Post-conditions
/// As in [`add_input`]: only `input_name` is relocked.
pub fn set_follower(config_dir: &str, input_name: &str, follower: FlakeInput) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Set follower to input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        UpdateInput::UpdateSelected(vec![input_name.to_string()]),
        |file| set_follower_no_transaction(file, input_name, follower),
    )
}

/// Lets an input pin its own revision again, in an already-open `flake.nix`.
///
/// # Parameters
/// * `file` - the open flake file to edit.
/// * `input_name` - input whose `follows` must go.
///
/// # Returns
/// `true` if at least one `follows` was removed, `false` if there was none.
///
/// # Post-conditions
/// Every `follows` declared for this input is removed; its `url` is untouched.
pub fn remove_follower_no_transaction(file: &mut NixFile, input_name: &str) -> mx::Result<bool> {
    mxOption::new(&format!("inputs.{}.follows", input_name))
        .set_option_all_instance_to_default(file)
}

/// Removes an input's `follows`, relocks it, and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `input_name` - input whose `follows` must go.
///
/// # Returns
/// `true` if a `follows` was removed; `false` if there was nothing to remove,
/// in which case the rebuild still runs.
///
/// # Post-conditions
/// As in [`add_input`]: the input is relocked, this time on its own revision.
pub fn remove_follower(config_dir: &str, input_name: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove follower from input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        UpdateInput::UpdateSelected(vec![input_name.to_string()]),
        |file| remove_follower_no_transaction(file, input_name),
    )
}

/// Removes a flake input entirely, in an already-open `flake.nix`.
///
/// # Parameters
/// * `file` - the open flake file to edit.
/// * `input_name` - input to undeclare, with its `url` and `follows`.
///
/// # Returns
/// `true` if at least one declaration was removed, `false` if the input was
/// not declared.
///
/// # Post-conditions
/// References to the input elsewhere in the configuration (an `imports` entry,
/// for instance) are not cleaned up, and will make the next evaluation fail.
pub fn remove_input_no_transaction(file: &mut NixFile, input_name: &str) -> mx::Result<bool> {
    mxOption::new(&format!("inputs.{}", input_name)).set_option_all_instance_to_default(file)
}

/// Removes a flake input and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `input_name` - input to undeclare.
///
/// # Returns
/// `true` if a declaration was removed, `false` if there was none.
///
/// # Post-conditions
/// As in [`remove_input_no_transaction`]; should the configuration still
/// reference the input, the rebuild fails with
/// [`mx::ErrorKind::BuildError`] and everything is rolled back.
pub fn remove_input(config_dir: &str, input_name: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        UpdateInput::UpdateSelected(vec![input_name.to_string()]),
        |file| remove_input_no_transaction(file, input_name),
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
