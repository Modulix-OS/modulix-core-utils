use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{Transaction, transaction::BuildCommand},
    },
    mx,
};

const FILE_SYSTEM_PATH: &str = "fstab.nix";

pub fn add_entry(
    mount_point: &str,
    device: &str,
    fs_type: &str,
    option: &[&str],
    encrypted: bool,
) -> mx::Result<()> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);

    let mut filesystem_transaction = Transaction::new(
        &format!("Add {} entry with device: {} in fstab", mount_point, device),
        BuildCommand::Build,
    )?;
    filesystem_transaction.add_file(FILE_SYSTEM_PATH)?;
    filesystem_transaction.begin()?;

    let fstab = filesystem_transaction.get_file(FILE_SYSTEM_PATH)?;

    if encrypted {
        let uuid = device
            .strip_prefix("/dev/disk/by-uuid/")
            .ok_or(mx::ErrorKind::InvalidUuid)?;
        let luks_name = format!("luks-{}", uuid);
        let luks_path = format!("/dev/mapper/{}", luks_name);
        let luks_option = format!("boot.initrd.luks.devices.\"{}\"", luks_name);
        mxOption::new(&format!("{}.device", luks_option))
            .set(fstab, format!("\"{}\"", device).as_str())?;

        mxOption::new(format!("{}.device", root_option).as_str())
            .set(fstab, format!("\"{}\"", luks_path).as_str())?;
    } else {
        mxOption::new(format!("{}.device", root_option).as_str())
            .set(fstab, format!("\"{}\"", device).as_str())?;
    }

    mxOption::new(format!("{}.fsType", root_option).as_str())
        .set(fstab, format!("\"{}\"", fs_type).as_str())?;

    let option_path = format!("{}.options", root_option);

    mxOption::new(&option_path).set_option_to_default(fstab)?;

    let list_opt = mxList::new(&option_path, true);
    for o in option {
        list_opt.add(fstab, &format!("\"{}\"", o))?;
    }

    filesystem_transaction.commit()?;
    Ok(())
}

pub fn remove_entry(mount_point: &str) -> mx::Result<bool> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);

    let mut filesystem_transaction = Transaction::new(
        &format!("remove {} entry in fstab", mount_point),
        BuildCommand::Build,
    )?;

    filesystem_transaction.add_file(FILE_SYSTEM_PATH)?;
    filesystem_transaction.begin()?;

    let fstab = filesystem_transaction.get_file(FILE_SYSTEM_PATH)?;

    let found = mxOption::new(&format!("{}.device", root_option)).set_option_to_default(fstab)?;

    if found {
        mxOption::new(format!("{}.fsType", root_option).as_str()).set_option_to_default(fstab)?;
        mxOption::new(&format!("{}.options", root_option)).set_option_to_default(fstab)?;
    }

    filesystem_transaction.commit()?;
    Ok(found)
}

pub fn add_swap(device: &str) -> mx::Result<()> {
    let mut transaction_swap =
        Transaction::new(&format!("Add swap device: {}", device), BuildCommand::Build)?;

    transaction_swap.add_file(FILE_SYSTEM_PATH)?;
    transaction_swap.begin()?;

    let fstab = transaction_swap.get_file(FILE_SYSTEM_PATH)?;

    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.add(fstab, &new_entry)?;

    transaction_swap.commit()?;
    Ok(())
}

pub fn remove_swap(device: &str) -> mx::Result<()> {
    let mut transaction_swap = Transaction::new(
        &format!("Remove swap device: {}", device),
        BuildCommand::Build,
    )?;

    transaction_swap.add_file(FILE_SYSTEM_PATH)?;
    transaction_swap.begin()?;

    let fstab = transaction_swap.get_file(FILE_SYSTEM_PATH)?;

    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.remove(fstab, &new_entry)?;

    transaction_swap.commit()?;
    Ok(())
}
