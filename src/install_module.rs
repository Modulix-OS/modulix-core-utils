use crate::core::app_info_trait::AppInfoMinimal;
use crate::core::transaction::transaction::UpdateInput;
use crate::module_info::ModuleInfo;
use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const FILE_MODULE_PATH: &str = "module.nix";

fn enable_path(module_name: &str) -> String {
    format!("mx.{}.enable", module_name)
}

fn plugins_path(module_name: &str) -> String {
    format!("mx.{}.plugins", module_name)
}

pub fn install_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set(file, "true")?;
    Ok(())
}

pub fn uninstall_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set_option_to_default(file)?;
    Ok(())
}

pub fn install_plugin_no_transaction(
    file: &mut NixFile,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set(file, "true")?;
    mxList::new(&plugins_path(module_name), true)
        .add(file, &format!("pkgs.{}.{}", plugin_namespace, plugin_name))?;
    Ok(())
}

pub fn remove_plugin_no_transaction(
    file: &mut NixFile,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    mxList::new(&plugins_path(module_name), true)
        .remove(file, &format!("pkgs.{}.{}", plugin_namespace, plugin_name))?;
    Ok(())
}

pub fn install(config_dir: &str, module_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install module {}", module_name),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_no_transaction(file, module_name),
    )
}

pub fn uninstall(config_dir: &str, module_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall module {}", module_name),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| uninstall_no_transaction(file, module_name),
    )
}

pub fn install_plugin(
    config_dir: &str,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install {} plugin for module {}", plugin_name, module_name),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_plugin_no_transaction(file, module_name, plugin_namespace, plugin_name),
    )
}

pub fn remove_plugin(
    config_dir: &str,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove {} plugin for module {}", plugin_name, module_name),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_plugin_no_transaction(file, module_name, plugin_namespace, plugin_name),
    )
}

/// Collect the names of every module enabled via `mx.<name>.enable = true;`.
fn enabled_module_names(file: &NixFile) -> mx::Result<Vec<String>> {
    let mut names = Vec::new();
    for module in mxOption::new("mx").list_children(file)? {
        match mxOption::new(&enable_path(&module)).get(file) {
            Ok(value) if value.trim() == "true" => names.push(module),
            Ok(_) | Err(mx::ErrorKind::OptionNotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(names)
}

/// Names of the modules currently enabled in the configuration (`mx.*.enable`),
/// read from `module.nix` without resolving remote metadata.
pub fn list_enabled_module_names(config_dir: &str) -> mx::Result<Vec<String>> {
    transaction::make_transaction_read_only(
        "List enabled modules",
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Boot,
        enabled_module_names,
    )
}

/// Installed (enabled) Modulix modules, resolved against the remote module index
/// so GUI metadata (display name, summary, icon) is available. Modules enabled
/// locally but absent from the index are skipped.
pub async fn list_installed_modules(config_dir: &str) -> mx::Result<Vec<ModuleInfo>> {
    let names = list_enabled_module_names(config_dir)?;
    let mut modules = Vec::with_capacity(names.len());
    for name in names {
        if let Ok(module) = ModuleInfo::new(&name).await {
            modules.push(module);
        }
    }
    Ok(modules)
}
