//! The traits every app-metadata source implements, so packages, modules and
//! Flathub entries can be searched and displayed through one shape.
//!
//! [`AppInfoMinimal`] is what a listing needs (name, summary, search);
//! [`AppInfoGui`] adds what a store UI shows (icon, description, screenshots)
//! and is only compiled for the GUI consumers.

use crate::mx;
#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;
#[cfg(feature = "app-info-gui")]
pub mod app_info_screenshot;
#[cfg(feature = "app-info-gui")]
pub use app_info_screenshot::AppScreenshot;

#[cfg(feature = "app-info-gui")]
mod flatpak;
#[cfg(feature = "app-info-gui")]
pub use flatpak::FlatpakInfo;

#[cfg(feature = "package-info")]
mod plugin_namespace;
#[cfg(feature = "package-info")]
pub use plugin_namespace::PLUGIN_NAMESPACE_PREFIXES;

#[cfg(any(feature = "package-info", feature = "module-info"))]
mod search;
#[cfg(any(feature = "package-info", feature = "module-info"))]
pub use search::score;

#[cfg(feature = "module-info")]
mod app_plugin;
#[cfg(feature = "module-info")]
pub use app_plugin::AppPlugin;

/// An absolute URL, kept as a distinct name so a signature says what the string
/// is meant to hold.
#[cfg(feature = "app-info-gui")]
type Url = str;

/// What every app-metadata source must provide: the identity and the search a
/// listing needs.
pub trait AppInfoMinimal: Sized {
    /// Loads the entry of one package by name.
    ///
    /// # Parameters
    /// * `pkg_name` - the source's own key: a nixpkgs attribute path for
    ///   packages, a module key for modules.
    ///
    /// # Returns
    /// The entry.
    ///
    /// # Errors
    /// [`mx::ErrorKind::PackageNotFound`] for an unknown key, plus whatever the
    /// source needs to reach it (index read, HTTP request, `nix` evaluation).
    fn new(pkg_name: &str) -> impl Future<Output = mx::Result<Self>> + Send;

    /// Searches the source, keeping each hit's relevance.
    ///
    /// # Parameters
    /// * `query` - free-text search terms.
    /// * `number_app` - upper bound on the number of hits returned.
    ///
    /// # Returns
    /// `(score, entry)` pairs, highest score first, at most `number_app` of
    /// them; entries scoring 0 are left out, so the result can be empty.
    fn search_scored(
        query: &str,
        number_app: u32,
    ) -> impl Future<Output = mx::Result<Vec<(u32, Self)>>> + Send;

    /// Searches the source, dropping the scores.
    ///
    /// # Parameters
    /// * `query` - free-text search terms.
    /// * `number_app` - upper bound on the number of hits returned.
    ///
    /// # Returns
    /// The entries of [`AppInfoMinimal::search_scored`], still best match
    /// first.
    fn search(query: &str, number_app: u32) -> impl Future<Output = mx::Result<Vec<Self>>> + Send
    where
        Self: Sized,
    {
        async move {
            Ok(Self::search_scored(query, number_app)
                .await?
                .into_iter()
                .map(|(_, item)| item)
                .collect())
        }
    }
    /// The entry's key in its own source.
    ///
    /// # Returns
    /// The key [`AppInfoMinimal::new`] takes, borrowed from `self`: what an
    /// install request must carry.
    fn package_name(&self) -> &str;

    /// The name to show a user.
    ///
    /// # Returns
    /// The human-readable name, borrowed from `self`; it falls back to the
    /// package name when the source has nothing better.
    fn display_name(&self) -> &str;

    /// The one-line description.
    ///
    /// # Returns
    /// The summary, borrowed from `self`; empty when the source has none.
    fn summary(&self) -> &str;
}

/// What a store UI additionally needs from a metadata source: identity as
/// AppStream sees it, artwork, and the long description.
///
/// The async methods are the ones whose data may not be local: they can trigger
/// a Flathub request or a `nix` evaluation on first call.
#[cfg(feature = "app-info-gui")]
pub trait AppInfoGui {
    /// The entry's AppStream component id.
    ///
    /// # Returns
    /// The id (e.g. `org.gnome.Calculator`), or `None` when the source could not
    /// map the package to one.
    fn id(&self) -> Option<&str>;

    /// The application's own name, as its AppStream metadata spells it.
    ///
    /// # Returns
    /// The name, or `None` when unknown - the caller then falls back to
    /// [`AppInfoMinimal::display_name`].
    fn app_name(&self) -> Option<&str>;

    /// URL of the application's icon.
    ///
    /// # Returns
    /// The URL to download, or `None` when the source has no remote icon; a
    /// locally themed icon may still be named by
    /// [`AppInfoGui::icon_name`].
    fn icon(&self) -> Option<&Url>;

    /// Name of the icon to look up in the user's icon theme, preferred over
    /// downloading [`AppInfoGui::icon`] when both are available.
    ///
    /// # Returns
    /// The theme icon name, or `None` - which is what the default
    /// implementation returns for sources that do not know one.
    fn icon_name(&self) -> Option<&str> {
        None
    }

    /// Extra search terms the entry should also match.
    ///
    /// # Returns
    /// The keywords, borrowed from `self`; empty when the source has none.
    fn keyword(&self) -> Vec<&str>;

    /// The long, multi-paragraph description.
    ///
    /// # Returns
    /// The description, borrowed when the source already holds it and owned when
    /// it had to be fetched or rendered. Empty when there is none: a missing
    /// description is not an error.
    fn description(&self) -> impl Future<Output = Cow<'_, str>> + Send;

    /// The entry's screenshots.
    ///
    /// # Returns
    /// The screenshot set, each in its available sizes, or `None` when the source
    /// has none or could not be reached.
    fn screenshots(&self) -> impl Future<Output = Option<AppScreenshot<'_>>> + Send;

    /// Name of the binary the entry is launched with.
    ///
    /// # Returns
    /// The program name, borrowed or owned depending on the source.
    ///
    /// # Errors
    /// Whatever resolving it needs: typically a `nix` evaluation of the
    /// package's `meta.mainProgram`.
    fn main_program(&self) -> impl Future<Output = mx::Result<Cow<'_, str>>> + Send;
}
