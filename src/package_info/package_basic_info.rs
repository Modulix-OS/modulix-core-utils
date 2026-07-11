#[cfg(not(feature = "flathub-info-gen"))]
include!("../../flathub_basic_info.rs");

pub struct NixInfo {
    pub name: &'static str,
    pub app_id: &'static str,
    pub icon: &'static str,
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

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_icon(pkg_name: &str) -> Option<&'static str> {
    Some(NIX_INFO.get(pkg_name)?.icon)
}

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_packages_by_app_id(app_id: &str) -> Vec<&'static str> {
    NIX_INFO
        .entries()
        .filter_map(|(pkg, info)| (info.app_id == app_id).then_some(*pkg))
        .collect()
}

#[cfg(not(feature = "flathub-info-gen"))]
pub fn get_keywords(pkg_name: &str) -> Option<&'static [&'static str]> {
    Some(NIX_INFO.get(pkg_name)?.keywords)
}
