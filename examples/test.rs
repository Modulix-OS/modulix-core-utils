use modulix_core_utils::transaction::{Transaction, transaction::BuildCommand};

fn main() {
    // let file_content = fs::read_to_string("./test.nix").unwrap();
    //
    // let ast = Root::parse(&file_content);
    //
    // println!("{:#?}", ast.syntax());
    // set_option_to_default("./test.nix", "test.\"nixos\".nix").unwrap();

    // set_option("./test.nix", "test.ni.enable", "./nix/temp").unwrap();
    // set_option_to_default("./test.nix", "test.nix.enable").unwrap();
    // add_in_list("./test.nix", "environment.test.systemPackages", "pkgs.firefox", true).unwrap();
    // add_in_list("./test.nix", "environment.test.systemPackages", "pkgs.baobab", true).unwrap();
    // remove_in_list("./test.nix", "environment.test.systemPackages", "pkgs.firefox").unwrap();
    // add_in_list("./test.nix", "environment.systemPackages", "pkgs.nautilus", true).unwrap();
    // set_option("./test.nix", "programs.steam.enable", "true").unwrap();
    // set_option("./test.nix", "test.nixos.auto-update", "true").unwrap();
    //
    let t = Transaction::new("Install pkgs", BuildCommand::Switch).unwrap();
}
