use phf::phf_map;

pub struct PluginNamespace {
    pub path_plugin: &'static str,
    pub path_enable_programs: &'static str,
    pub path_plugin_list: &'static str,
}

impl PluginNamespace {
    pub const fn new(
        path_plugin: &'static str,
        path_enable_programs: &'static str,
        path_plugin_list: &'static str,
    ) -> Self {
        Self {
            path_plugin,
            path_enable_programs,
            path_plugin_list,
        }
    }
}

pub static PLUGIN_NAMESPACES: phf::Map<&'static str, PluginNamespace> = phf_map! {
    "vscode" => PluginNamespace::new(
        "vscode-extensions",
        "programs.vscode.enable",
        "programs.vscode.extensions"),
    "obs-studio" => PluginNamespace::new(
        "obs-studio-plugins",
        "programs.obs-studio.enable",
        "programs.obs-studio.plugins"),
};
