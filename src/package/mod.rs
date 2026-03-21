use phf::phf_map;
use std::process;
pub mod desktop_icon;

use serde::{Deserialize, Serialize};

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

#[derive(Debug, Deserialize, Serialize)]
pub struct NixPlugin {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NixPackage {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct License {
    pub full_name: Option<String>,
    pub spdx_id: Option<String>,
    pub url: Option<String>,
    pub free: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Maintainer {
    pub name: Option<String>,
    pub email: Option<String>,
    pub github: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadata {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub long_description: Option<String>,
    pub homepage: Option<OneOrMany<String>>,
    pub license: Option<OneOrMany<License>>,
    pub maintainers: Option<OneOrMany<Maintainer>>,
    pub platforms: Option<OneOrMany<String>>,
    pub broken: Option<bool>,
    pub unfree: Option<bool>,
    pub position: Option<String>,
}

const FILE_PACKAGE_PATH: &str = "package.nix";

struct PluginNamespace {
    pub path_plugin: &'static str,
    pub path_enable_programs: &'static str,
    pub path_plugin_list: &'static str,
}

impl PluginNamespace {
    pub const fn new(
        path_plugin: &'static str,
        path_enable_programs: &'static str,
        path_plugin_list: &'static str,
    ) -> Self {
        Self {
            path_plugin,
            path_enable_programs,
            path_plugin_list,
        }
    }
}

static PLUGIN_NAMESPACES: phf::Map<&'static str, PluginNamespace> = phf_map! {
    "vscode" => PluginNamespace::new(
        "vscode-extensions",
        "programs.vscode.enable",
        "programs.vscode.extensions"),
    "obs-studio" => PluginNamespace::new(
        "obs-studio-plugins",
        "programs.obs-studio.enable",
        "programs.obs-studio.plugins"),
};

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

pub fn list_installed_package_no_transaction(
    config_dir: &str,
    file: &mut NixFile,
) -> mx::Result<Vec<NixPackage>> {
    let pkgs = mxList::new("environment.systemPackages", true);
    let mut names: Vec<&str> = match pkgs.get_element_in_list(file) {
        Ok(e) => e.map(|n| n.strip_prefix("pkgs.").unwrap_or(n)).collect(),
        Err(mx::ErrorKind::OptionNotFound) => vec![],
        Err(e) => return Err(e),
    };

    for (pkgs, pkgs_info) in PLUGIN_NAMESPACES.entries() {
        let option_pkgs = mxOption::new(pkgs_info.path_enable_programs);
        if match option_pkgs.get(file) {
            Ok(res) => res,
            Err(mx::ErrorKind::OptionNotFound) => "false",
            Err(e) => return Err(e),
        } == "true"
        {
            names.push(pkgs);
        }
    }

    let nix_list = names
        .iter()
        .map(|n| format!("\"{}\"", n))
        .collect::<Vec<_>>()
        .join(" ");

    let nix_expr = format!(
        "let pkgs = (builtins.getFlake \"{}\").inputs.nixpkgs.legacyPackages.${{builtins.currentSystem}}; in \
         builtins.listToAttrs \
           (builtins.filter (x: x != null) \
             (map (name: \
               let pkg = pkgs.${{name}} or null; in \
               if pkg == null then null \
               else {{ name = name; value = pkg.meta.description or \"\"; }}) \
             [ {} ]))",
        config_dir, nix_list
    );

    let output = std::process::Command::new("nix")
        .args(["eval", "--impure", "--json", "--expr", &nix_expr])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let descriptions: std::collections::HashMap<String, String> =
        serde_json::from_str(&json_str)
            .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let packages = names
        .clone()
        .into_iter()
        .zip(names.into_iter())
        .map(|(original_name, clean_name)| {
            let description = descriptions.get(clean_name).cloned().unwrap_or_default();
            NixPackage {
                name: original_name.to_string(),
                description,
            }
        })
        .collect();

    Ok(packages)
}

pub fn install(config_dir: &str, package_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install {}", package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        |file| install_no_transaction(file, package_name),
    )
}

pub fn uninstall(config_dir: &str, package_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall {}", package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        |file| uninstall_no_transaction(file, package_name),
    )
}

pub fn install_plugin(config_dir: &str, package_name: &str, plugin_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install {} plugin for {}", plugin_name, package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        |file| install_plugin_no_transaction(file, package_name, plugin_name),
    )
}

pub fn remove_plugin(config_dir: &str, package_name: &str, plugin_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove {} plugin for {}", plugin_name, package_name),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        |file| remove_plugin_no_transaction(file, package_name, plugin_name),
    )
}

pub fn list_installed_package(config_dir: &str) -> mx::Result<Vec<NixPackage>> {
    transaction::make_transaction(
        "List installed package",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| list_installed_package_no_transaction(config_dir, file),
    )
}

pub fn get_package_outputs(package: &str) -> mx::Result<Vec<String>> {
    let expr = format!("nixpkgs#{}.outputs", package);

    let output = process::Command::new("nix")
        .args(["eval", "--json", &expr])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
    let outputs: Vec<String> = serde_json::from_str(&stdout).map_err(|_| {
        mx::ErrorKind::NixCommandError(String::from("Impossible to grep output format"))
    })?;

    Ok(outputs)
}

fn score_package(name: &str, description: &str, query: &str) -> u32 {
    let query_lower = query.to_lowercase();
    let name_lower = name.to_lowercase();
    let desc_lower = description.to_lowercase();
    let mut score = 0u32;

    if name_lower == query_lower {
        score += 1000;
    }
    if let Some(pos) = name_lower.find(&query_lower) {
        score += match pos {
            0 => 500,
            1..=3 => 300,
            _ => 100,
        };
    }
    if let Some(pos) = desc_lower.find(&query_lower) {
        score += match pos {
            0 => 50,
            1..=10 => 30,
            _ => 10,
        };
    }
    let dist = levenshtein(name_lower.as_str(), query_lower.as_str());
    score += match dist {
        0 => 200,
        1 => 100,
        2 => 50,
        3 => 20,
        _ => 0,
    };

    score
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1])
            };
        }
    }
    dp[m][n]
}

pub fn search_packages(query: &str) -> mx::Result<Vec<NixPackage>> {
    let output = process::Command::new("nix")
        .args(["search", "nixpkgs", "--json", query])
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
    let raw: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&stdout).map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let prefix = format!("legacyPackages.{}.", env!("TARGET_NIX"));
    let plugin_namespaces: std::collections::HashSet<&str> =
        PLUGIN_NAMESPACES.values().map(|v| v.path_plugin).collect();

    let mut packages: Vec<(u32, NixPackage)> = raw
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(key, _)| key.trim_start_matches(&prefix).to_string())
        .filter(|name| {
            if PLUGIN_NAMESPACES.contains_key(name.as_str()) {
                return true;
            }
            !plugin_namespaces.iter().any(|ns| name.starts_with(ns))
        })
        .map(|name| {
            let value = &raw[&format!("{}{}", prefix, name)];
            let description = value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let score = score_package(&name, &description, query);
            (score, NixPackage { name, description })
        })
        .collect();

    packages.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(packages.into_iter().map(|(_, pkg)| pkg).collect())
}

pub fn list_plugins(package: &str) -> mx::Result<Vec<NixPlugin>> {
    let namespace = PLUGIN_NAMESPACES
        .get(package)
        .ok_or_else(|| {
            mx::ErrorKind::NixCommandError(format!(
                "No plugin namespace found for package '{}'",
                package
            ))
        })?
        .path_plugin;

    let expr = format!(
        "nixpkgs#legacyPackages.{}.{}",
        env!("TARGET_NIX"),
        namespace
    );
    let output = process::Command::new("nix")
        .args([
            "eval",
            "--json",
            &expr,
            "--apply",
            "attrs: builtins.mapAttrs
                (
                    name: pkg:
                    let tried = builtins.tryEval
                        (pkg.meta.description or \"\");
                    in {
                        description = if tried.success then
                                        tried.value
                                        else \"\";
                    }
                ) attrs",
        ])
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
    let raw: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&stdout).map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    Ok(raw
        .into_iter()
        .map(|(name, value)| {
            let description = value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            NixPlugin { name, description }
        })
        .collect())
}

pub fn get_package_metadata(package_name: &str) -> mx::Result<PackageMetadata> {
    let expr = format!(
        r#"
        let
          pkgs = (builtins.getFlake "nixpkgs").legacyPackages.{arch};
          pkg = pkgs.{package_name};
          meta = pkg.meta or {{}};
        in {{
          name = pkg.name or null;
          version = pkg.version or null;
          description = meta.description or null;
          longDescription = meta.longDescription or null;
          homepage = meta.homepage or null;
          license = meta.license or null;
          maintainers = map (m: {{
            name = m.name or null;
            email = m.email or null;
            github = m.github or null;
          }}) (meta.maintainers or []);
          platforms = meta.platforms or null;
          broken = meta.broken or null;
          unfree = meta.unfree or null;
          position = meta.position or null;
        }}
        "#,
        arch = env!("TARGET_NIX")
    );

    let output = process::Command::new("nix")
        .args(["eval", "--json", "--impure", "--expr", &expr])
        .output()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&json_str).map_err(mx::ErrorKind::ParseError)
}
