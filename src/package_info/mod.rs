use std::{collections::HashMap, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::mx;

#[cfg(feature = "package-info-full")]
mod flatpak;
#[cfg(feature = "package-info-full")]
use flatpak::FlatpakInfo;
#[cfg(feature = "package-info-full")]
mod package_basic_info;

#[cfg(feature = "package-info-full")]
mod screenshot;
use crate::core::plugin_namespace::PLUGIN_NAMESPACES;
#[cfg(feature = "package-info-full")]
pub use screenshot::AppSreenshot;

#[cfg(feature = "package-info-full")]
use tokio::sync::OnceCell;

type Url = str;

#[derive(Debug, Serialize, Deserialize)]
pub struct NixPackage {
    #[serde(skip)]
    pub(crate) pkg_name: String,

    pub(crate) description: String,
    pub(crate) pname: String,
    pub(crate) version: String,

    #[serde(default)]
    pub(crate) outputs: Vec<String>,

    #[cfg(feature = "package-info-full")]
    #[serde(skip)]
    pub(crate) flatpak: OnceCell<Option<FlatpakInfo>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NixPlugin {
    pub name: String,
    pub description: String,
}

impl NixPackage {
    #[cfg(feature = "package-info-full")]
    async fn get_flatpak(&self) -> Option<&FlatpakInfo> {
        self.flatpak
            .get_or_init(|| async { FlatpakInfo::new(self.id()?).await.ok() })
            .await
            .as_ref()
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
        let dist = Self::levenshtein(name_lower.as_str(), query_lower.as_str());
        score += match dist {
            0 => 200,
            1 => 100,
            2 => 50,
            3 => 20,
            _ => 0,
        };

        #[cfg(feature = "package-info-full")]
        if let Some(keywords) = package_basic_info::get_keywords(name) {
            for keyword in keywords {
                let keyword_lower = keyword.to_lowercase();
                if keyword_lower == query_lower {
                    score += 400;
                } else if keyword_lower.contains(&query_lower) {
                    score += 150;
                } else {
                    let dist = Self::levenshtein(&keyword_lower, &query_lower);
                    score += match dist {
                        1 => 80,
                        2 => 30,
                        _ => 0,
                    };
                }
            }
        }

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

    pub async fn new(pkg_name: &str) -> crate::mx::Result<Self> {
        let expr = format!(
            r#"let p = (import <nixpkgs> {{}}).{}; in {{ name = p.meta.name or p.name; version = p.version; description = p.meta.description or ""; outputs = p.outputs or []; }}"#,
            pkg_name
        );

        let output = tokio::process::Command::new("nix")
            .args(["eval", "--json", "--expr", &expr])
            .output()
            .await
            .map_err(mx::ErrorKind::IOError)?;

        let mut info: NixPackage =
            serde_json::from_slice(&output.stdout).map_err(mx::ErrorKind::ParseError)?;
        info.pkg_name = pkg_name.to_string();
        Ok(info)
    }

    pub async fn search(query: &str, number_app: u32) -> mx::Result<Vec<Self>> {
        let output = tokio::process::Command::new("nix")
            .args(["search", "nixpkgs", "--json", query])
            .env("NIXPKGS_ALLOW_UNFREE", "1")
            .output()
            .await
            .map_err(mx::ErrorKind::IOError)?;

        let raw: HashMap<String, NixPackage> =
            serde_json::from_slice(&output.stdout).map_err(mx::ErrorKind::ParseError)?;

        let plugin_namespaces: std::collections::HashSet<&str> =
            PLUGIN_NAMESPACES.values().map(|v| v.path_plugin).collect();

        let prefix = format!("legacyPackages.{}.", env!("TARGET_NIX"));

        let mut packages: Vec<(u32, Self)> = raw
            .into_iter()
            .filter_map(|(key, value)| {
                let name = key.strip_prefix(&prefix).unwrap_or(&key);
                if !PLUGIN_NAMESPACES.contains_key(name)
                    && plugin_namespaces.iter().any(|ns| name.starts_with(ns))
                {
                    return None;
                }
                let score = Self::score_package(name, &value.description, query);
                Some((
                    score,
                    Self {
                        pkg_name: name.to_string(),
                        version: value.version,
                        description: value.description,
                        pname: value.pname,
                        outputs: vec![],
                        #[cfg(feature = "package-info-full")]
                        flatpak: OnceCell::new(),
                    },
                ))
            })
            .collect();

        packages.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        packages.truncate(number_app as usize);
        Ok(packages.into_iter().map(|(_, pkg)| pkg).collect())
    }

    pub fn package_name(&self) -> &str {
        &self.pkg_name
    }

    pub fn name(&self) -> &str {
        &self.pname
    }

    pub fn summary(&self) -> &str {
        &self.description
    }

    #[cfg(feature = "package-info-full")]
    pub fn id(&self) -> Option<&str> {
        package_basic_info::get_app_id(self.name())
    }

    #[cfg(feature = "package-info-full")]
    pub fn icon(&self) -> Option<&Url> {
        package_basic_info::get_icon(self.name())
    }

    #[cfg(feature = "package-info-full")]
    pub fn keyword(&self) -> Option<&[&str]> {
        package_basic_info::get_keywords(&self.pkg_name)
    }

    #[cfg(feature = "package-info-full")]
    pub async fn description(&self) -> &str {
        if let Some(flatpak) = self.get_flatpak().await {
            flatpak.description()
        } else {
            self.summary()
        }
    }

    #[cfg(feature = "package-info-full")]
    pub async fn screenshots<'a>(&'a self) -> Option<AppSreenshot<'a>> {
        self.get_flatpak().await?.screenshots()
    }

    pub async fn get_outputs(&self) -> mx::Result<Vec<String>> {
        let expr = format!("nixpkgs#{}.outputs", self.pkg_name);

        let output = tokio::process::Command::new("nix")
            .args(["eval", "--json", &expr])
            .output()
            .await
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

    pub async fn list_plugins(package: &str) -> mx::Result<Vec<NixPlugin>> {
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
        let output = tokio::process::Command::new("nix")
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
            .await
            .map_err(mx::ErrorKind::IOError)?;

        if !output.status.success() {
            return Err(mx::ErrorKind::NixCommandError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
        let raw: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&stdout)
            .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

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
}
