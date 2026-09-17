use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

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
    icon_name: Option<String>,
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

/// Directory an `index.json` copy is expected to live in (see
/// [`read_local_index`]): the config repo's cache directory, shared with the
/// package index. `$MX_MODULE_INDEX_DIR` still overrides it on its own, for
/// tests and dev checkouts.
fn local_module_index_dir() -> std::path::PathBuf {
    std::env::var_os("MX_MODULE_INDEX_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(crate::cache_dir)
}

/// `modules/index.json` is only 20 entries / 5 KB and part of the system
/// closure, so reading it locally is instant and works offline — unlike
/// `REMOTE_MODULE_URL`, which hits `raw.githubusercontent.com` on every
/// process's first search. `None` on any I/O/parse error (missing file,
/// no such module deployed yet, …), so the caller's HTTP fallback still runs.
async fn read_local_index() -> Option<HashMap<String, IndexEntry>> {
    let path = local_module_index_dir().join("index.json");
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn fetch_index() -> mx::Result<&'static HashMap<String, IndexEntry>> {
    INDEX
        .get_or_try_init(|| async {
            if let Some(index) = read_local_index().await {
                return Ok(index);
            }
            crate::core::http_client::client()?
                .get(format!("{REMOTE_MODULE_URL}index.json"))
                .send()
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
static EMPTY_INDEX_OVERLAY: LazyLock<HashMap<String, IndexOverlayEntry>> =
    LazyLock::new(HashMap::new);

/// Unlike [`fetch_index`], a failed overlay fetch must not be cached forever:
/// `get_or_try_init` only ever populates the cell on success, so a transient
/// network failure is retried on the next call instead of permanently hiding
/// every localized name/summary for the process lifetime.
async fn fetch_index_overlay() -> &'static HashMap<String, IndexOverlayEntry> {
    let Some(lang) = current_lang() else {
        return &EMPTY_INDEX_OVERLAY;
    };
    let Ok(client) = crate::core::http_client::client() else {
        return &EMPTY_INDEX_OVERLAY;
    };

    INDEX_OVERLAY
        .get_or_try_init(|| async {
            client
                .get(format!("{REMOTE_MODULE_URL}index.{lang}.json"))
                .send()
                .await?
                .error_for_status()?
                .json::<HashMap<String, IndexOverlayEntry>>()
                .await
        })
        .await
        .unwrap_or(&EMPTY_INDEX_OVERLAY)
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

/// Modules whose `flathub_id` equals `app_id` (the index is already cached by
/// [`fetch_index`]). Used to prepend the Modulix module row(s) to the
/// `alternate-of` version-selector for an app that also has a module. Does
/// **not** call [`ModuleInfo::resolve`] (a network round-trip per module): the
/// version-selector row only needs `origin_ui` + the packaging format, not an
/// icon or description.
#[cfg(feature = "app-info-gui")]
pub async fn modules_for_app_id(app_id: &str) -> mx::Result<Vec<ModuleInfo>> {
    let index = fetch_index().await?;
    let overlay = fetch_index_overlay().await;
    Ok(index
        .iter()
        .filter(|(_, entry)| entry.flathub_id.as_deref() == Some(app_id))
        .map(|(name, entry)| ModuleInfo::from_entry(name, entry, overlay.get(name)))
        .collect())
}

#[cfg(feature = "app-info-gui")]
async fn fetch_metadata(path: &str) -> mx::Result<ModuleMetadata> {
    let primary = async {
        crate::core::http_client::client()?
            .get(format!("{REMOTE_MODULE_URL}{path}/metadata.json"))
            .send()
            .await
            .map_err(mx::ErrorKind::HttpError)?
            .error_for_status()
            .map_err(mx::ErrorKind::HttpError)?
            .json::<ModuleMetadata>()
            .await
            .map_err(mx::ErrorKind::HttpError)
    };

    // No overlay language: skip the second request entirely rather than
    // firing it off just to discard the result.
    let overlay = async {
        let lang = current_lang()?;
        let client = crate::core::http_client::client().ok()?;
        let fetch = async {
            client
                .get(format!("{REMOTE_MODULE_URL}{path}/metadata.{lang}.json"))
                .send()
                .await?
                .error_for_status()?
                .json::<ModuleMetadata>()
                .await
        };
        fetch.await.ok()
    };

    let (metadata, overlay) = tokio::join!(primary, overlay);
    let mut metadata = metadata?;

    if let Some(overlay) = overlay {
        if overlay.description.is_some() {
            metadata.description = overlay.description;
        }
        if overlay.keyword.is_some() {
            metadata.keyword = overlay.keyword;
        }
        if overlay.icon.is_some() {
            metadata.icon = overlay.icon;
        }
        if overlay.icon_name.is_some() {
            metadata.icon_name = overlay.icon_name;
        }
    }

    Ok(metadata)
}

/// Process-wide cache of [`list_plugins_in_namespace`] results, keyed by
/// namespace: forcing `meta.description` across a whole nixpkgs namespace is
/// expensive, and the details page re-requests the same module's plugins on
/// every refine.
static PLUGIN_NAMESPACE_CACHE: LazyLock<Mutex<HashMap<String, Vec<AppPlugin>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn list_plugins_in_namespace(namespace: &str) -> mx::Result<Vec<AppPlugin>> {
    if let Some(cached) = PLUGIN_NAMESPACE_CACHE.lock().unwrap().get(namespace) {
        return Ok(cached.clone());
    }

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

    let plugins: Vec<AppPlugin> = raw
        .into_iter()
        .map(|(name, value)| {
            let description = value
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            AppPlugin { name, description }
        })
        .collect();

    PLUGIN_NAMESPACE_CACHE
        .lock()
        .unwrap()
        .insert(namespace.to_string(), plugins.clone());

    Ok(plugins)
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

    async fn search_scored(query: &str, number_app: u32) -> mx::Result<Vec<(u32, Self)>> {
        let index = fetch_index().await?;
        let overlay = fetch_index_overlay().await;
        // Score off borrowed data first — `Self::from_entry` clones several
        // `String`s per module — so a non-match never pays that cost.
        let mut modules: Vec<(u32, Self)> = index
            .iter()
            .filter_map(|(name, entry)| {
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
                (relevance > 0).then(|| (relevance, Self::from_entry(name, entry, ov)))
            })
            .collect();
        modules.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        modules.truncate(number_app as usize);
        Ok(modules)
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

    fn icon_name(&self) -> Option<&str> {
        if let Some(Some(metadata)) = self.metadata.get()
            && let Some(icon_name) = metadata.icon_name.as_deref()
        {
            return Some(icon_name);
        }
        crate::package_info::packages_for_app_id(self.flathub_id.as_deref()?)
            .first()
            .and_then(|attr| crate::package_info::icon_name_for_package(attr))
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
