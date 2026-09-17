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
pub fn cache_dir() -> std::path::PathBuf {
    std::env::var_os("MX_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(CONFIG_DIRECTORY).join(CACHE_DIRECTORY_NAME))
}

enum GitRefs {
    Branch,
    Tag,
}

impl GitRefs {
    pub const fn github_path(&self) -> &'static str {
        match self {
            GitRefs::Branch => "heads",
            GitRefs::Tag => "tags",
        }
    }
}

const REFS_FOLLOW: GitRefs = GitRefs::Branch;
const REFS_FOLLOWED: &str = "master";
const REFS_FOLLOW_PATH: &str = REFS_FOLLOW.github_path();

pub const REMOTE_CONFIG_URL: &str = concatcp!(
    "https://raw.githubusercontent.com/Modulix-OS/config/",
    REFS_FOLLOW_PATH,
    "/",
    REFS_FOLLOWED,
    "/"
);

pub const REMOTE_MODULE_URL: &str = concatcp!(
    "https://raw.githubusercontent.com/Modulix-OS/mxpkgs/",
    REFS_FOLLOW_PATH,
    "/",
    REFS_FOLLOWED,
    "/modules/"
);

const CONFIG_NAME: &str = "default";

pub mod mx {
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;

    #[cfg(feature = "firewall")]
    pub use crate::firewall::NetworkProtocol;

    #[cfg(feature = "init")]
    pub use crate::init::Desktop;
}
