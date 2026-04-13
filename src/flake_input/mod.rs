use crate::{
    core::{
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

pub enum FlakeInput {
    Nixpkgs,
    Modulix,
    Other(String),
}

impl FlakeInput {
    pub fn as_str(&self) -> &str {
        match self {
            FlakeInput::Nixpkgs => "nixpkgs",
            FlakeInput::Modulix => "modulix-os/nixpkgs",
            FlakeInput::Other(url) => url,
        }
    }
}

const FLAKE_INPUT_FILE: &str = "flake.nix";

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
        |file| add_input_no_transaction(file, input_name, input, follower),
    )
}

pub fn set_follower_no_transaction(
    file: &mut NixFile,
    input_name: &str,
    follower: FlakeInput,
) -> mx::Result<()> {
    mxOption::new(&format!("inputs.{}.follows", input_name))
        .set(file, &format!("\"{}\"", follower.as_str()))?;
    Ok(())
}

pub fn set_follower(config_dir: &str, input_name: &str, follower: FlakeInput) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Set follower to input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        |file| set_follower_no_transaction(file, input_name, follower),
    )
}

pub fn remove_follower_no_transaction(file: &mut NixFile, input_name: &str) -> mx::Result<bool> {
    mxOption::new(&format!("inputs.{}.follows", input_name))
        .set_option_all_instance_to_default(file)
}

pub fn remove_follower(config_dir: &str, input_name: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove follower from input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        |file| remove_follower_no_transaction(file, input_name),
    )
}

pub fn remove_input_no_transaction(file: &mut NixFile, input_name: &str) -> mx::Result<bool> {
    mxOption::new(&format!("inputs.{}", input_name)).set_option_all_instance_to_default(file)
}

pub fn remove_input(config_dir: &str, input_name: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove input {}", input_name),
        config_dir,
        FLAKE_INPUT_FILE,
        BuildCommand::Switch,
        |file| remove_input_no_transaction(file, input_name),
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
