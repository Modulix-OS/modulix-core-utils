#[cfg(not(feature = "flathub-info-gen"))]
include!("../../flathub_basic_info.rs");

pub struct NixInfo {
    pub name: &'static str,
    pub app_id: &'static str,
    pub icon_name: &'static str,
    pub keywords: &'static [&'static str],
}

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_app_id(pkg_name: &str) -> Option<&'static str> {
    Some(NIX_INFO.get(pkg_name)?.app_id)
}

/// Flatpak/AppStream display name for a nix attribute, when matched.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_name(pkg_name: &str) -> Option<&'static str> {
    let name = NIX_INFO.get(pkg_name)?.name;
    (!name.is_empty()).then_some(name)
}

/// Themed icon name (`meta.mainProgram`) for a nix attribute, when matched.
#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_icon_name(pkg_name: &str) -> Option<&'static str> {
    let icon_name = NIX_INFO.get(pkg_name)?.icon_name;
    (!icon_name.is_empty()).then_some(icon_name)
}

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_packages_by_app_id(app_id: &str) -> &'static [&'static str] {
    APP_ID_TO_PACKAGES.get(app_id).copied().unwrap_or(&[])
}

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_keywords(pkg_name: &str) -> Option<&'static [&'static str]> {
    Some(NIX_INFO.get(pkg_name)?.keywords)
}
