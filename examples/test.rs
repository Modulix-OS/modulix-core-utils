use modulix_core_utils::edit_option::{set_option, set_option_to_default};
use modulix_core_utils::edit_list::{add_in_list, remove_in_list};
use rnix::Root;

fn main() {
    // let file_content = fs::read_to_string("./test.nix").unwrap();
//
// let ast = Root::parse(&file_content);
//
// println!("{:#?}", ast.syntax());
    set_option_to_default("./test.nix", "test.\"nixos\".nix").unwrap();

    set_option("./test.nix", "test.ni.enable", "./nix/temp").unwrap();
    set_option_to_default("./test.nix", "test.nix.enable").unwrap();
    add_in_list("./test.nix", "environment.test.systemPackages", "pkgs.firefox", true).unwrap();
    add_in_list("./test.nix", "environment.test.systemPackages", "pkgs.baobab", true).unwrap();
    remove_in_list("./test.nix", "environment.test.systemPackages", "pkgs.firefox").unwrap();
    add_in_list("./test.nix", "environment.systemPackages", "pkgs.nautilus", true).unwrap();
    set_option("./test.nix", "programs.steam.enable", "true").unwrap();
    set_option("./test.nix", "test.nixos.auto-update", "true").unwrap();

}
