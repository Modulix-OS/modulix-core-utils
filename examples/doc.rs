use modulix_core_utils::mx::core::{
    option::Option, transaction::Transaction, transaction::transaction::BuildCommand,
};

fn main() {
    let mut transac_add_pkgs = Transaction::new("Enable NixOS Doc", BuildCommand::Switch).unwrap();
    transac_add_pkgs
        .add_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    transac_add_pkgs.begin().unwrap();

    let file = transac_add_pkgs
        .get_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();
    let doc = Option::new("documentation.nixos.enable");
    doc.set(file, "true").unwrap();
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };

    // let mut f = fs::File::create("./test.txt").unwrap();
    // f.write("Coucou".as_bytes()).unwrap();
}
