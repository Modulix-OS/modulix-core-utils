use std::fs;

use modulix_core_utils::edit_option::{set_option, set_option_to_default};
use modulix_core_utils::edit_list::{add_in_list, remove_in_list};
use modulix_core_utils::filesystem::edit_filesystem::filesystem_add_entry;
use rnix::Root;

fn main() {
    // let file_content = fs::read_to_string("./test.nix").unwrap();
//
// let ast = Root::parse(&file_content);
//
// println!("{:#?}", ast.syntax());
    set_option_to_default("./test.nix", "test.\"nixos\".nix").unwrap();
    filesystem_add_entry(
        "/mnt/Games",
        "/dev/disk/by-uuid/1b35568b-4447-4c80-9880-4b359d4ecb6c",
        "ext4",
        &vec!["noatime", "nodiratime", "discard", "defaults", "commit=120"]
    );

    set_option("./test.nix", "test.ni.enable", "./nix/temp").unwrap();
    set_option_to_default("./test.nix", "test.nix.enable").unwrap();
    add_in_list("./test.nix", "environment.test.systemPackages", "pkgs.firefox", true).unwrap();
    //remove_in_list("./test.nix", "environment.systemPackages", "pkgs.firefox").unwrap();
    add_in_list("./test.nix", "environment.systemPackages", "pkgs.nautilus", true).unwrap();
    set_option("./test.nix", "programs.steam.enable", "true").unwrap();
    set_option("./test.nix", "test.nixos.auto-update", "true").unwrap();

}
