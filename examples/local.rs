use modulix_core_utils::mx::core::{
    option::Option, transaction::Transaction, transaction::transaction::BuildCommand,
};

fn main() {
    let mut transac_add_pkgs = Transaction::new("Set locale", BuildCommand::Switch).unwrap();
    transac_add_pkgs
        .add_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    transac_add_pkgs.begin().unwrap();

    let file = transac_add_pkgs
        .get_file("/home/quentin/Programmes/Modulix-OS/modulix-core-utils/test/configuration.nix")
        .unwrap();

    Option::new("time.timeZone")
        .set(file, "\"Europe/Paris\"")
        .unwrap();

    Option::new("i18n.defaultLocale")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_ADDRESS")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_IDENTIFICATION")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_MEASUREMENT")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_MEASUREMENT")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_MONETARY")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_NAME")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_NUMERIC")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_PAPER")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_TELEPHONE")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("i18n.extraLocaleSettings.LC_TIME")
        .set(file, "\"fr_FR.UTF-8\"")
        .unwrap();

    Option::new("console.keyMap").set(file, "\"fr\"").unwrap();

    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };

    // let mut f = fs::File::create("./test.txt").unwrap();
    // f.write("Coucou".as_bytes()).unwrap();
}
