use modulix_core_utils::edit_option::set_option;


fn main() {
    set_option("./test.nix", "test.ni.enable", "./nix/temp").unwrap();
}
