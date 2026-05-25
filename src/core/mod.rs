#[cfg(feature = "core-arch-info")]
pub mod arch;

#[cfg(feature = "core-nix-file")]
pub mod list;

#[cfg(feature = "core-nix-file")]
mod localise_option;

#[cfg(feature = "core-nix-file")]
pub mod option;

#[cfg(feature = "core-nix-file")]
pub mod transaction;

#[cfg(feature = "core-nix-file")]
pub mod param;

#[cfg(feature = "core-user-info")]
pub mod user;

#[cfg(feature = "core-nix-plugin-namespace")]
pub mod plugin_namespace;

//pub mod utils;
#[cfg(feature = "core-nix-file")]
pub const TABULATION_SIZE: usize = 2;
