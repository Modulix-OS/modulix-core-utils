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

    /// Package version string (e.g. `"115.0"`), as evaluated from nixpkgs.
    pub fn version(&self) -> &str {
        &self.version
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
        // `NIX_INFO` is keyed by the nixpkgs attribute (`pkg_name`); look that up
        // first and only fall back to the `pname` for the rare attr-not-in-table
        // case. Looking up `display_name()` alone misses every variant whose
        // attribute differs from its pname (firefox-bin, firefox-esr, …).
        package_basic_info::get_app_id(&self.pkg_name)
            .or_else(|| package_basic_info::get_app_id(self.display_name()))
    }

    fn app_name(&self) -> Option<&str> {
        package_basic_info::get_name(&self.pkg_name)
            .or_else(|| package_basic_info::get_name(self.display_name()))
    }

    fn icon(&self) -> Option<&Url> {
        package_basic_info::get_icon(&self.pkg_name)
            .or_else(|| package_basic_info::get_icon(self.display_name()))
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

/// Canonical AppStream/Flathub app-ids for which the **Flatpak** source should
/// rank above the nixpkgs package(s) when GNOME Software lists install sources
/// of the same application. A Modulix module, when one exists for the app,
/// always outranks both — this table never overrides that.
///
/// Curated on purpose: add an entry only for apps whose Flatpak build is the
/// recommended one on Modulix-OS. Keyed by the same id returned by
/// [`AppInfoGui::id`] (see [`packages_for_app_id`]).
#[cfg(feature = "app-info-gui")]
pub static FLATPAK_PREFERRED_APP_IDS: &[&str] = &[
    // "com.spotify.Client",
    // "com.discordapp.Discord",
];

/// Whether the Flatpak source is preferred over nixpkgs for the given canonical
/// app-id (see [`FLATPAK_PREFERRED_APP_IDS`]).
#[cfg(feature = "app-info-gui")]
pub fn is_flatpak_preferred(app_id: &str) -> bool {
    FLATPAK_PREFERRED_APP_IDS.contains(&app_id)
}

/// All nixpkgs attribute names that map to the given canonical app-id, i.e. the
/// nix install variants of one application (e.g. `org.mozilla.firefox` →
/// `["firefox", "firefox-esr", …]`). Used to populate the "other sources" of an
/// app for GNOME Software's `alternate-of` query.
#[cfg(feature = "app-info-gui")]
pub fn packages_for_app_id(app_id: &str) -> Vec<&'static str> {
    package_basic_info::get_packages_by_app_id(app_id)
}

/// Flatpak/AppStream display name for a nixpkgs attribute, when matched (e.g.
/// `firefox-bin` → `"Firefox"`).
#[cfg(feature = "app-info-gui")]
pub fn name_for_package(pkg_name: &str) -> Option<&'static str> {
    package_basic_info::get_name(pkg_name)
}
