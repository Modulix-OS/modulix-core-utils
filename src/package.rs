use crate::{
    core::{
        list::List as mxList,
        transaction::{Transaction, transaction::BuildCommand},
    },
    mx,
};

pub fn install(package_name: &str) -> mx::Result<()> {
    let mut transac_add_pkgs =
        Transaction::new(&format!("Install {}", package_name), BuildCommand::Switch)?;
    transac_add_pkgs.add_file("package.nix")?;

    transac_add_pkgs.begin()?;

    let file = match transac_add_pkgs.get_file("package.nix") {
        Ok(f) => f,
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    let pkgs = mxList::new("environment.systemPackages", true);
    match pkgs.add(file, &format!("pkgs.{}", package_name)) {
        Ok(()) => (),
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };
    Ok(())
}

pub fn uninstall(package_name: &str) -> mx::Result<()> {
    let mut transac_add_pkgs =
        Transaction::new(&format!("Uninstall {}", package_name), BuildCommand::Switch)?;
    transac_add_pkgs.add_file("package.nix")?;

    transac_add_pkgs.begin()?;

    let file = match transac_add_pkgs.get_file("package.nix") {
        Ok(f) => f,
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    let pkgs = mxList::new("environment.systemPackages", true);
    match pkgs.remove(file, &format!("pkgs.{}", package_name)) {
        Ok(()) => (),
        Err(e) => {
            transac_add_pkgs.rollback()?;
            return Err(e);
        }
    };
    match transac_add_pkgs.commit() {
        Ok(_) => (),
        Err(e) => println!("{}", e.to_string()),
    };
    Ok(())
}
