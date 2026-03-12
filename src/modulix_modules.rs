use crate::{
    core::{
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const FILE_MODULE_PATH: &str = "modules.nix";

pub fn add_module_no_transaction(nix_file: &mut NixFile, module_path: &str) -> mx::Result<()> {
    mxOption::new(&format!("modulix.modules.{}.enable", module_path)).set(nix_file, "true")?;
    Ok(())
}

pub fn remove_module_no_transaction(nix_file: &mut NixFile, module_path: &str) -> mx::Result<()> {
    mxOption::new(&format!("modulix.modules.{}.enable", module_path))
        .set_option_to_default(nix_file)?;
    Ok(())
}

pub fn add_modules(config_dir: &str, module_path: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add module {}", module_path),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        |file| add_module_no_transaction(file, module_path),
    )
}

pub fn remove_modules(config_dir: &str, module_path: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add module {}", module_path),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        |file| remove_module_no_transaction(file, module_path),
    )
}
