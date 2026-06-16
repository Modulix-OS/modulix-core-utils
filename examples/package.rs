use modulix_core_utils::{CONFIG_DIRECTORY, install_package};

fn main() {
    println!(
        "{:#?}",
        install_package::list_installed_package(CONFIG_DIRECTORY).unwrap()
    );

    install_package::install(CONFIG_DIRECTORY, &["cargo", "gcc", "obs-studio"]).unwrap();
    println!(
        "{:#?}",
        install_package::list_installed_package(CONFIG_DIRECTORY).unwrap()
    );
}
