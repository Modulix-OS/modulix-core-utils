use modulix_core_utils::{CONFIG_DIRECTORY, package};

fn main() {
    package::install(CONFIG_DIRECTORY, "cargo").unwrap();
    // package::uninstall(CONFIG_DIRECTORY, "gcc").unwrap();
    // package::uninstall(CONFIG_DIRECTORY, "obs-studio").unwrap();
    // package::remove_plugin(CONFIG_DIRECTORY, "obs-studio", "obs-tuna").unwrap();
}
