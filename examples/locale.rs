use modulix_core_utils::locale;

fn main() {
    locale::set_locale("Europe/Paris", "fr_FR.UTF-8", "fr").unwrap();
}
