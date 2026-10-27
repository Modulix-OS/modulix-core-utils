use crate::{
    CONFIG_DIRECTORY,
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

pub const FILE_HOSTNAME_PATH: &str = "configuration.nix";

pub fn change_hostname_no_transaction(file: &mut NixFile, new_hostname: &str) -> mx::Result<()> {
    mxOption::new("networking.hostName")
        .set(file, &format!("\"{}\"", new_hostname))
        .map(|_| ())
}

pub fn change_hostname(new_hostname: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Update hostname to {}", new_hostname),
        CONFIG_DIRECTORY,
        FILE_HOSTNAME_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |f| change_hostname_no_transaction(f, new_hostname),
    )
}
