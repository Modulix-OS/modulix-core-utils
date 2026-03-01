use modulix_core_utils::package;

fn main() {
    package::uninstall("firefox").unwrap();
}
