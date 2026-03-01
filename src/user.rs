use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{Transaction, transaction::BuildCommand},
    },
    mx,
};

const USER_FILE_PATH: &str = "users.nix";

pub fn add(
    username: &str,
    initial_password: &str,
    description: &str,
    shell: &str,
    extra_groups: &[&str],
    is_normal_user: bool,
) -> mx::Result<()> {
    let root_option = format!("users.users.{}", username);

    let mut transaction =
        Transaction::new(&format!("Add user {}", username), BuildCommand::Switch)?;

    transaction.add_file(USER_FILE_PATH)?;
    transaction.begin()?;

    let file = match transaction.get_file(USER_FILE_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.isNormalUser", root_option))
        .set(file, if is_normal_user { "true" } else { "false" })
    {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.initialPassword", root_option))
        .set(file, &format!("\"{}\"", initial_password))
    {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.createHome", root_option)).set(file, "true") {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.group", root_option)).set(file, "\"users\"") {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.description", root_option))
        .set(file, &format!("\'\'{}\'\'", description))
    {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    match mxOption::new(&format!("{}.shell", root_option)).set(file, &format!("\"{}\"", shell)) {
        Ok(()) => (),
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    let extra_group = format!("{}.extraGroups", root_option);
    let extra_groups_list = mxList::new(&extra_group, true);
    for group in extra_groups {
        match extra_groups_list.add(file, &format!("\"{}\"", group)) {
            Ok(()) => (),
            Err(e) => {
                transaction.rollback()?;
                return Err(e);
            }
        };
    }

    transaction.commit()?;
    Ok(())
}

pub fn remove(username: &str) -> mx::Result<bool> {
    let root_option = format!("users.users.{}", username);

    let mut transaction =
        Transaction::new(&format!("Remove user {}", username), BuildCommand::Switch)?;

    transaction.add_file(USER_FILE_PATH)?;
    transaction.begin()?;

    let file = match transaction.get_file(USER_FILE_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    let found = match mxOption::new(&root_option).set_option_all_instance_to_default(file) {
        Ok(f) => f,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

    transaction.commit()?;
    Ok(found)
}
