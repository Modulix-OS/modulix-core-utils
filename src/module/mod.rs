use crate::{
    core::{
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const MODULE_FILE_PATH: &str = "module.nix";

fn make_module_path(module_name: &str) -> String {
    format!("mx.{}.enable", module_name)
}

pub fn install_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    let module_path = make_module_path(module_name);
    let module = mxOption::new(&module_path);
    module.set(file, "true")?;
    Ok(())
}

pub fn install(config_dir: &str, module_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install module {}", module_name),
        config_dir,
        MODULE_FILE_PATH,
        BuildCommand::Switch,
        |file| install_no_transaction(file, module_name),
    )
}

pub fn uninstall_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    let module_path = make_module_path(module_name);
    let module = mxOption::new(&module_path);
    module.set(file, "false")?;
    Ok(())
}

pub fn uninstall(config_dir: &str, module_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall module {}", module_name),
        config_dir,
        MODULE_FILE_PATH,
        BuildCommand::Switch,
        |file| uninstall_no_transaction(file, module_name),
    )
}
