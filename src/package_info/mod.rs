#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;
use std::{collections::HashMap, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::core::app_info_trait::AppInfoMinimal;
use crate::core::app_info_trait::PLUGIN_NAMESPACE_PREFIXES;
use crate::core::app_info_trait::score;
use crate::mx;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::AppInfoGui;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::AppScreenshot;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::FlatpakInfo;

#[cfg(feature = "app-info-gui")]
mod package_basic_info;

#[cfg(feature = "app-info-gui")]
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

    #[cfg(feature = "app-info-gui")]
    #[serde(skip)]
    pub(crate) flatpak: OnceCell<Option<FlatpakInfo>>,
}

impl NixPackage {
    #[cfg(feature = "app-info-gui")]
    async fn get_flatpak(&self) -> Option<&FlatpakInfo> {
        self.flatpak
            .get_or_init(|| async { FlatpakInfo::new(self.id()?).await.ok() })
            .await
            .as_ref()
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
}

impl AppInfoMinimal for NixPackage {
    async fn new(pkg_name: &str) -> mx::Result<Self> {
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

    async fn search(query: &str, number_app: u32) -> mx::Result<Vec<Self>> {
        let output = tokio::process::Command::new("nix")
            .args(["search", "nixpkgs", "--json", query])
            .env("NIXPKGS_ALLOW_UNFREE", "1")
            .output()
            .await
            .map_err(mx::ErrorKind::IOError)?;

        let raw: HashMap<String, NixPackage> =
            serde_json::from_slice(&output.stdout).map_err(mx::ErrorKind::ParseError)?;

        let prefix = format!("legacyPackages.{}.", env!("TARGET_NIX"));

        let mut packages: Vec<(u32, Self)> = raw
            .into_iter()
            .filter_map(|(key, value)| {
                let name = key.strip_prefix(&prefix).unwrap_or(&key);
                // Plugin attribute sets get a dedicated module, never a search hit.
                if PLUGIN_NAMESPACE_PREFIXES
                    .iter()
                    .any(|ns| name.starts_with(ns))
                {
                    return None;
                }
                #[cfg(feature = "app-info-gui")]
                let keywords: Vec<&str> = package_basic_info::get_keywords(name)
                    .map(<[&str]>::to_vec)
                    .unwrap_or_default();
                #[cfg(not(feature = "app-info-gui"))]
                let keywords: Vec<&str> = Vec::new();
                let relevance = score(name, &value.description, &keywords, query);
                Some((
                    relevance,
                    Self {
                        pkg_name: name.to_string(),
                        version: value.version,
                        description: value.description,
                        pname: value.pname,
                        outputs: vec![],
                        #[cfg(feature = "app-info-gui")]
                        flatpak: OnceCell::new(),
                    },
                ))
            })
            .collect();

        packages.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        packages.truncate(number_app as usize);
        Ok(packages.into_iter().map(|(_, pkg)| pkg).collect())
    }

    fn package_name(&self) -> &str {
        &self.pkg_name
    }

    fn display_name(&self) -> &str {
        &self.pname
    }

    fn summary(&self) -> &str {
        &self.description
    }
}

#[cfg(feature = "app-info-gui")]
impl AppInfoGui for NixPackage {
    fn id(&self) -> Option<&str> {
        package_basic_info::get_app_id(self.display_name())
    }

    fn icon(&self) -> Option<&Url> {
        package_basic_info::get_icon(self.display_name())
    }

    fn keyword(&self) -> Vec<&str> {
        package_basic_info::get_keywords(&self.pkg_name)
            .map(<[&str]>::to_vec)
            .unwrap_or_default()
    }

    async fn description(&self) -> Cow<'_, str> {
        if let Some(flatpak) = self.get_flatpak().await {
            Cow::Borrowed(flatpak.description())
        } else {
            Cow::Borrowed(self.summary())
        }
    }

    async fn screenshots<'a>(&'a self) -> Option<AppScreenshot<'a>> {
        self.get_flatpak().await?.screenshots()
    }

    async fn main_program(&self) -> mx::Result<Cow<'_, str>> {
        let expr = format!("nixpkgs#{}.meta.mainProgram", self.pkg_name);
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
        let main_program: String = serde_json::from_str(&stdout).map_err(|_| {
            mx::ErrorKind::NixCommandError(String::from("Impossible to parse mainProgram"))
        })?;
        Ok(Cow::Owned(main_program))
    }
}
