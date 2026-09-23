//! nixpkgs packages as the store sees them: [`NixPackage`] implements the
//! app-info traits over the on-disk index, `nix` evaluation and the Flathub
//! enrichment.

#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;
use std::{collections::HashMap, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::core::app_info_trait::AppInfoMinimal;
use crate::core::app_info_trait::PLUGIN_NAMESPACE_PREFIXES;
use crate::core::app_info_trait::score;
use crate::core::license;
use crate::core::nix_eval;
use crate::mx;
use crate::package_index;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::AppInfoGui;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::AppScreenshot;

#[cfg(feature = "app-info-gui")]
use crate::core::app_info_trait::FlatpakInfo;

#[cfg(feature = "app-info-gui")]
pub(crate) mod package_basic_info;

#[cfg(feature = "app-info-gui")]
use tokio::sync::OnceCell;

/// An absolute URL, kept as a distinct name for readability in signatures.
type Url = str;

/// One nixpkgs package, as the store reads it: the index row, plus the Flathub
/// metadata fetched on demand for the GUI consumers.
///
/// # Fields
/// * `pkg_name` - the nixpkgs attribute path, which is the install key; not
///   serialised, since it is the map key on the index side.
/// * `description` - the package's `meta.description`.
/// * `pname` - its nixpkgs `pname`, used as the display name.
/// * `version` - its version string.
/// * `outputs` - its nix outputs, when the index carried them.
/// * `flatpak` - Flathub payload, fetched at most once per instance and only
///   when a GUI accessor needs it; not serialised.
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
    /// The package's Flathub payload, fetched on first use and cached.
    ///
    /// # Returns
    /// The payload, or `None` when the package maps to no AppStream id, or when
    /// the request failed - the error is swallowed, a missing enrichment being a
    /// normal outcome rather than a failure.
    ///
    /// # Post-conditions
    /// The first call decides for the lifetime of this instance: a failed fetch
    /// is not retried.
    #[cfg(feature = "app-info-gui")]
    async fn get_flatpak(&self) -> Option<&FlatpakInfo> {
        self.flatpak
            .get_or_init(|| async { FlatpakInfo::new(self.id()?).await.ok() })
            .await
            .as_ref()
    }

    /// Package version string (e.g. `"115.0"`), as evaluated from nixpkgs.
    ///
    /// # Returns
    /// The version, borrowed from `self`; empty when the index row carried none.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Evaluates the package's nix outputs.
    ///
    /// # Returns
    /// The output names (`out`, `dev`, `man`, …).
    ///
    /// # Pre-conditions
    /// `nix` must be on `PATH` with flakes enabled; this hits the network on a
    /// cold nix cache. Prefer the `outputs` field when the index already filled
    /// it.
    ///
    /// # Errors
    /// [`mx::ErrorKind::NixCommandError`] when the evaluation fails or times
    /// out, notably for a broken attribute.
    pub async fn get_outputs(&self) -> mx::Result<Vec<String>> {
        let expr = format!("nixpkgs#{}.outputs", self.pkg_name);
        nix_eval::eval_json(&[&expr]).await
    }
}

impl AppInfoMinimal for NixPackage {
    /// Loads one package by evaluating its nixpkgs attribute.
    ///
    /// # Parameters
    /// * `pkg_name` - the nixpkgs attribute path (e.g. `firefox`).
    ///
    /// # Returns
    /// The package, with the fields `pname`, `version`, `description` and
    /// `outputs` filled in - each falling back to an empty value when the
    /// attribute does not define it.
    ///
    /// # Pre-conditions
    /// `nix` must be on `PATH` with flakes enabled, and the `nixpkgs` registry
    /// entry must resolve.
    ///
    /// # Errors
    /// [`mx::ErrorKind::NixCommandError`] when the evaluation fails, which is
    /// also how an unknown attribute surfaces.
    ///
    /// # Post-conditions
    /// Evaluates through the `nixpkgs#<attr>` flake installable rather than
    /// `import <nixpkgs> {}`, since the latter needs `--impure` to resolve
    /// `<nixpkgs>` on any nix with flakes enabled and fails outright otherwise.
    async fn new(pkg_name: &str) -> mx::Result<Self> {
        let installable = format!("nixpkgs#{pkg_name}");
        let mut info: NixPackage = nix_eval::eval_json(&[
            &installable,
            "--apply",
            r#"p: { pname = p.pname or (p.name or ""); version = p.version or ""; description = p.meta.description or ""; outputs = p.outputs or []; }"#,
        ])
        .await?;
        info.pkg_name = pkg_name.to_string();
        Ok(info)
    }

    /// Searches nixpkgs, through the on-disk index when it is available and
    /// through `nix search` otherwise.
    ///
    /// # Parameters
    /// * `query` - free-text search terms.
    /// * `number_app` - upper bound on the number of hits returned.
    ///
    /// # Returns
    /// `(score, package)` pairs, best match first, at most `number_app` of them;
    /// hits scoring 0 are dropped. Packages from a plugin namespace are never
    /// returned - they belong to a module instead. Index hits carry no `outputs`;
    /// call [`NixPackage::get_outputs`] for those.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] or [`mx::ErrorKind::NixCommandError`] when the
    /// `nix search` fallback cannot run or fails, and
    /// [`mx::ErrorKind::ParseError`] when its output does not deserialise. The
    /// index path does not fail: a missing or stale index just means the
    /// fallback runs - which is also what happens on the first-ever run, or
    /// whenever the background index build has not caught up yet.
    ///
    /// # Post-conditions
    /// The `nix search` fallback process is spawned with `kill_on_drop`, so a
    /// cancelled search never leaves `nix` running in the background.
    async fn search_scored(query: &str, number_app: u32) -> mx::Result<Vec<(u32, Self)>> {
        if let Some(index) = package_index::get().await {
            let hits = package_index::search(&index, query, number_app as usize);
            return Ok(hits
                .into_iter()
                .map(|(relevance, row)| {
                    (
                        relevance,
                        Self {
                            pkg_name: row.attr.to_string(),
                            version: row.version.to_string(),
                            description: row.description.to_string(),
                            pname: row.pname.to_string(),
                            outputs: vec![],
                            #[cfg(feature = "app-info-gui")]
                            flatpak: OnceCell::new(),
                        },
                    )
                })
                .collect());
        }

        let output = tokio::process::Command::new("nix")
            .args(["search", "nixpkgs", "--json", query])
            .env("NIXPKGS_ALLOW_UNFREE", "1")
            .kill_on_drop(true)
            .output()
            .await
            .map_err(mx::ErrorKind::IOError)?;

        if !output.status.success() {
            return Err(mx::ErrorKind::NixCommandError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let raw: HashMap<String, NixPackage> =
            serde_json::from_slice(&output.stdout).map_err(mx::ErrorKind::ParseError)?;

        let prefix = format!("legacyPackages.{}.", env!("TARGET_NIX"));

        let mut packages: Vec<(u32, Self)> = raw
            .into_iter()
            .filter_map(|(key, value)| {
                let name = key.strip_prefix(&prefix).unwrap_or(&key);
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
                (relevance > 0).then_some((
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
        Ok(packages)
    }

    /// # Returns
    /// The nixpkgs attribute path, which is what an install request must carry.
    fn package_name(&self) -> &str {
        &self.pkg_name
    }

    /// # Returns
    /// The package's `pname`, which is not necessarily its AppStream name - the
    /// `AppInfoGui::app_name` accessor gives that.
    fn display_name(&self) -> &str {
        &self.pname
    }

    /// # Returns
    /// The package's `meta.description`; empty when it declares none.
    fn summary(&self) -> &str {
        &self.description
    }
}

#[cfg(feature = "app-info-gui")]
impl AppInfoGui for NixPackage {
    /// # Returns
    /// The AppStream component id from the generated table, looked up by
    /// attribute (`pkg_name`) first and only falling back to `pname` for the
    /// rare attr-not-in-table case; `None` when neither is listed. Looking up
    /// [`NixPackage::display_name`] alone would miss every variant whose
    /// attribute differs from its pname (firefox-bin, firefox-esr, …).
    fn id(&self) -> Option<&str> {
        package_basic_info::get_app_id(&self.pkg_name)
            .or_else(|| package_basic_info::get_app_id(self.display_name()))
    }

    /// # Returns
    /// The AppStream display name from the generated table, looked up by
    /// attribute then by `pname`, or `None` when neither is listed.
    fn app_name(&self) -> Option<&str> {
        package_basic_info::get_name(&self.pkg_name)
            .or_else(|| package_basic_info::get_name(self.display_name()))
    }

    /// # Returns
    /// Always `None`: a nix package has no icon URL of its own, so the UI relies
    /// on the themed icon name instead.
    fn icon(&self) -> Option<&Url> {
        None
    }

    /// # Returns
    /// The themed icon name from the generated table, looked up by attribute then
    /// by `pname`, or `None` when neither is listed.
    fn icon_name(&self) -> Option<&str> {
        package_basic_info::get_icon_name(&self.pkg_name)
            .or_else(|| package_basic_info::get_icon_name(self.display_name()))
    }

    /// # Returns
    /// The keywords of the generated table, looked up by attribute only; empty
    /// when the attribute is not listed.
    fn keyword(&self) -> Vec<&str> {
        package_basic_info::get_keywords(&self.pkg_name)
            .map(<[&str]>::to_vec)
            .unwrap_or_default()
    }

    /// # Returns
    /// Flathub's long description when the payload could be fetched, else the
    /// package's own one-line `meta.description` as a fallback. Borrowed either
    /// way; the first call may trigger the Flathub request.
    async fn description(&self) -> Cow<'_, str> {
        if let Some(flatpak) = self.get_flatpak().await {
            Cow::Borrowed(flatpak.description())
        } else {
            Cow::Borrowed(self.summary())
        }
    }

    /// # Returns
    /// Flathub's screenshots, or `None` when the package maps to no AppStream id,
    /// the payload could not be fetched, or it carries none. nixpkgs itself has
    /// no screenshot metadata, so this is the only source.
    async fn screenshots<'a>(&'a self) -> Option<AppScreenshot<'a>> {
        self.get_flatpak().await?.screenshots()
    }

    /// # Returns
    /// The package's `meta.mainProgram`, evaluated with `nix` at each call.
    ///
    /// # Errors
    /// [`mx::ErrorKind::NixCommandError`] when the attribute does not define
    /// `meta.mainProgram`, or the evaluation fails or times out.
    async fn main_program(&self) -> mx::Result<Cow<'_, str>> {
        let expr = format!("nixpkgs#{}.meta.mainProgram", self.pkg_name);
        let main_program: String = nix_eval::eval_json(&[&expr]).await?;
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
pub static FLATPAK_PREFERRED_APP_IDS: &[&str] = &[];

/// Whether the Flatpak source is preferred over nixpkgs for the given canonical
/// app-id (see [`FLATPAK_PREFERRED_APP_IDS`]).
///
/// # Parameters
/// * `app_id` - AppStream component id, compared exactly.
///
/// # Returns
/// `true` only for the curated ids; `false` for everything else, which is the
/// default of preferring nixpkgs.
#[cfg(feature = "app-info-gui")]
pub fn is_flatpak_preferred(app_id: &str) -> bool {
    FLATPAK_PREFERRED_APP_IDS.contains(&app_id)
}

/// All nixpkgs attribute names that map to the given canonical app-id, i.e. the
/// nix install variants of one application (e.g. `org.mozilla.firefox` →
/// `["firefox", "firefox-esr", …]`). Used to populate the "other sources" of an
/// app for GNOME Software's `alternate-of` query.
///
/// # Parameters
/// * `app_id` - AppStream component id.
///
/// # Returns
/// The attribute paths, or an empty slice when the id maps to no nix package.
#[cfg(feature = "app-info-gui")]
pub fn packages_for_app_id(app_id: &str) -> &'static [&'static str] {
    package_basic_info::get_packages_by_app_id(app_id)
}

/// Flatpak/AppStream display name for a nixpkgs attribute, when matched (e.g.
/// `firefox-bin` → `"Firefox"`).
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The display name, or `None` when the attribute is not in the generated table.
#[cfg(feature = "app-info-gui")]
pub fn name_for_package(pkg_name: &str) -> Option<&'static str> {
    package_basic_info::get_name(pkg_name)
}

/// Themed icon name (`meta.mainProgram`) for a nixpkgs attribute, when matched.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The icon name, or `None` when the attribute is not in the generated table.
/// Costs nothing: the name comes from the table, not from a `nix` evaluation.
#[cfg(feature = "app-info-gui")]
pub fn icon_name_for_package(pkg_name: &str) -> Option<&'static str> {
    package_basic_info::get_icon_name(pkg_name)
}

/// SPDX expression for a nixpkgs attribute's `meta.license`, or `None` when
/// the attribute has no license metadata or cannot be evaluated (broken
/// attribute, eval timeout — see `nix_eval::run_json`).
///
/// Costs one `nix eval` per attribute: this is the only source for the
/// license of a nix package, since `nix search --json` (and therefore the
/// on-disk index) does not carry `meta`.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The SPDX expression, or one of the `LicenseRef-*` refs when only the
/// free/unfree bit is known (see `core::license::normalize`); `None` when the
/// attribute declares no license or cannot be evaluated - the evaluation error
/// is swallowed rather than reported.
pub async fn license_for_package(pkg_name: &str) -> Option<String> {
    let installable = format!("nixpkgs#{pkg_name}");
    let raw: Option<license::RawLicense> =
        nix_eval::eval_json(&[&installable, "--apply", "p: p.meta.license or null"])
            .await
            .ok()?;
    license::normalize(&raw?)
}
