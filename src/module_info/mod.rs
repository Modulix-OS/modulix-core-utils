use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::REMOTE_MODULE_URL;
use crate::core::app_info_trait::{AppInfoMinimal, AppPlugin, score};
use crate::core::lang::current_lang;
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
    #[serde(default)]
    modules: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct IndexOverlayEntry {
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
    sub_modules: Vec<String>,
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

static INDEX_OVERLAY: OnceCell<HashMap<String, IndexOverlayEntry>> = OnceCell::const_new();

async fn fetch_index_overlay() -> &'static HashMap<String, IndexOverlayEntry> {
    INDEX_OVERLAY
        .get_or_init(|| async {
            let Some(lang) = current_lang() else {
                return HashMap::new();
            };
            let fetch = async {
                reqwest::get(format!("{REMOTE_MODULE_URL}index.{lang}.json"))
                    .await?
                    .error_for_status()?
                    .json::<HashMap<String, IndexOverlayEntry>>()
                    .await
            };
            fetch.await.unwrap_or_default()
        })
        .await
}

fn expand_targets(index: &HashMap<String, IndexEntry>, name: &str) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([name.to_string()]);
    while let Some(current) = queue.pop_front() {
        if !seen.insert(current.clone()) {
            continue;
        }
        if let Some(children) = index.get(&current).and_then(|e| e.modules.as_ref()) {
            queue.extend(children.iter().cloned());
        }
        ordered.push(current);
    }
    ordered
}

pub async fn resolve_with_children(name: &str) -> mx::Result<Vec<String>> {
    Ok(expand_targets(fetch_index().await?, name))
}

#[cfg(feature = "app-info-gui")]
async fn fetch_metadata(path: &str) -> mx::Result<ModuleMetadata> {
    let mut metadata = reqwest::get(format!("{REMOTE_MODULE_URL}{path}/metadata.json"))
        .await
        .map_err(mx::ErrorKind::HttpError)?
        .error_for_status()
        .map_err(mx::ErrorKind::HttpError)?
        .json::<ModuleMetadata>()
        .await
        .map_err(mx::ErrorKind::HttpError)?;

    if let Some(lang) = current_lang() {
        let overlay = async {
            reqwest::get(format!("{REMOTE_MODULE_URL}{path}/metadata.{lang}.json"))
                .await?
                .error_for_status()?
                .json::<ModuleMetadata>()
                .await
        }
        .await;

        if let Ok(overlay) = overlay {
            if overlay.description.is_some() {
                metadata.description = overlay.description;
            }
            if overlay.keyword.is_some() {
                metadata.keyword = overlay.keyword;
            }
            if overlay.icon.is_some() {
                metadata.icon = overlay.icon;
            }
        }
    }

    Ok(metadata)
}

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
    fn from_entry(name: &str, entry: &IndexEntry, overlay: Option<&IndexOverlayEntry>) -> Self {
        Self {
            name: name.to_string(),
            path: entry.path.clone(),
            nix_info: entry.nix_info.clone(),
            flathub_id: entry.flathub_id.clone(),
            inline_name: overlay
                .and_then(|o| o.name.clone())
                .or_else(|| entry.name.clone()),
            inline_summary: overlay
                .and_then(|o| o.summary.clone())
                .or_else(|| entry.summary.clone()),
            #[cfg(feature = "app-info-gui")]
            inline_keyword: overlay
                .and_then(|o| o.keyword.clone())
                .or_else(|| entry.keyword.clone()),
            plugins: entry.plugins.clone(),
            sub_modules: entry.modules.clone().unwrap_or_default(),
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

    pub fn sub_modules(&self) -> &[String] {
        &self.sub_modules
    }

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
        let overlay = fetch_index_overlay().await;
        Ok(Self::from_entry(name, entry, overlay.get(name)))
    }

    async fn search(query: &str, number_app: u32) -> mx::Result<Vec<Self>> {
        let index = fetch_index().await?;
        let overlay = fetch_index_overlay().await;
        let mut modules: Vec<(u32, Self)> = index
            .iter()
            .map(|(name, entry)| {
                let ov = overlay.get(name);
                let keywords: Vec<&str> = ov
                    .and_then(|o| o.keyword.as_deref())
                    .or(entry.keyword.as_deref())
                    .unwrap_or(&[])
                    .iter()
                    .map(String::as_str)
                    .collect();
                let summary = ov
                    .and_then(|o| o.summary.as_deref())
                    .or(entry.summary.as_deref())
                    .unwrap_or("");
                let display = ov
                    .and_then(|o| o.name.as_deref())
                    .or(entry.name.as_deref())
                    .unwrap_or(name);
                let relevance = score(name, summary, &keywords, query)
                    .max(score(display, summary, &keywords, query));
                (relevance, Self::from_entry(name, entry, ov))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, children: &[&str]) -> IndexEntry {
        IndexEntry {
            path: path.to_string(),
            nix_info: None,
            flathub_id: None,
            plugins: None,
            name: None,
            summary: None,
            keyword: None,
            modules: (!children.is_empty())
                .then(|| children.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn sample_index() -> HashMap<String, IndexEntry> {
        HashMap::from([
            (
                "programs.games".to_string(),
                entry(
                    "programs/games",
                    &[
                        "programs.games.steam",
                        "programs.games.lutris",
                        "programs.games.heroic",
                        "programs.games.umu",
                    ],
                ),
            ),
            (
                "programs.games.steam".to_string(),
                entry("programs/games/steam", &[]),
            ),
            (
                "programs.games.lutris".to_string(),
                entry("programs/games/lutris", &[]),
            ),
            (
                "programs.games.heroic".to_string(),
                entry("programs/games/heroic", &[]),
            ),
            (
                "programs.games.umu".to_string(),
                entry("programs/games/umu", &[]),
            ),
        ])
    }

    #[test]
    fn meta_expands_to_itself_and_children() {
        let index = sample_index();
        assert_eq!(
            expand_targets(&index, "programs.games"),
            vec![
                "programs.games",
                "programs.games.steam",
                "programs.games.lutris",
                "programs.games.heroic",
                "programs.games.umu",
            ]
        );
    }

    #[test]
    fn leaf_expands_to_itself_only() {
        let index = sample_index();
        assert_eq!(
            expand_targets(&index, "programs.games.steam"),
            vec!["programs.games.steam"]
        );
    }

    #[test]
    fn unknown_name_expands_to_itself_only() {
        let index = sample_index();
        assert_eq!(
            expand_targets(&index, "does.not.exist"),
            vec!["does.not.exist"]
        );
    }

    #[test]
    fn expansion_deduplicates_shared_children() {
        let mut index = sample_index();
        index.insert(
            "programs.everything".to_string(),
            entry(
                "programs/everything",
                &["programs.games", "programs.games.steam"],
            ),
        );
        let targets = expand_targets(&index, "programs.everything");
        let mut unique = targets.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            targets.len(),
            unique.len(),
            "expansion must not repeat a module: {targets:?}"
        );
        assert!(targets.contains(&"programs.games.steam".to_string()));
    }

    #[test]
    fn sibling_mxpkgs_index_matches_schema() {
        use std::path::Path;

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../mxpkgs/modules/index.json");
        if !Path::new(path).exists() {
            eprintln!("skipping: sibling mxpkgs checkout not found at {path}");
            return;
        }

        let raw = std::fs::read_to_string(path).expect("read index.json");
        let index: HashMap<String, IndexEntry> =
            serde_json::from_str(&raw).expect("index.json must match the IndexEntry schema");

        assert!(!index.is_empty(), "index must not be empty");
        for (key, entry) in &index {
            assert!(!entry.path.is_empty(), "{key}: `path` is required");
            for child in entry.modules.iter().flatten() {
                assert!(
                    index.contains_key(child),
                    "{key}: meta sub-module `{child}` is absent from the index"
                );
            }
        }
    }

    #[test]
    fn overlay_text_overrides_base_and_falls_back() {
        let mut base = entry("services/llm", &[]);
        base.name = Some("Local AI".to_string());
        base.summary = Some("Local LLM runtime".to_string());

        let overlay = IndexOverlayEntry {
            name: Some("IA locale".to_string()),
            summary: None,
            keyword: None,
        };
        let localized = ModuleInfo::from_entry("services.llm", &base, Some(&overlay));
        assert_eq!(localized.display_name(), "IA locale");
        assert_eq!(localized.summary(), "Local LLM runtime");

        let english = ModuleInfo::from_entry("services.llm", &base, None);
        assert_eq!(english.display_name(), "Local AI");
        assert_eq!(english.summary(), "Local LLM runtime");
    }

    #[test]
    fn sibling_mxpkgs_fr_index_overlay_matches_schema() {
        use std::path::Path;

        let base_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../mxpkgs/modules/index.json");
        let fr_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mxpkgs/modules/index.fr.json"
        );
        if !Path::new(fr_path).exists() {
            eprintln!("skipping: overlay not found at {fr_path}");
            return;
        }

        let base: HashMap<String, IndexEntry> =
            serde_json::from_str(&std::fs::read_to_string(base_path).expect("read index.json"))
                .expect("index.json must match the IndexEntry schema");
        let fr: HashMap<String, IndexOverlayEntry> =
            serde_json::from_str(&std::fs::read_to_string(fr_path).expect("read index.fr.json"))
                .expect("index.fr.json must match the IndexOverlayEntry schema");

        assert!(!fr.is_empty(), "overlay must not be empty");
        for key in fr.keys() {
            assert!(
                base.contains_key(key),
                "{key}: overlay id is absent from the base index"
            );
        }
    }

    #[cfg(feature = "app-info-gui")]
    #[test]
    fn sibling_mxpkgs_fr_metadata_matches_schema() {
        use std::path::Path;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mxpkgs/modules/services/llm/metadata.fr.json"
        );
        if !Path::new(path).exists() {
            eprintln!("skipping: overlay not found at {path}");
            return;
        }

        let metadata: ModuleMetadata =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read metadata.fr.json"))
                .expect("metadata.fr.json must match the ModuleMetadata schema");
        assert!(
            metadata.description.is_some(),
            "fr metadata overlay should provide a description"
        );
    }
}
