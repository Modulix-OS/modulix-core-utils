use modulix_core_utils::edit_option::{set_option, set_option_to_default};


fn main() {
    set_option("./test.nix", "test.ni.enable", "./nix/temp").unwrap();
    set_option_to_default("./test.nix", "test.ni.enable").unwrap();
}
