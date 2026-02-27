use modulix_core_utils::mx::core::{
    list::List, option::Option, transaction::Transaction, transaction::transaction::BuildCommand,
};

fn main() {
    let mut transac_add_pkgs = Transaction::new("Create qhor User", BuildCommand::Switch).unwrap();
    transac_add_pkgs
        .add_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    transac_add_pkgs.begin().unwrap();

    let file = transac_add_pkgs
        .get_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    let user = Option::new("users.users.qhor.isNormalUser");
    user.set(file, "true").unwrap();

    let user_password = Option::new("users.users.qhor.initialPassword");
    user_password.set(file, "\"1234\"").unwrap();

    let user_home = Option::new("users.users.qhor.createHome");
    user_home.set(file, "true").unwrap();

    let user_home = Option::new("users.users.qhor.createHome");
    user_home.set(file, "true").unwrap();

    let user_group = Option::new("users.users.qhor.group");
    user_group.set(file, "\"users\"").unwrap();

    let extra_group = List::new("users.users.qhor.extraGroups", true);
    extra_group.add(file, "\"wheel\"").unwrap();

    let user_descrition = Option::new("users.users.qhor.description");
    user_descrition.set(file, "\'\'Main user\'\'").unwrap();

    let user_shell = Option::new("users.users.qhor.shell");
    user_shell.set(file, "\"${pkgs.bash}/bin/bash\"").unwrap();

    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };

    // let mut f = fs::File::create("./test.txt").unwrap();
    // f.write("Coucou".as_bytes()).unwrap();
}
