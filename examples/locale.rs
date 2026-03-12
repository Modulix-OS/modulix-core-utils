use modulix_core_utils::{CONFIG_DIRECTORY, locale};

fn main() {
    locale::set_locale(CONFIG_DIRECTORY, "Europe/Paris", "fr_FR.UTF-8", "fr").unwrap();
}
