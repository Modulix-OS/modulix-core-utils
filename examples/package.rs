use modulix_core_utils::package;

fn main() {
    // dbg!(package::get_package_metadata("firefox-bin").unwrap());
    // package::install("firefox-bin").unwrap();
    package::install("cargo").unwrap();
    //package::uninstall("gcc").unwrap();
    // package::uninstall("obs-studio").unwrap();
    // package::remove_plugin("obs-studio", "obs-tuna").unwrap();
    //dbg!(package::list_installed_package().unwrap());
    // dbg!(package::get_package_outputs("openmpi")).unwrap();
    // dbg!(package::search_packages("vim", "x86_64-linux").unwrap());
    // dbg!(package::list_plugins("vim", "x86_64-linux").unwrap());
}
