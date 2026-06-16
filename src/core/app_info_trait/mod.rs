use crate::mx;
#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;
#[cfg(feature = "app-info-gui")]
pub mod app_info_screenshot;
#[cfg(feature = "app-info-gui")]
pub use app_info_screenshot::AppScreenshot;

#[cfg(feature = "app-info-gui")]
mod flatpak;
#[cfg(feature = "app-info-gui")]
pub use flatpak::FlatpakInfo;

#[cfg(feature = "package-info")]
mod plugin_namespace;
#[cfg(feature = "package-info")]
pub use plugin_namespace::PLUGIN_NAMESPACE_PREFIXES;

#[cfg(any(feature = "package-info", feature = "module-info"))]
mod search;
#[cfg(any(feature = "package-info", feature = "module-info"))]
pub use search::score;

#[cfg(feature = "module-info")]
mod app_plugin;
#[cfg(feature = "module-info")]
pub use app_plugin::AppPlugin;

#[cfg(feature = "app-info-gui")]
type Url = str;

pub trait AppInfoMinimal: Sized {
    fn new(pkg_name: &str) -> impl Future<Output = mx::Result<Self>> + Send;
    fn search(query: &str, number_app: u32) -> impl Future<Output = mx::Result<Vec<Self>>> + Send;
    fn package_name(&self) -> &str;
    fn display_name(&self) -> &str;
    fn summary(&self) -> &str;
}

#[cfg(feature = "app-info-gui")]
pub trait AppInfoGui {
    fn id(&self) -> Option<&str>;
    fn icon(&self) -> Option<&Url>;
    fn keyword(&self) -> Vec<&str>;
    fn description(&self) -> impl Future<Output = Cow<'_, str>> + Send;
    fn screenshots(&self) -> impl Future<Output = Option<AppScreenshot<'_>>> + Send;
    fn main_program(&self) -> impl Future<Output = mx::Result<Cow<'_, str>>> + Send;
}
