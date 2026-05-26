use std::collections::HashMap;

#[cfg(feature = "app-info-gui")]
use tokio::sync::OnceCell;

use crate::core::app_info_trait::PLUGIN_NAMESPACES;

use crate::core::transaction::transaction::UpdateInput;
use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
    package_info::NixPackage,
};

const FILE_PACKAGE_PATH: &str = "package.nix";

pub fn install_no_transaction(file: &mut NixFile, package_name: &str) -> mx::Result<()> {
    if let Some(pkgs_info) = PLUGIN_NAMESPACES.get(package_name) {
        mxOption::new(pkgs_info.path_enable_programs).set(file, "true")?;
    } else {
        mxList::new("environment.systemPackages", true)
            .add(file, &format!("pkgs.{}", package_name))?;
    }
    Ok(())
}

pub fn uninstall_no_transaction(file: &mut NixFile, package_name: &str) -> mx::Result<()> {
    if let Some(pkgs_info) = PLUGIN_NAMESPACES.get(package_name) {
        match pkgs_info.path_enable_programs.strip_suffix(".enable") {
            Some(path) => {
                mxOption::new(path).set_option_all_instance_to_default(file)?;
            }
            None => {
                mxOption::new(pkgs_info.path_enable_programs).set(file, "false")?;
            }
        }
    } else {
        mxList::new("environment.systemPackages", true)
            .remove(file, &format!("pkgs.{}", package_name))?;
    }
    Ok(())
}

pub fn install_plugin_no_transaction(
    file: &mut NixFile,
    package_name: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    let pkgs_info = PLUGIN_NAMESPACES
        .get(package_name)
        .ok_or(mx::ErrorKind::PackageDoesNotHaveAPlugin)?;

    mxOption::new(pkgs_info.path_enable_programs).set(file, "true")?;
    mxList::new(pkgs_info.path_plugin_list, true).add(
        file,
        &format!("pkgs.{}.{}", pkgs_info.path_plugin, plugin_name),
    )?;
    Ok(())
}

pub fn remove_plugin_no_transaction(
    file: &mut NixFile,
    package_name: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    let pkgs_info = PLUGIN_NAMESPACES
        .get(package_name)
        .ok_or(mx::ErrorKind::PackageDoesNotHaveAPlugin)?;

    mxList::new(pkgs_info.path_plugin_list, true).remove(
        file,
        &format!("pkgs.{}.{}", pkgs_info.path_plugin, plugin_name),
    )?;
    Ok(())
}

const NIX_OUTPUTS: &[&str] = &["out", "dev", "lib", "doc", "man", "info", "static"];

fn parse_pkg_entry(raw: &str) -> (String, String) {
    let stripped = raw.strip_prefix("pkgs.").unwrap_or(raw);
    match stripped.rsplit_once('.') {
        Some((name, output)) if NIX_OUTPUTS.contains(&output) => {
            (name.to_string(), output.to_string())
        }
        _ => (stripped.to_string(), "out".to_string()),
    }
}

fn collect_entries(file: &NixFile) -> mx::Result<Vec<(String, String)>> {
    let pkgs = mxList::new("environment.systemPackages", true);
    let mut entries: Vec<(String, String)> = match pkgs.get_element_in_list(file) {
        Ok(e) => e.map(|n| parse_pkg_entry(n)).collect(),
        Err(mx::ErrorKind::OptionNotFound) => vec![],
        Err(e) => return Err(e),
    };
    for (pkg, pkgs_info) in PLUGIN_NAMESPACES.entries() {
        let option_pkgs = mxOption::new(pkgs_info.path_enable_programs);
        if match option_pkgs.get(file) {
            Ok(res) => res,
            Err(mx::ErrorKind::OptionNotFound) => "false",
            Err(e) => return Err(e),
        } == "true"
        {
            entries.push((pkg.to_string(), "out".to_string()));
        }
    }
    Ok(entries)
}

fn build_nix_expr(config_dir: &str, entries: &[(String, String)]) -> String {
    let nix_list = entries
        .iter()
        .map(|(name, _)| format!("\"{}\"", name))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "let pkgs = (builtins.getFlake \"{}\").inputs.nixpkgs.legacyPackages.${{builtins.currentSystem}}; in \
         builtins.listToAttrs \
           (builtins.filter (x: x != null) \
             (map (name: \
               let pkg = pkgs.${{name}} or null; in \
               if pkg == null then null \
               else {{ name = name; value = {{ \
                 pname = pkg.pname or name; \
                 description = pkg.meta.description or \"\"; \
                 version = pkg.version or \"\"; \
               }}; }}) \
             [ {} ]))",
        config_dir, nix_list
    )
}

fn eval_nix_expr(expr: &str) -> mx::Result<HashMap<String, serde_json::Value>> {
    let cmd_output = std::process::Command::new("nix")
        .args(["eval", "--impure", "--json", "--expr", expr])
        .output()
        .map_err(mx::ErrorKind::IOError)?;
    if !cmd_output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&cmd_output.stderr).to_string(),
        ));
    }
    serde_json::from_slice(&cmd_output.stdout)
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))
}

fn build_package(
    name: String,
    explicit_output: String,
    pkg_map: &HashMap<String, serde_json::Value>,
) -> NixPackage {
    let info = pkg_map.get(&name);
    NixPackage {
        pname: info
            .and_then(|v| v["pname"].as_str())
            .unwrap_or(&name)
            .to_string(),
        description: info
            .and_then(|v| v["description"].as_str())
            .unwrap_or_default()
            .to_string(),
        version: info
            .and_then(|v| v["version"].as_str())
            .unwrap_or_default()
            .to_string(),
        outputs: vec![explicit_output],
        pkg_name: name,
        #[cfg(feature = "app-info-gui")]
        flatpak: OnceCell::new(),
    }
}

pub fn list_installed_package_no_transaction(
    config_dir: &str,
    file: &NixFile,
) -> mx::Result<Vec<NixPackage>> {
    let entries = collect_entries(file)?;
    let nix_expr = build_nix_expr(config_dir, &entries);
    let pkg_map = eval_nix_expr(&nix_expr)?;
    Ok(entries
        .into_iter()
        .map(|(name, output)| build_package(name, output, &pkg_map))
        .collect())
}

pub fn install(config_dir: &str, package_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install package {}", package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_no_transaction(file, package_name),
    )
}

pub fn uninstall(config_dir: &str, package_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall {}", package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| uninstall_no_transaction(file, package_name),
    )
}

pub fn install_plugin(config_dir: &str, package_name: &str, plugin_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install {} plugin for {}", plugin_name, package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_plugin_no_transaction(file, package_name, plugin_name),
    )
}

pub fn remove_plugin(config_dir: &str, package_name: &str, plugin_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove {} plugin for {}", plugin_name, package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_plugin_no_transaction(file, package_name, plugin_name),
    )
}

pub fn list_installed_package(config_dir: &str) -> mx::Result<Vec<NixPackage>> {
    transaction::make_transaction_read_only(
        "List installed package",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| list_installed_package_no_transaction(config_dir, file),
    )
}
