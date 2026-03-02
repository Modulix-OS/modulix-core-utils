use phf::phf_map;
use std::process;

use serde::Deserialize;

use crate::{
    core::{
        list::List as mxList,
        transaction::{Transaction, transaction::BuildCommand},
    },
    mx,
};

#[derive(Debug)]
pub struct NixPlugin {
    pub name: String,
    pub full_name: String, // namespace.name
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct NixPackage {
    name: String,
    description: String,
}

static PLUGIN_NAMESPACES: phf::Map<&'static str, &'static [&'static str]> = phf_map! {
    // Éditeurs
    "vim"                   => &["vimPlugins"],
    "neovim"                => &["vimPlugins"],
    "emacs"                 => &["emacsPackages"],
    "kakoune"               => &["kakounePlugins"],
    "vscode"                => &["vscode-extensions"],
    "vscodium"              => &["vscode-extensions"],

    // Audio / Vidéo
    "obs-studio"            => &["obs-studio-plugins"],
    "deadbeef"              => &["deadbeefPlugins"],
    "kodi"                  => &["kodiPackages"],
    "mpv"                   => &["mpvScripts"],
    "gst_all_1.gstreamer"   => &["gst_all_1"],

    // Graphisme
    "gimp"                  => &["gimpPlugins"],
    "gimp2"                 => &["gimp2Plugins"],
    "inkscape"              => &["inkscape-extensions"],

    // Shell
    "zsh"                   => &["zshPlugins"],
    "fish"                  => &["fishPlugins"],

    // Langages - Python
    "python313"             => &["python313Packages"],
    "python312"             => &["python312Packages"],
    "python311"             => &["python311Packages"],
    "python310"             => &["python310Packages"],

    // Langages - PHP
    "php82"                 => &["php82Packages", "php82Extensions"],
    "php83"                 => &["php83Packages", "php83Extensions"],
    "php84"                 => &["php84Packages", "php84Extensions"],
    "php85"                 => &["php85Packages", "php85Extensions"],

    // Langages - Lua
    "lua51"                 => &["lua51Packages"],
    "lua52"                 => &["lua52Packages"],
    "lua53"                 => &["lua53Packages"],
    "lua54"                 => &["lua54Packages"],

    // Langages - Perl
    "perl538"               => &["perl538Packages"],

    // Langages - Autres
    "ruby"                  => &["rubyPackages"],
    "ocaml"                 => &["ocamlPackages"],
    "sbcl"                  => &["sbclPackages"],
    "haskell"               => &["haskellPackages"],
    "R"                     => &["rPackages"],

    // LaTeX
    "texlive"               => &["texlivePackages"],
    "texliveFull"           => &["texlivePackages"],
    "texliveSmall"          => &["texlivePackages"],
    "texliveBasic"          => &["texlivePackages"],
    "texliveMedium"         => &["texlivePackages"],
    "texliveMinimal"        => &["texlivePackages"],

    // Outils de dev
    "terraform"             => &["terraform-providers"],
    "buildbot"              => &["buildbot-plugins"],

    // Bureau
    "pantheon"              => &["pantheon"],

    // GPU / HPC
    "rocm"                  => &["rocmPackages"],

    // Musique
    "open-music-kontroller" => &["open-music-kontroller"],

    // Réseau
    "emilua"                => &["emiluaPlugins"],
};

pub fn install(package_name: &str) -> mx::Result<()> {
    let mut transac_add_pkgs =
        Transaction::new(&format!("Install {}", package_name), BuildCommand::Switch)?;
    transac_add_pkgs.add_file("package.nix")?;

    transac_add_pkgs.begin()?;

    let file = match transac_add_pkgs.get_file("package.nix") {
        Ok(f) => f,
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    let pkgs = mxList::new("environment.systemPackages", true);
    match pkgs.add(file, &format!("pkgs.{}", package_name)) {
        Ok(()) => (),
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };
    Ok(())
}

pub fn uninstall(package_name: &str) -> mx::Result<()> {
    let mut transac_add_pkgs =
        Transaction::new(&format!("Uninstall {}", package_name), BuildCommand::Switch)?;
    transac_add_pkgs.add_file("package.nix")?;

    transac_add_pkgs.begin()?;

    let file = match transac_add_pkgs.get_file("package.nix") {
        Ok(f) => f,
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    let pkgs = mxList::new("environment.systemPackages", true);
    match pkgs.remove(file, &format!("pkgs.{}", package_name)) {
        Ok(()) => (),
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };
    Ok(())
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

    // Correspondance exacte
    if name_lower == query_lower {
        score += 1000;
    }

    // Position du match dans le nom (début > milieu > fin)
    if let Some(pos) = name_lower.find(&query_lower) {
        score += match pos {
            0 => 500,
            1..=3 => 300,
            _ => 100,
        };
    }

    // Position du match dans la description (pondérée moins)
    if let Some(pos) = desc_lower.find(&query_lower) {
        score += match pos {
            0 => 50,
            1..=10 => 30,
            _ => 10,
        };
    }

    // Recherche floue sur le nom (distance de Levenshtein)
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

pub fn search_packages(query: &str, arch: &str) -> mx::Result<Vec<NixPackage>> {
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

    let prefix = format!("legacyPackages.{}.", arch);

    // Collecter tous les namespaces de plugins
    let plugin_namespaces: std::collections::HashSet<&str> = PLUGIN_NAMESPACES
        .values()
        .flat_map(|v| v.iter().copied())
        .collect();

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

pub fn list_plugins(package: &str, arch: &str) -> mx::Result<Vec<NixPlugin>> {
    let namespaces = PLUGIN_NAMESPACES.get(package).ok_or_else(|| {
        mx::ErrorKind::NixCommandError(format!(
            "No plugin namespace found for package '{}'",
            package
        ))
    })?;

    let mut all_plugins = Vec::new();

    for namespace in *namespaces {
        let expr = format!("nixpkgs#legacyPackages.{}.{}", arch, namespace);
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
        let raw: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&stdout)
            .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

        for (name, value) in raw {
            let description = value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();

            all_plugins.push(NixPlugin {
                name: name.clone(),
                full_name: format!("{}.{}", namespace, name),
                description,
            });
        }
    }

    Ok(all_plugins)
}
