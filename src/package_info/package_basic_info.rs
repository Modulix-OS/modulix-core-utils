//! The compiled-in lookup table linking nixpkgs attributes to their AppStream
//! identity.
//!
//! The table itself is generated - `cargo run --bin flathub-info-gen` writes
//! `flathub_basic_info.rs`, which is included here - so these lookups are
//! constant-time and need neither network nor index. The `flathub-info-gen`
//! feature compiles the module without the table, since the generator is what
//! produces it.

#[cfg(not(feature = "flathub-info-gen"))]
include!("../../flathub_basic_info.rs");

/// One row of the generated table: what is known about a nixpkgs attribute
/// without evaluating anything.
///
/// # Fields
/// * `name` - AppStream display name; empty when unknown.
/// * `app_id` - AppStream component id.
/// * `icon_name` - themed icon name, from the package's `meta.mainProgram`;
///   empty when unknown.
/// * `keywords` - extra search terms for the scoring.
pub struct NixInfo {
    pub name: &'static str,
    pub app_id: &'static str,
    pub icon_name: &'static str,
    pub keywords: &'static [&'static str],
}

/// AppStream component id of a nixpkgs attribute.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The id, or `None` when the attribute is not in the generated table - which is
/// the case for everything that is not a desktop application.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_app_id(pkg_name: &str) -> Option<&'static str> {
    Some(NIX_INFO.get(pkg_name)?.app_id)
}

/// Flatpak/AppStream display name for a nix attribute, when matched.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The display name, or `None` when the attribute is absent from the table or
/// its row carries no name.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_name(pkg_name: &str) -> Option<&'static str> {
    let name = NIX_INFO.get(pkg_name)?.name;
    (!name.is_empty()).then_some(name)
}

/// Themed icon name (`meta.mainProgram`) for a nix attribute, when matched.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// The icon name to look up in the user's icon theme, or `None` when the
/// attribute is absent from the table or its row carries no icon name.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_icon_name(pkg_name: &str) -> Option<&'static str> {
    let icon_name = NIX_INFO.get(pkg_name)?.icon_name;
    (!icon_name.is_empty()).then_some(icon_name)
}

/// The nixpkgs attributes that provide a given application.
///
/// # Parameters
/// * `app_id` - AppStream component id.
///
/// # Returns
/// The attribute paths mapped to that id - several when nixpkgs packages the app
/// more than once - or an empty slice when none is. This is the reverse of
/// [`get_app_id`].
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_packages_by_app_id(app_id: &str) -> &'static [&'static str] {
    APP_ID_TO_PACKAGES.get(app_id).copied().unwrap_or(&[])
}

/// Extra search terms of a nixpkgs attribute.
///
/// # Parameters
/// * `pkg_name` - the nixpkgs attribute path.
///
/// # Returns
/// Its keywords, which the search scoring weighs heavily, or `None` when the
/// attribute is absent from the table. A row with no keyword yields an empty
/// slice rather than `None`.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_keywords(pkg_name: &str) -> Option<&'static [&'static str]> {
    Some(NIX_INFO.get(pkg_name)?.keywords)
}
