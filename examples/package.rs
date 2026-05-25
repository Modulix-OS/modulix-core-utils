use modulix_core_utils::{CONFIG_DIRECTORY, install_package};

fn main() {
    println!(
        "{:#?}",
        install_package::list_installed_package(CONFIG_DIRECTORY).unwrap()
    );

    install_package::install(CONFIG_DIRECTORY, "cargo").unwrap();
    install_package::install(CONFIG_DIRECTORY, "gcc").unwrap();
    install_package::install(CONFIG_DIRECTORY, "obs-studio").unwrap();
    install_package::install_plugin(CONFIG_DIRECTORY, "obs-studio", "obs-tuna").unwrap();
    println!(
        "{:#?}",
        install_package::list_installed_package(CONFIG_DIRECTORY).unwrap()
    );
}
