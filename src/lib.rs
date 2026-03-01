#[cfg(debug_assertions)]
use const_format::concatcp;

pub mod core; // TODO: Swap to private
mod error;
pub mod filesystem;

#[cfg(not(debug_assertions))]
const CONFIG_DIRECTORY: &str = "/etc/nixos/";
#[cfg(debug_assertions)]
const CONFIG_DIRECTORY: &str = concatcp!(env!("CARGO_MANIFEST_DIR"), "/test/");

const CONFIG_NAME: &str = "default";

pub mod mx {
    pub use crate::core;
    pub use crate::error::ErrorKind;
    pub use crate::error::Result;
}
