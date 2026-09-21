//! Building blocks the public modules are written against: Nix file editing
//! (options, lists, parameters, transactions), `nix` evaluation, the shared
//! HTTP client, and the app-metadata traits.

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

#[cfg(feature = "core-nix-eval")]
pub mod nix_eval;

#[cfg(feature = "reqwest")]
pub mod http_client;

#[cfg(feature = "core-user-info")]
pub mod user;

#[cfg(feature = "core-app-info-trait")]
pub mod app_info_trait;

#[cfg(any(feature = "package-info", feature = "app-info-gui"))]
pub mod license;

#[cfg(any(feature = "module-info", feature = "app-info-gui"))]
pub mod lang;

/// Indentation width, in spaces, of every configuration snippet this crate
/// writes.
#[cfg(feature = "core-nix-file")]
pub const TABULATION_SIZE: usize = 2;
