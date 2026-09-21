//! Library behind every Modulix front-end (`mx-daemon`, the GNOME Software
//! plugin, the installer): it reads and edits the NixOS configuration the
//! system is built from, and answers the questions a store UI asks about
//! packages, modules and hardware.
//!
//! Each area lives in its own module, gated by a Cargo feature so a consumer
//! links only what it uses. Every write goes through
//! `core::transaction`, which makes a
//! configuration edit plus its `nixos-rebuild` one revertible unit; the read
//! side (search, listings, metadata) is built on the generated indexes under
//! [`cache_dir`].

use const_format::concatcp;

#[cfg(feature = "package-info")]
pub mod package_info;

#[cfg(feature = "package-info")]
pub mod package_index;

#[cfg(feature = "config-store")]
mod config_store;

mod core;

#[cfg(feature = "core-app-info-trait")]
pub use core::app_info_trait::AppInfoMinimal;

#[cfg(feature = "app-info-gui")]
pub use core::app_info_trait::AppInfoGui;

#[cfg(feature = "app-info-gui")]
pub use core::app_info_trait::AppScreenshot;

#[cfg(feature = "app-info-gui")]
pub use core::app_info_trait::FlatpakInfo;

#[cfg(any(feature = "package-info", feature = "app-info-gui"))]
pub use core::license;

#[cfg(feature = "desktop-environment")]
pub mod desktop_environment;

#[cfg(feature = "detect-hardware")]
pub mod detect_hardware;

#[cfg(feature = "filesystem")]
pub mod filesystem;

#[cfg(feature = "firewall")]
pub mod firewall;

#[cfg(feature = "flake-input")]
pub mod flake_input;

#[cfg(feature = "hardware-config")]
pub mod hardware_config;

#[cfg(feature = "init")]
pub mod init;

#[cfg(feature = "locale")]
pub mod locale;

#[cfg(feature = "module-info")]
pub mod module_info;

#[cfg(feature = "install-module")]
pub mod install_module;

#[cfg(feature = "modulix-module")]
pub mod modulix_modules;

#[cfg(feature = "install-package")]
pub mod install_package;

#[cfg(feature = "user")]
pub mod user;

mod error;

/// Git repository holding the system's NixOS configuration, and the default
/// working directory of every transaction.
///
/// `/etc/modulix-os/` in release builds; in debug builds the crate's own
/// `test/` fixture, so a development run never edits the live system.
#[cfg(not(debug_assertions))]
pub const CONFIG_DIRECTORY: &str = "/etc/modulix-os/";
#[cfg(debug_assertions)]
pub const CONFIG_DIRECTORY: &str = concatcp!(env!("CARGO_MANIFEST_DIR"), "/test/");

/// Name of the generated-index directory inside the config directory.
///
/// Kept dot-prefixed and listed in the repo's `.git/info/exclude` by
/// [`init::init`]: the indexes are rebuilt artifacts, and an untracked
/// directory inside the config repo would otherwise be stashed away by
/// every transaction.
pub const CACHE_DIRECTORY_NAME: &str = ".cache";

/// Directory holding the generated indexes (module index, package index).
///
/// Defaults to `CACHE_DIRECTORY_NAME` inside [`CONFIG_DIRECTORY`]. Since
/// `CONFIG_DIRECTORY` is fixed at compile time, `$MX_CACHE_DIR` overrides it
/// for deployments whose config repo lives elsewhere — notably a debug build
/// driving a real system, where the compiled-in path is the source fixture.
///
/// # Returns
/// `$MX_CACHE_DIR` when set (used as-is, even if it does not exist yet), else
/// [`CACHE_DIRECTORY_NAME`] inside [`CONFIG_DIRECTORY`]. Creating the
/// directory is the caller's job.
pub fn cache_dir() -> std::path::PathBuf {
    std::env::var_os("MX_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(CONFIG_DIRECTORY).join(CACHE_DIRECTORY_NAME))
}

/// Which kind of Git ref the remote URLs below point at.
///
/// # Variants
/// * `Branch` - a branch name, fetched from GitHub's `heads` namespace.
/// * `Tag` - a release tag, fetched from the `tags` namespace.
enum GitRefs {
    Branch,
    Tag,
}

impl GitRefs {
    /// URL path segment GitHub uses for this kind of ref.
    ///
    /// # Returns
    /// `"heads"` for a branch, `"tags"` for a tag.
    pub const fn github_path(&self) -> &'static str {
        match self {
            GitRefs::Branch => "heads",
            GitRefs::Tag => "tags",
        }
    }
}

/// Kind of ref the remote URLs track.
const REFS_FOLLOW: GitRefs = GitRefs::Branch;

/// Name of the tracked ref, i.e. the upstream revision a freshly installed or
/// freshly indexed system follows.
const REFS_FOLLOWED: &str = "master";

/// [`REFS_FOLLOW`] rendered as its GitHub URL segment.
const REFS_FOLLOW_PATH: &str = REFS_FOLLOW.github_path();

/// Raw-content base URL of the reference configuration repository.
///
/// Currently unused: [`init`] builds a new system's configuration from local
/// templates and `nixos-generate-config` rather than fetching it from here.
pub const REMOTE_CONFIG_URL: &str = concatcp!(
    "https://raw.githubusercontent.com/Modulix-OS/config/",
    REFS_FOLLOW_PATH,
    "/",
    REFS_FOLLOWED,
    "/"
);

/// Raw-content base URL of the `mxpkgs` modules tree: where
/// [`module_info`] fetches the module index and each module's metadata from,
/// rather than from the locally checked-out flake input.
pub const REMOTE_MODULE_URL: &str = concatcp!(
    "https://raw.githubusercontent.com/Modulix-OS/mxpkgs/",
    REFS_FOLLOW_PATH,
    "/",
    REFS_FOLLOWED,
    "/modules/"
);

/// `nixosConfigurations` attribute the flake in [`CONFIG_DIRECTORY`] exposes,
/// i.e. the flake output `nixos-rebuild` is pointed at.
const CONFIG_NAME: &str = "default";

/// Public re-exports of the crate's error vocabulary and of the enums a caller
/// has to name when calling into the modules, gathered so consumers write
/// `mx::Result<T>` instead of reaching into private modules.
pub mod mx {
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;

    #[cfg(feature = "firewall")]
    pub use crate::firewall::NetworkProtocol;

    #[cfg(feature = "init")]
    pub use crate::init::Desktop;
}
