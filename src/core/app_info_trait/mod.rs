use crate::mx;
#[cfg(feature = "app-info-gui")]
use std::borrow::Cow;
#[cfg(feature = "app-info-gui")]
pub mod app_info_screenshot;
#[cfg(feature = "app-info-gui")]
pub use app_info_screenshot::AppScreenshot;

mod plugin_namespace;
pub use plugin_namespace::PLUGIN_NAMESPACES;

mod app_plugin;
pub use app_plugin::AppPlugin;

type Url = str;

pub trait AppInfoMinimal: Sized {
    fn new(pkg_name: &str) -> impl Future<Output = mx::Result<Self>> + Send;
    fn search(query: &str, number_app: u32) -> impl Future<Output = mx::Result<Vec<Self>>> + Send;
    fn package_name(&self) -> &str;
    fn display_name(&self) -> &str;
    fn summary(&self) -> &str;
    fn list_plugins(package: &str) -> impl Future<Output = mx::Result<Vec<AppPlugin>>> + Send;
}

#[cfg(feature = "app-info-gui")]
pub trait AppInfoGui {
    fn id(&self) -> Option<&str>;
    fn icon(&self) -> Option<&Url>;
    fn keyword(&self) -> Option<&[&str]>;
    fn description(&self) -> impl Future<Output = Cow<'_, str>> + Send;
    fn screenshots(&self) -> impl Future<Output = Option<AppScreenshot<'_>>> + Send;
    fn main_program(&self) -> impl Future<Output = mx::Result<Cow<'_, str>>> + Send;
}
