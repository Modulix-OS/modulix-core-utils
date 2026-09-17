use std::collections::HashMap;
use std::path;

#[cfg(feature = "app-info-gui")]
use tokio::sync::OnceCell;

use crate::core::transaction::transaction::UpdateInput;
use crate::{
    core::{
        list::List as mxList,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
    package_info::NixPackage,
};

const FILE_PACKAGE_PATH: &str = "package.nix";

pub fn install_no_transaction(file: &mut NixFile, packages: &[&str]) -> mx::Result<()> {
    let list = mxList::new("environment.systemPackages", true);
    for package_name in packages {
        list.add(file, &format!("pkgs.{}", package_name))?;
    }
    Ok(())
}

pub fn uninstall_no_transaction(file: &mut NixFile, packages: &[&str]) -> mx::Result<()> {
    let list = mxList::new("environment.systemPackages", true);
    for package_name in packages {
        list.remove(file, &format!("pkgs.{}", package_name))?;
    }
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
    let entries: Vec<(String, String)> = match pkgs.get_element_in_list(file) {
        Ok(e) => e.map(|n| parse_pkg_entry(n)).collect(),
        Err(mx::ErrorKind::OptionNotFound) => vec![],
        Err(e) => return Err(e),
    };
    Ok(entries)
}

/// Where `build_nix_expr` looks for nixpkgs in the configuration flake.
///
/// A Modulix configuration consumes nixpkgs *through* `mxpkgs` — its own
/// inputs are `mxpkgs` and `nixos-hardware` only — so the direct
/// `inputs.nixpkgs` this used to assume fails outright there with
/// `attribute 'nixpkgs' missing`, taking the whole installed listing with it.
/// The direct input is still tried first so a configuration that does expose
/// nixpkgs keeps working.
///
/// Deliberately not `nixosConfigurations.<name>.pkgs`, which would be more
/// faithful (overlays included) but evaluates the entire system configuration
/// on every listing; `legacyPackages` is a cheap lookup.
const NIXPKGS_LOOKUP: &str = "flake.inputs.nixpkgs \
     or flake.inputs.mxpkgs.inputs.nixpkgs \
     or (throw \"no nixpkgs input in the Modulix configuration flake\")";

fn build_nix_expr(config_dir: &str, entries: &[(String, String)]) -> String {
    let nix_list = entries
        .iter()
        .map(|(name, _)| format!("\"{}\"", name))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "let \
           flake = builtins.getFlake \"{}\"; \
           nixpkgs = {}; \
           pkgs = nixpkgs.legacyPackages.${{builtins.currentSystem}}; in \
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
        config_dir, NIXPKGS_LOOKUP, nix_list
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

pub fn install(config_dir: &str, packages: &[&str]) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install packages {}", packages.join(", ")),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_no_transaction(file, packages),
    )
}

pub fn uninstall(config_dir: &str, packages: &[&str]) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall {}", packages.join(", ")),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| uninstall_no_transaction(file, packages),
    )
}

pub fn list_installed_package(config_dir: &str) -> mx::Result<Vec<NixPackage>> {
    // No `package.nix` → nothing was ever installed. See
    // [`list_installed_package_names`] for why the check lives here.
    if !path::Path::new(&format!("{config_dir}{FILE_PACKAGE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    transaction::make_transaction_read_only(
        "List installed package",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| list_installed_package_no_transaction(config_dir, file),
    )
}

/// nixpkgs attributes listed in `environment.systemPackages`, without the
/// `nix eval` [`list_installed_package`] pays to resolve pname/version/
/// description. Parsing `package.nix` is all it takes to answer "is this
/// installed?", which callers ask on hot paths (every store search).
///
/// A configuration where nothing has ever been installed has no `package.nix`
/// at all: that is "no package installed", not an error (a read-only
/// transaction does not create the file, it returns `FileNotFound`). The check
/// is done here rather than by catching `FileNotFound` from the transaction,
/// which also opens `configuration.nix` and would make a genuinely broken
/// configuration look like an empty one.
pub fn list_installed_package_names(config_dir: &str) -> mx::Result<Vec<String>> {
    // Same concatenation as `NixFile::new`, which the transaction uses.
    if !path::Path::new(&format!("{config_dir}{FILE_PACKAGE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    transaction::make_transaction_read_only(
        "List installed package names",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| {
            Ok(collect_entries(file)?
                .into_iter()
                .map(|(name, _)| name)
                .collect())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_nix_expr_resolves_nixpkgs_through_mxpkgs() {
        let expr = build_nix_expr("/etc/modulix-os/", &[("htop".into(), "out".into())]);
        // The direct input stays first; the `mxpkgs` hop is what a real
        // Modulix configuration actually needs (see `NIXPKGS_LOOKUP`).
        assert!(expr.contains("flake.inputs.nixpkgs"));
        assert!(expr.contains("flake.inputs.mxpkgs.inputs.nixpkgs"));
        assert!(expr.contains("builtins.getFlake \"/etc/modulix-os/\""));
        assert!(expr.contains("[ \"htop\" ]"));
    }
}
