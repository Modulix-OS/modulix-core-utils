use crate::{
    core::transaction,
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const USER_FILE_PATH: &str = "users.nix";

pub fn add_no_transaction(
    file: &mut NixFile,
    username: &str,
    initial_password: &str,
    description: &str,
    shell: &str,
    extra_groups: &[&str],
    is_normal_user: bool,
) -> mx::Result<()> {
    let root_option = format!("users.users.{}", username);

    mxOption::new(&format!("{}.isNormalUser", root_option))
        .set(file, if is_normal_user { "true" } else { "false" })?;
    mxOption::new(&format!("{}.initialPassword", root_option))
        .set(file, &format!("\"{}\"", initial_password))?;
    mxOption::new(&format!("{}.createHome", root_option)).set(file, "true")?;
    mxOption::new(&format!("{}.group", root_option)).set(file, "\"users\"")?;
    mxOption::new(&format!("{}.description", root_option))
        .set(file, &format!("\'\'{}\'\'", description))?;
    mxOption::new(&format!("{}.shell", root_option)).set(file, &format!("\"{}\"", shell))?;

    let extra_group_name = &format!("{}.extraGroups", root_option);
    let extra_groups_list = mxList::new(extra_group_name, true);
    for group in extra_groups {
        extra_groups_list.add(file, &format!("\"{}\"", group))?;
    }

    Ok(())
}

pub fn remove_no_transaction(file: &mut NixFile, username: &str) -> mx::Result<bool> {
    let root_option = format!("users.users.{}", username);
    mxOption::new(&root_option).set_option_all_instance_to_default(file)
}

pub fn add(
    config_dir: &str,
    username: &str,
    initial_password: &str,
    description: &str,
    shell: &str,
    extra_groups: &[&str],
    is_normal_user: bool,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add user {}", username),
        config_dir,
        USER_FILE_PATH,
        BuildCommand::Switch,
        |file| {
            add_no_transaction(
                file,
                username,
                initial_password,
                description,
                shell,
                extra_groups,
                is_normal_user,
            )
        },
    )
}

pub fn remove(config_dir: &str, username: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("Remove user {}", username),
        config_dir,
        USER_FILE_PATH,
        BuildCommand::Switch,
        |file| remove_no_transaction(file, username),
    )
}
