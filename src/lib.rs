#[cfg(debug_assertions)]
use const_format::concatcp;

mod core;
mod error;
pub mod filesystem;
pub mod firewall;
pub mod init;
pub mod locale;
pub mod modulix_modules;
pub mod package;
pub mod user;

#[cfg(not(debug_assertions))]
const CONFIG_DIRECTORY: &str = "/etc/modulix-os/";
#[cfg(debug_assertions)]
const CONFIG_DIRECTORY: &str = concatcp!(env!("CARGO_MANIFEST_DIR"), "/test/");

const CONFIG_NAME: &str = "default";

pub mod mx {
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;
    pub use crate::firewall::NetworkProtocol;
}
