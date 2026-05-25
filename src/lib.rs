#[cfg(debug_assertions)]
use const_format::concatcp;

mod config_store;
mod core;
pub mod desktop_environment;
pub mod detect_hardware;
mod error;
pub mod filesystem;
pub mod firewall;
pub mod flake_input;
pub mod hardware_config;
pub mod init;
pub mod locale;
pub mod modulix_modules;
pub mod package;
pub mod user;

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

const CONFIG_NAME: &str = "default";

pub mod mx {
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;
    pub use crate::firewall::NetworkProtocol;
}
