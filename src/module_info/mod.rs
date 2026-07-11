use std::collections::HashMap;

use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::REMOTE_MODULE_URL;
use crate::core::app_info_trait::{AppInfoMinimal, AppPlugin, score};
use crate::core::nix_eval;
use crate::mx;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::{AppInfoGui, AppScreenshot, FlatpakInfo};
#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;

#[cfg(feature = "app-info-gui")]
type Url = str;

/// One `modules/index.json` entry: the routing + lightweight search metadata of
/// a module. Rich GUI info (icon, description, screenshots) is resolved lazily
/// from Flathub (`flathub_id`) or from the module's `metadata.json`.
#[derive(Debug, Deserialize)]
struct IndexEntry {
    path: String,
    #[serde(default)]
    nix_info: Option<String>,
    #[serde(default)]
    flathub_id: Option<String>,
    #[serde(default)]
    plugins: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    keyword: Option<Vec<String>>,
}

/// `metadata.json` of a custom module (one without a `flathub_id`). Its name and
/// summary already live inline in `index.json`; only the rich GUI fields below
/// are read from here.
#[cfg(feature = "app-info-gui")]
#[derive(Debug, Deserialize)]
struct ModuleMetadata {
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    keyword: Option<Vec<String>>,
}

/// A Modulix module: a NixOS configuration fragment enabled through
/// `mx.<name>.enable`. Consumed like a [`crate::package_info::NixPackage`] via
/// the [`AppInfoMinimal`] / [`AppInfoGui`] traits.
pub struct ModuleInfo {
    name: String,
    path: String,
    nix_info: Option<String>,
    flathub_id: Option<String>,
    inline_name: Option<String>,
    inline_summary: Option<String>,
    #[cfg(feature = "app-info-gui")]
    inline_keyword: Option<Vec<String>>,
    plugins: Option<String>,
    #[cfg(feature = "app-info-gui")]
    flatpak: OnceCell<Option<FlatpakInfo>>,
    #[cfg(feature = "app-info-gui")]
    metadata: OnceCell<Option<ModuleMetadata>>,
}

/// Process-wide cache of `modules/index.json` (fetched once).
static INDEX: OnceCell<HashMap<String, IndexEntry>> = OnceCell::const_new();

async fn fetch_index() -> mx::Result<&'static HashMap<String, IndexEntry>> {
    INDEX
        .get_or_try_init(|| async {
            reqwest::get(format!("{REMOTE_MODULE_URL}index.json"))
                .await
                .map_err(mx::ErrorKind::HttpError)?
                .error_for_status()
                .map_err(mx::ErrorKind::HttpError)?
                .json::<HashMap<String, IndexEntry>>()
                .await
                .map_err(mx::ErrorKind::HttpError)
        })
        .await
}

#[cfg(feature = "app-info-gui")]
async fn fetch_metadata(path: &str) -> mx::Result<ModuleMetadata> {
    reqwest::get(format!("{REMOTE_MODULE_URL}{path}/metadata.json"))
        .await
        .map_err(mx::ErrorKind::HttpError)?
        .error_for_status()
        .map_err(mx::ErrorKind::HttpError)?
        .json::<ModuleMetadata>()
        .await
        .map_err(mx::ErrorKind::HttpError)
}

/// List the plugin packages exposed by a nixpkgs namespace (e.g.
/// `obs-studio-plugins`) with their descriptions.
async fn list_plugins_in_namespace(namespace: &str) -> mx::Result<Vec<AppPlugin>> {
    let expr = format!(
        "nixpkgs#legacyPackages.{}.{}",
        env!("TARGET_NIX"),
        namespace
    );
    let raw: serde_json::Map<String, serde_json::Value> = nix_eval::eval_json(&[
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
    .await?;

    Ok(raw
        .into_iter()
        .map(|(name, value)| {
            let description = value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            AppPlugin { name, description }
        })
        .collect())
}

impl ModuleInfo {
    fn from_entry(name: &str, entry: &IndexEntry) -> Self {
        Self {
            name: name.to_string(),
            path: entry.path.clone(),
            nix_info: entry.nix_info.clone(),
            flathub_id: entry.flathub_id.clone(),
            inline_name: entry.name.clone(),
            inline_summary: entry.summary.clone(),
            #[cfg(feature = "app-info-gui")]
            inline_keyword: entry.keyword.clone(),
            plugins: entry.plugins.clone(),
            #[cfg(feature = "app-info-gui")]
            flatpak: OnceCell::new(),
            #[cfg(feature = "app-info-gui")]
            metadata: OnceCell::new(),
        }
    }

    /// Relative directory of the module under `modules/`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Flathub application id backing this module's GUI info, if any.
    pub fn flathub_id(&self) -> Option<&str> {
        self.flathub_id.as_deref()
    }

    /// nixpkgs attribute used for nix-side metadata (`mainProgram`…), if any.
    pub fn nix_info(&self) -> Option<&str> {
        self.nix_info.as_deref()
    }

    /// nixpkgs namespace the module pulls its plugins from, if any.
    pub fn plugins_namespace(&self) -> Option<&str> {
        self.plugins.as_deref()
    }

    /// Plugins available for this module, from its registered nixpkgs namespace.
    pub async fn list_plugins(&self) -> mx::Result<Vec<AppPlugin>> {
        let namespace = self
            .plugins
            .as_deref()
            .ok_or(mx::ErrorKind::PackageDoesNotHaveAPlugin)?;
        list_plugins_in_namespace(namespace).await
    }

    #[cfg(feature = "app-info-gui")]
    async fn get_flatpak(&self) -> Option<&FlatpakInfo> {
        let id = self.flathub_id.as_deref()?;
        self.flatpak
            .get_or_init(|| async { FlatpakInfo::new(id).await.ok() })
            .await
            .as_ref()
    }

    #[cfg(feature = "app-info-gui")]
    async fn get_metadata(&self) -> Option<&ModuleMetadata> {
        self.metadata
            .get_or_init(|| async { fetch_metadata(&self.path).await.ok() })
            .await
            .as_ref()
    }

    /// Resolve the remote GUI source (Flathub or `metadata.json`) so the sync
    /// [`AppInfoGui::icon`] / [`AppInfoGui::keyword`] accessors return data.
    /// `search` results are not resolved eagerly; call this on a selected item.
    #[cfg(feature = "app-info-gui")]
    pub async fn resolve(&self) {
        if self.flathub_id.is_some() {
            self.get_flatpak().await;
        } else {
            self.get_metadata().await;
        }
    }
}

impl AppInfoMinimal for ModuleInfo {
    async fn new(name: &str) -> mx::Result<Self> {
        let index = fetch_index().await?;
        let entry = index.get(name).ok_or(mx::ErrorKind::PackageNotFound)?;
        Ok(Self::from_entry(name, entry))
    }

    async fn search(query: &str, number_app: u32) -> mx::Result<Vec<Self>> {
        let index = fetch_index().await?;
        let mut modules: Vec<(u32, Self)> = index
            .iter()
            .map(|(name, entry)| {
                let keywords: Vec<&str> = entry
                    .keyword
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(String::as_str)
                    .collect();
                let summary = entry.summary.as_deref().unwrap_or("");
                let display = entry.name.as_deref().unwrap_or(name);
                let relevance = score(name, summary, &keywords, query)
                    .max(score(display, summary, &keywords, query));
                (relevance, Self::from_entry(name, entry))
            })
            .collect();
        modules.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        modules.truncate(number_app as usize);
        Ok(modules.into_iter().map(|(_, m)| m).collect())
    }

    fn package_name(&self) -> &str {
        &self.name
    }

    fn display_name(&self) -> &str {
        self.inline_name.as_deref().unwrap_or(&self.name)
    }

    fn summary(&self) -> &str {
        self.inline_summary.as_deref().unwrap_or("")
    }
}

#[cfg(feature = "app-info-gui")]
impl AppInfoGui for ModuleInfo {
    fn id(&self) -> Option<&str> {
        self.flathub_id.as_deref()
    }

    /// Modules keep their own display name (the plugin never appends a Flatpak
    /// title for them), so this is always `None`.
    fn app_name(&self) -> Option<&str> {
        None
    }

    fn icon(&self) -> Option<&Url> {
        if let Some(Some(flatpak)) = self.flatpak.get() {
            return Some(flatpak.icon());
        }
        if let Some(Some(metadata)) = self.metadata.get() {
            return metadata.icon.as_deref();
        }
        None
    }

    fn keyword(&self) -> Vec<&str> {
        if let Some(keyword) = &self.inline_keyword {
            return keyword.iter().map(String::as_str).collect();
        }
        if let Some(Some(flatpak)) = self.flatpak.get()
            && let Some(keyword) = flatpak.keywords()
        {
            return keyword.iter().map(String::as_str).collect();
        }
        if let Some(Some(metadata)) = self.metadata.get()
            && let Some(keyword) = &metadata.keyword
        {
            return keyword.iter().map(String::as_str).collect();
        }
        Vec::new()
    }

    async fn description(&self) -> Cow<'_, str> {
        self.resolve().await;
        if let Some(Some(flatpak)) = self.flatpak.get() {
            return Cow::Borrowed(flatpak.description());
        }
        if let Some(Some(metadata)) = self.metadata.get()
            && let Some(description) = &metadata.description
        {
            return Cow::Borrowed(description);
        }
        Cow::Borrowed(self.summary())
    }

    async fn screenshots<'a>(&'a self) -> Option<AppScreenshot<'a>> {
        self.get_flatpak().await?.screenshots()
    }

    async fn main_program(&self) -> mx::Result<Cow<'_, str>> {
        let attr = self.nix_info.as_deref().ok_or_else(|| {
            mx::ErrorKind::NixCommandError(String::from("Module has no nix_info"))
        })?;
        let expr = format!("nixpkgs#{}.meta.mainProgram", attr);
        let main_program: String = nix_eval::eval_json(&[&expr]).await?;
        Ok(Cow::Owned(main_program))
    }
}
