use std::process;

use crate::{
    CONFIG_DIRECTORY,
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
        CONFIG_DIRECTORY,
        &format!("Add {} entry with device: {} in fstab", mount_point, device),
        BuildCommand::Boot,
    )?;
    filesystem_transaction.add_file(FILE_SYSTEM_PATH)?;
    filesystem_transaction.begin()?;

    let fstab = match filesystem_transaction.get_file(FILE_SYSTEM_PATH) {
        Ok(f) => f,
        Err(e) => {
            filesystem_transaction.rollback()?;
            return Err(e);
        }
    };

    if encrypted {
        let uuid = match device
            .strip_prefix("/dev/disk/by-uuid/")
            .ok_or(mx::ErrorKind::InvalidUuid)
        {
            Ok(uuid) => uuid,
            Err(e) => {
                filesystem_transaction.rollback()?;
                return Err(e);
            }
        };
        let luks_name = format!("luks-{}", uuid);
        let luks_path = format!("/dev/mapper/{}", luks_name);
        let luks_option = format!("boot.initrd.luks.devices.\"{}\"", luks_name);
        match mxOption::new(&format!("{}.device", luks_option))
            .set(fstab, format!("\"{}\"", device).as_str())
        {
            Ok(_) => (),
            Err(e) => {
                filesystem_transaction.rollback()?;
                return Err(e);
            }
        };

        match mxOption::new(format!("{}.device", root_option).as_str())
            .set(fstab, format!("\"{}\"", luks_path).as_str())
        {
            Ok(()) => (),
            Err(e) => {
                filesystem_transaction.rollback()?;
                return Err(e);
            }
        };
    } else {
        match mxOption::new(format!("{}.device", root_option).as_str())
            .set(fstab, format!("\"{}\"", device).as_str())
        {
            Ok(()) => (),
            Err(e) => {
                filesystem_transaction.rollback()?;
                return Err(e);
            }
        };
    }

    match mxOption::new(format!("{}.fsType", root_option).as_str())
        .set(fstab, format!("\"{}\"", fs_type).as_str())
    {
        Ok(()) => (),
        Err(e) => {
            filesystem_transaction.rollback()?;
            return Err(e);
        }
    };

    let option_path = format!("{}.options", root_option);

    match mxOption::new(&option_path).set_option_to_default(fstab) {
        Ok(_) => (),
        Err(e) => {
            filesystem_transaction.rollback()?;
            return Err(e);
        }
    };

    let list_opt = mxList::new(&option_path, true);
    for o in option {
        match list_opt.add(fstab, &format!("\"{}\"", o)) {
            Ok(()) => (),
            Err(e) => {
                filesystem_transaction.rollback()?;
                return Err(e);
            }
        };
    }

    filesystem_transaction.commit()?;
    Ok(())
}

pub fn remove_entry(mount_point: &str) -> mx::Result<bool> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);

    let mut filesystem_transaction = Transaction::new(
        CONFIG_DIRECTORY,
        &format!("remove {} entry in fstab", mount_point),
        BuildCommand::Boot,
    )?;

    filesystem_transaction.add_file(FILE_SYSTEM_PATH)?;
    filesystem_transaction.begin()?;

    let fstab = match filesystem_transaction.get_file(FILE_SYSTEM_PATH) {
        Ok(f) => f,
        Err(e) => {
            filesystem_transaction.rollback()?;
            return Err(e);
        }
    };

    let found = match mxOption::new(&root_option).set_option_all_instance_to_default(fstab) {
        Ok(f) => f,
        Err(e) => {
            filesystem_transaction.rollback()?;
            return Err(e);
        }
    };

    filesystem_transaction.commit()?;
    Ok(found)
}

pub fn add_swap(device: &str) -> mx::Result<()> {
    let mut transaction_swap = Transaction::new(
        CONFIG_DIRECTORY,
        &format!("Add swap device: {}", device),
        BuildCommand::Boot,
    )?;

    transaction_swap.add_file(FILE_SYSTEM_PATH)?;
    transaction_swap.begin()?;

    let fstab = match transaction_swap.get_file(FILE_SYSTEM_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction_swap.rollback()?;
            return Err(e);
        }
    };

    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    match list_swap.add(fstab, &new_entry) {
        Ok(()) => (),
        Err(e) => {
            transaction_swap.rollback()?;
            return Err(e);
        }
    }

    transaction_swap.commit()?;
    Ok(())
}

pub fn remove_swap(device: &str) -> mx::Result<()> {
    let mut transaction_swap = Transaction::new(
        CONFIG_DIRECTORY,
        &format!("Remove swap device: {}", device),
        BuildCommand::Boot,
    )?;

    transaction_swap.add_file(FILE_SYSTEM_PATH)?;
    transaction_swap.begin()?;

    let fstab = match transaction_swap.get_file(FILE_SYSTEM_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction_swap.rollback()?;
            return Err(e);
        }
    };

    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    match list_swap.remove(fstab, &new_entry) {
        Ok(()) => (),
        Err(e) => {
            transaction_swap.rollback()?;
            return Err(e);
        }
    };

    transaction_swap.commit()?;
    Ok(())
}

pub(super) fn get_filesystem_from_fstab(root_dir: &str) -> mx::Result<String> {
    let full = process::Command::new("nixos-generate-config")
        .args(["--root", root_dir, "--show-hardware-config"])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    let no_fs = process::Command::new("nixos-generate-config")
        .args(["--show-hardware-config", "--no-filesystems"])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    let full_str = String::from_utf8(full.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;
    let no_fs_str = String::from_utf8(no_fs.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let no_fs_lines: std::collections::HashSet<&str> = no_fs_str.lines().collect();
    let diff: Vec<&str> = full_str
        .lines()
        .filter(|line| !no_fs_lines.contains(line))
        .collect();

    Ok(diff.join("\n"))
}

pub fn def_filesystem_from_unix_fstab() -> mx::Result<()> {
    let mut transaction_reset =
        Transaction::new(CONFIG_DIRECTORY, "Reset filesystem", BuildCommand::Boot)?;

    transaction_reset.add_file(FILE_SYSTEM_PATH)?;
    transaction_reset.begin()?;

    let fstab = match transaction_reset.get_file(FILE_SYSTEM_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction_reset.rollback()?;
            return Err(e);
        }
    };

    let content: &mut String = match fstab.get_mut_file_content() {
        Ok(content) => content,
        Err(e) => {
            transaction_reset.rollback()?;
            return Err(e);
        }
    };

    let new_file: String = match get_filesystem_from_fstab("/") {
        Ok(s) => s,
        Err(e) => {
            transaction_reset.rollback()?;
            return Err(e);
        }
    };
    *content = format!("{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n", new_file);
    transaction_reset.commit()?;
    Ok(())
}
