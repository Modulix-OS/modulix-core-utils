use modulix_core_utils::package;

fn main() {
    println!(
        "{}",
        package::desktop_icon::get_desktop_file("firefox-bin").unwrap()
    );
    //package::install(CONFIG_DIRECTORY, "cargo").unwrap();
    // package::uninstall(CONFIG_DIRECTORY, "gcc").unwrap();
    // package::uninstall(CONFIG_DIRECTORY, "obs-studio").unwrap();
    // package::remove_plugin(CONFIG_DIRECTORY, "obs-studio", "obs-tuna").unwrap();
}
