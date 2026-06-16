use const_format::concatcp;

#[cfg(feature = "package-info")]
pub mod package_info;

#[cfg(feature = "config-store")]
mod config_store;

mod core;

#[cfg(feature = "core-app-info-trait")]
pub use core::app_info_trait::AppInfoMinimal;

#[cfg(feature = "app-info-gui")]
pub use core::app_info_trait::AppInfoGui;

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
    "https://raw.githubusercontent.com/Modulix-OS/modules/",
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
}
