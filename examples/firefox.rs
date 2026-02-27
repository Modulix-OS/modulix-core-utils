use modulix_core_utils::mx::core::{
    list::List, transaction::Transaction, transaction::transaction::BuildCommand,
};

fn main() {
    let mut transac_add_pkgs = Transaction::new("Install firefox", BuildCommand::Switch).unwrap();
    transac_add_pkgs
        .add_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    transac_add_pkgs.begin().unwrap();

    let file = transac_add_pkgs
        .get_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();
    let pkgs = List::new("environment.systemPackages", true);
    pkgs.add(file, "pkgs.firefox").unwrap();
    pkgs.add(file, "pkgs.firefoxpwa").unwrap();

    //let mut pkgs2 = List::new("systemPackages", true).unwrap();
    //pkgs2.add(file, "pkgs.firefox").unwrap();
    //pkgs2.add(file, "pkgs.firefoxpwa").unwrap();
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };

    // let mut f = fs::File::create("./test.txt").unwrap();
    // f.write("Coucou".as_bytes()).unwrap();
}
