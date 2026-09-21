//! Modulix modules (`mx.<name>.enable` NixOS fragments) as an app-info source:
//! [`ModuleInfo`] implements the [`AppInfoMinimal`] / [`AppInfoGui`] traits over
//! `modules/index.json` (routing + search metadata) and, lazily, over Flathub or
//! the module's own `metadata.json` (rich GUI info).

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
///
/// # Fields
/// * `path` - relative directory of the module under `modules/`, e.g.
///   `programs/games/steam`.
/// * `nix_info` - nixpkgs attribute used for nix-side metadata
///   (`meta.mainProgram`…), when the module maps to one.
/// * `flathub_id` - Flathub application id backing this module's GUI info,
///   when the module wraps a Flatpak-able app.
/// * `plugins` - nixpkgs namespace the module pulls its plugins from (see
///   [`list_plugins_in_namespace`]), when the module exposes a plugin list.
/// * `name` - inline display name, in the absence of a localized overlay
///   entry (see [`IndexOverlayEntry::name`]).
/// * `summary` - inline one-line summary, in the absence of a localized
///   overlay entry (see [`IndexOverlayEntry::summary`]).
/// * `keyword` - inline extra search terms, in the absence of a localized
///   overlay entry (see [`IndexOverlayEntry::keyword`]).
/// * `modules` - keys of the sub-modules this entry is a meta-module for;
///   `None`/empty for a leaf module. Walked by [`expand_targets`].
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

/// One `modules/index.<lang>.json` entry: the localized override of an
/// [`IndexEntry`]'s text fields for the current process locale (see
/// [`current_lang`]). A field absent here leaves the base [`IndexEntry`] value
/// in place (see [`ModuleInfo::from_entry`]).
///
/// # Fields
/// * `name` - localized display name, overriding [`IndexEntry::name`] when set.
/// * `summary` - localized one-line summary, overriding [`IndexEntry::summary`]
///   when set.
/// * `keyword` - localized extra search terms, overriding [`IndexEntry::keyword`]
///   when set.
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
///
/// # Fields
/// * `icon` - URL of the module's icon, when it declares a remote one.
/// * `icon_name` - name of the icon in the user's icon theme, preferred over
///   `icon` when both are set (see [`ModuleInfo::icon_name`]).
/// * `description` - long, multi-paragraph description; falls back to the
///   inline summary when absent (see [`ModuleInfo::description`]).
/// * `keyword` - extra search terms, in the absence of an inline
///   [`IndexEntry::keyword`]/overlay override.
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
///
/// # Fields
/// * `name` - the module's key in `modules/index.json`, e.g.
///   `programs.games.steam`; also its own [`AppInfoMinimal::package_name`].
/// * `path` - relative directory of the module under `modules/`, copied from
///   the index entry's `path`.
/// * `nix_info` - nixpkgs attribute for nix-side metadata, copied from the
///   index entry's `nix_info`.
/// * `flathub_id` - Flathub application id backing this module's GUI info,
///   copied from the index entry's `flathub_id`.
/// * `inline_name` - display name resolved at construction time from the
///   localized overlay, falling back to the index entry's `name`; see
///   `ModuleInfo::from_entry`.
/// * `inline_summary` - one-line summary resolved the same way from the
///   overlay, falling back to the index entry's `summary`.
/// * `inline_keyword` - extra search terms resolved the same way from the
///   overlay, falling back to the index entry's `keyword`.
/// * `plugins` - nixpkgs namespace the module pulls its plugins from, copied
///   from the index entry's `plugins`.
/// * `sub_modules` - keys of this module's sub-modules, copied from the
///   index entry's `modules` (empty for a leaf module).
/// * `flatpak` - lazily-resolved Flathub payload, populated on first
///   `get_flatpak`/[`ModuleInfo::resolve`] call when `flathub_id` is set;
///   `None` once resolved if the fetch failed.
/// * `metadata` - lazily-resolved `metadata.json` payload, populated on first
///   `get_metadata`/[`ModuleInfo::resolve`] call when `flathub_id` is absent;
///   `None` once resolved if the fetch failed.
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
///
/// # Returns
/// `$MX_MODULE_INDEX_DIR` when set, else [`crate::cache_dir`].
fn local_module_index_dir() -> std::path::PathBuf {
    std::env::var_os("MX_MODULE_INDEX_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(crate::cache_dir)
}

/// `modules/index.json` is only 20 entries / 5 KB and part of the system
/// closure, so reading it locally is instant and works offline — unlike
/// `REMOTE_MODULE_URL`, which hits `raw.githubusercontent.com` on every
/// process's first search.
///
/// # Returns
/// The parsed index read from `<local_module_index_dir()>/index.json`, or
/// `None` on any I/O or parse error (missing file, no such module deployed
/// yet, malformed JSON, …) — the error itself is discarded, so the caller's
/// HTTP fallback ([`fetch_index`]) always still runs in that case.
async fn read_local_index() -> Option<HashMap<String, IndexEntry>> {
    let path = local_module_index_dir().join("index.json");
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The process-wide, memoized `modules/index.json`: local file first (see
/// [`read_local_index`]), falling back to `REMOTE_MODULE_URL` over HTTP.
///
/// # Returns
/// A `'static` reference to the cached index. Once populated (from either
/// source), the same value is returned on every later call for the life of the
/// process — a successful fetch is never retried or refreshed.
///
/// # Errors
/// Only reachable via the HTTP fallback, when the local file is absent or
/// unreadable: [`mx::ErrorKind::RequestSenderError`] if the shared HTTP client
/// cannot be built, [`mx::ErrorKind::HttpError`] if the request fails, the
/// response status is an error, or the body does not deserialize into the
/// index's schema.
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

/// Process-wide cache of `modules/index.<lang>.json` (fetched once per
/// successful attempt; see [`fetch_index_overlay`]).
static INDEX_OVERLAY: OnceCell<HashMap<String, IndexOverlayEntry>> = OnceCell::const_new();

/// Stand-in overlay used whenever there is nothing to overlay with (no
/// process locale, no HTTP client, or a failed fetch): an empty map makes
/// every lookup miss and fall back to the base [`IndexEntry`] text.
static EMPTY_INDEX_OVERLAY: LazyLock<HashMap<String, IndexOverlayEntry>> =
    LazyLock::new(HashMap::new);

/// The process-wide, memoized `modules/index.<lang>.json` for the current
/// locale (see [`current_lang`]).
///
/// Unlike [`fetch_index`], a failed overlay fetch must not be cached forever:
/// `get_or_try_init` only ever populates the cell on success, so a transient
/// network failure is retried on the next call instead of permanently hiding
/// every localized name/summary for the process lifetime.
///
/// # Returns
/// A `'static` reference to the cached overlay, or [`EMPTY_INDEX_OVERLAY`]
/// when there is no process locale ([`current_lang`] returns `None`), the
/// shared HTTP client cannot be built, or the request/parse fails for any
/// reason. Never returns an error: the overlay is a pure enhancement.
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

/// Expands a module key into itself plus every sub-module reachable through
/// [`IndexEntry::modules`], breadth-first, so enabling a meta-module (e.g.
/// `programs.games`) also targets its children (`programs.games.steam`, …).
///
/// # Parameters
/// * `index` - the module index to walk `modules` links in.
/// * `name` - the module key to expand; need not exist in `index`.
///
/// # Pre-conditions
/// None: an unknown `name` or a cycle in `modules` links are both handled
/// (a key absent from `index` simply has no children; each key is visited
/// at most once via `seen`).
///
/// # Returns
/// `name` followed by its descendants in breadth-first, first-seen order,
/// each key appearing exactly once. `vec![name.to_string()]` when `name` is
/// unknown to `index` or is a leaf module.
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

/// Resolves a module key to the full set of module keys it targets: itself
/// plus every sub-module, for a meta-module (see `expand_targets`).
///
/// # Parameters
/// * `name` - the module key to resolve; need not exist in the index.
///
/// # Returns
/// `name` followed by its descendants, in breadth-first order (see
/// `expand_targets`); just `[name]` if `name` is a leaf or unknown module.
///
/// # Errors
/// Whatever `fetch_index` returns.
pub async fn resolve_with_children(name: &str) -> mx::Result<Vec<String>> {
    Ok(expand_targets(fetch_index().await?, name))
}

/// Modules whose `flathub_id` equals `app_id` (the index is already cached by
/// `fetch_index`). Used to prepend the Modulix module row(s) to the
/// `alternate-of` version-selector for an app that also has a module. Does
/// **not** call [`ModuleInfo::resolve`] (a network round-trip per module): the
/// version-selector row only needs `origin_ui` + the packaging format, not an
/// icon or description.
///
/// # Parameters
/// * `app_id` - the Flathub application id to match against each module's
///   `flathub_id` index entry.
///
/// # Returns
/// Every module whose `flathub_id` equals `app_id`, each still unresolved
/// (its `flatpak`/`metadata` cell empty); empty when none match.
///
/// # Errors
/// Whatever `fetch_index` returns.
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

/// Fetches a module's `metadata.json` from `REMOTE_MODULE_URL`, merging in the
/// localized `metadata.<lang>.json` overlay (see [`current_lang`]) when one is
/// available. The base request and the overlay request run concurrently.
///
/// # Parameters
/// * `path` - the module's directory under `modules/` (its [`IndexEntry::path`]
///   / [`ModuleInfo::path`]), used to build both request URLs.
///
/// # Returns
/// The base `metadata.json`, with each of `description`/`keyword`/`icon`/
/// `icon_name` replaced by the overlay's value when the overlay request
/// succeeded and that field is `Some` there; fields the overlay left `None`
/// keep the base value.
///
/// # Errors
/// [`mx::ErrorKind::RequestSenderError`]/[`mx::ErrorKind::HttpError`] from the
/// base `metadata.json` request (see [`crate::core::http_client::client`]) —
/// a failure in the overlay request is not propagated: it is silently
/// dropped and the base metadata is returned unmodified.
///
/// # Post-conditions
/// When there is no overlay language (see [`current_lang`]), the overlay
/// request is skipped entirely rather than fired off just to discard the
/// result.
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

/// Lists every attribute of a nixpkgs namespace as an [`AppPlugin`], evaluating
/// each attribute's `meta.description` (via `nix eval --apply`) in one call.
/// Results are cached per `namespace` in [`PLUGIN_NAMESPACE_CACHE`] for the
/// life of the process.
///
/// # Parameters
/// * `namespace` - nixpkgs attribute path under
///   `legacyPackages.<TARGET_NIX>`, e.g. `vscode-extensions.ms-python`.
///
/// # Pre-conditions
/// `nix` must be on `PATH` with flakes enabled, as for any [`nix_eval`] call.
///
/// # Returns
/// One [`AppPlugin`] per attribute in `namespace`, each `description` empty
/// when `meta.description` is absent or its evaluation fails (`tryEval`
/// swallows the failure Nix-side rather than erroring the whole call).
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] on eval timeout, non-zero exit, or a
/// result that does not parse as JSON; [`mx::ErrorKind::IOError`] if `nix`
/// cannot be spawned; [`mx::ErrorKind::FromUtf8Error`] if its stdout is not
/// UTF-8 (see [`nix_eval::eval_json`]). A cache hit short-circuits before any
/// of this runs.
///
/// # Post-conditions
/// On success, `namespace`'s result is stored in [`PLUGIN_NAMESPACE_CACHE`];
/// a failed evaluation leaves the cache untouched, so the next call retries.
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
    /// Builds a [`ModuleInfo`] from an index entry and its optional localized
    /// overlay, resolving `inline_name`/`inline_summary`/`inline_keyword` to
    /// the overlay's value when set, else the entry's own (see
    /// [`IndexOverlayEntry`]). `flatpak`/`metadata` start empty, unresolved.
    ///
    /// # Parameters
    /// * `name` - the module's key in the index, stored as `self.name`.
    /// * `entry` - the base index entry to copy routing/search fields from.
    /// * `overlay` - the matching localized overlay entry, when the current
    ///   locale (see [`current_lang`]) has one for `name`.
    ///
    /// # Returns
    /// The constructed [`ModuleInfo`].
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

    /// Keys of this module's sub-modules, empty for a leaf module.
    pub fn sub_modules(&self) -> &[String] {
        &self.sub_modules
    }

    /// Lists the plugins this module exposes, via its `plugins` nixpkgs
    /// namespace (see `list_plugins_in_namespace`).
    ///
    /// # Returns
    /// One `AppPlugin` per attribute in the namespace.
    ///
    /// # Errors
    /// [`mx::ErrorKind::PackageDoesNotHaveAPlugin`] when the module has no
    /// `plugins` namespace; otherwise whatever `list_plugins_in_namespace`
    /// returns.
    pub async fn list_plugins(&self) -> mx::Result<Vec<AppPlugin>> {
        let namespace = self
            .plugins
            .as_deref()
            .ok_or(mx::ErrorKind::PackageDoesNotHaveAPlugin)?;
        list_plugins_in_namespace(namespace).await
    }

    /// Resolves and caches `self.flatpak` from Flathub, when `flathub_id` is
    /// set. The `flatpak` [`OnceCell`] means this only ever fetches once per
    /// instance; a failed fetch is cached as `None` and never retried.
    ///
    /// # Returns
    /// The resolved [`FlatpakInfo`], or `None` when there is no `flathub_id`
    /// or the fetch failed.
    #[cfg(feature = "app-info-gui")]
    async fn get_flatpak(&self) -> Option<&FlatpakInfo> {
        let id = self.flathub_id.as_deref()?;
        self.flatpak
            .get_or_init(|| async { FlatpakInfo::new(id).await.ok() })
            .await
            .as_ref()
    }

    /// Resolves and caches `self.metadata` from the module's `metadata.json`
    /// (via [`fetch_metadata`]). The `metadata` [`OnceCell`] means this only
    /// ever fetches once per instance; a failed fetch is cached as `None` and
    /// never retried.
    ///
    /// # Returns
    /// The resolved metadata, or `None` when the fetch failed.
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
    /// Loads one module by its index key.
    ///
    /// # Parameters
    /// * `name` - the module's key in `modules/index.json`.
    ///
    /// # Returns
    /// The module, with its localized overlay already applied when the
    /// current locale has one for `name`; `flatpak`/`metadata` unresolved.
    ///
    /// # Errors
    /// [`mx::ErrorKind::PackageNotFound`] when `name` is absent from the
    /// index; otherwise whatever `fetch_index` returns.
    async fn new(name: &str) -> mx::Result<Self> {
        let index = fetch_index().await?;
        let entry = index.get(name).ok_or(mx::ErrorKind::PackageNotFound)?;
        let overlay = fetch_index_overlay().await;
        Ok(Self::from_entry(name, entry, overlay.get(name)))
    }

    /// Searches every module in the index by name/display name, summary and
    /// keywords (see `score`), preferring the localized overlay's text over
    /// the index entry's own when both a module and the query relevance are
    /// computed. Non-matching modules are scored off borrowed data only, so
    /// `Self::from_entry`'s clones are paid only for actual hits.
    ///
    /// # Parameters
    /// * `query` - free-text search terms, matched case-insensitively.
    /// * `number_app` - upper bound on the number of hits returned.
    ///
    /// # Returns
    /// `(score, module)` pairs, highest score first, truncated to
    /// `number_app`; modules scoring 0 are excluded.
    ///
    /// # Errors
    /// Whatever `fetch_index` returns.
    async fn search_scored(query: &str, number_app: u32) -> mx::Result<Vec<(u32, Self)>> {
        let index = fetch_index().await?;
        let overlay = fetch_index_overlay().await;
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

    /// # Returns
    /// The module's index key, e.g. `programs.games.steam`.
    fn package_name(&self) -> &str {
        &self.name
    }

    /// # Returns
    /// `inline_name` (the overlay's localized name, or the index entry's
    /// `name`) when set, else the module's index key.
    fn display_name(&self) -> &str {
        self.inline_name.as_deref().unwrap_or(&self.name)
    }

    /// # Returns
    /// `inline_summary` (the overlay's localized summary, or the index
    /// entry's `summary`) when set, else the empty string.
    fn summary(&self) -> &str {
        self.inline_summary.as_deref().unwrap_or("")
    }
}

#[cfg(feature = "app-info-gui")]
impl AppInfoGui for ModuleInfo {
    /// # Returns
    /// `flathub_id`, unchanged: for a module it doubles as the AppStream id.
    fn id(&self) -> Option<&str> {
        self.flathub_id.as_deref()
    }

    /// Modules keep their own display name (the plugin never appends a Flatpak
    /// title for them), so this is always `None`.
    fn app_name(&self) -> Option<&str> {
        None
    }

    /// # Returns
    /// The already-resolved [`FlatpakInfo::icon`] URL when `flatpak` was
    /// resolved (see [`ModuleInfo::resolve`]), else `metadata.icon` when
    /// `metadata` was resolved instead, else `None` — including when neither
    /// cell has been resolved yet.
    fn icon(&self) -> Option<&Url> {
        if let Some(Some(flatpak)) = self.flatpak.get() {
            return Some(flatpak.icon());
        }
        if let Some(Some(metadata)) = self.metadata.get() {
            return metadata.icon.as_deref();
        }
        None
    }

    /// # Returns
    /// `metadata.icon_name` when `metadata` was resolved (see
    /// [`ModuleInfo::resolve`]) and carries one; otherwise, when `flathub_id`
    /// is set, the theme icon name of the first nixpkgs package Flathub's
    /// `app_id` maps to (via [`crate::package_info::packages_for_app_id`] then
    /// [`crate::package_info::icon_name_for_package`]); `None` if neither
    /// source has an answer.
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

    /// # Returns
    /// `inline_keyword` when set; else the resolved Flathub keywords when
    /// `flatpak` was resolved and carries some; else `metadata.keyword` when
    /// `metadata` was resolved and carries some; else an empty vector.
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

    /// Resolves the remote GUI source first (see [`ModuleInfo::resolve`]), so
    /// this always reflects the latest fetch attempt regardless of prior
    /// calls.
    ///
    /// # Returns
    /// The resolved Flathub description when `flathub_id` is set and the
    /// fetch succeeded; else `metadata.description` when the `metadata.json`
    /// fetch succeeded and carries one; else [`ModuleInfo::summary`] (itself
    /// possibly empty).
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

    /// Resolves `flatpak` (via `get_flatpak`) rather than requiring a prior
    /// [`ModuleInfo::resolve`] call, since screenshots only ever come from
    /// Flathub.
    ///
    /// # Returns
    /// [`FlatpakInfo::screenshots`] when `flathub_id` is set and the fetch
    /// succeeded; `None` otherwise (no `flathub_id`, or the fetch failed).
    async fn screenshots<'a>(&'a self) -> Option<AppScreenshot<'a>> {
        self.get_flatpak().await?.screenshots()
    }

    /// # Returns
    /// `meta.mainProgram` of the module's `nix_info` nixpkgs attribute,
    /// evaluated fresh on every call (not cached).
    ///
    /// # Errors
    /// [`mx::ErrorKind::NixCommandError`] when the module has no `nix_info`;
    /// otherwise whatever `nix_eval::eval_json` returns for the evaluation
    /// itself.
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

    /// Builds a minimal [`IndexEntry`] for `expand_targets` tests: only
    /// `path` and `modules` are populated, every other field is `None`.
    ///
    /// # Parameters
    /// * `path` - stored verbatim as the entry's `path`.
    /// * `children` - sub-module keys; stored as `modules` when non-empty,
    ///   else `None` (mirrors real index entries, which omit an empty list).
    ///
    /// # Returns
    /// The constructed entry.
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

    /// A small fixture index: the `programs.games` meta-module and its four
    /// leaf children (`steam`, `lutris`, `heroic`, `umu`), used by the
    /// `expand_targets` tests.
    ///
    /// # Returns
    /// The fixture index, keyed by module name.
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
