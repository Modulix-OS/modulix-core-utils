use modulix_core_utils::package;

fn main() {
    package::install("firefox").unwrap();
    // dbg!(package::get_package_outputs("openmpi")).unwrap();
    // dbg!(package::search_packages("vim", "x86_64-linux").unwrap());
    // dbg!(package::list_plugins("vim", "x86_64-linux").unwrap());
}
