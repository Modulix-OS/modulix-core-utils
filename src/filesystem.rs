use std::process;

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};

const FILE_SYSTEM_PATH: &str = "fstab.nix";

pub fn add_entry_no_transaction(
    fstab: &mut NixFile,
    mount_point: &str,
    device: &str,
    fs_type: &str,
    option: &[&str],
    encrypted: bool,
) -> mx::Result<()> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);
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
    Ok(())
}

pub fn add_entry(
    config_dir: &str,
    mount_point: &str,
    device: &str,
    fs_type: &str,
    option: &[&str],
    encrypted: bool,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add {} entry with device: {} in fstab", mount_point, device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        |file| add_entry_no_transaction(file, mount_point, device, fs_type, option, encrypted),
    )
}

pub fn remove_entry_no_transaction(fstab: &mut NixFile, mount_point: &str) -> mx::Result<bool> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);
    let found = mxOption::new(&root_option).set_option_all_instance_to_default(fstab)?;
    Ok(found)
}

pub fn remove_entry(config_dir: &str, mount_point: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("remove {} entry in fstab", mount_point),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        |file| remove_entry_no_transaction(file, mount_point),
    )
}

pub fn add_swap_no_transaction(fstab: &mut NixFile, device: &str) -> mx::Result<()> {
    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.add(fstab, &new_entry)?;
    Ok(())
}

pub fn add_swap(config_dir: &str, device: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add swap device: {}", device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        |file| add_swap_no_transaction(file, device),
    )
}

pub fn remove_swap_no_transaction(fstab: &mut NixFile, device: &str) -> mx::Result<()> {
    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.remove(fstab, &new_entry)?;
    Ok(())
}

pub fn remove_swap(config_dir: &str, device: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove swap device: {}", device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        |file| remove_swap_no_transaction(file, device),
    )
}

pub(super) fn get_filesystem_from_fstab(root_dir: &str) -> mx::Result<String> {
    let mut cmd_full = process::Command::new("nixos-generate-config");
    cmd_full.args(["--show-hardware-config"]);
    if root_dir != "/" {
        cmd_full.args(["--root", root_dir]);
    }
    let full = cmd_full.output().map_err(mx::ErrorKind::IOError)?;

    let mut cmd_no_fs = process::Command::new("nixos-generate-config");
    cmd_no_fs.args(["--show-hardware-config", "--no-filesystems"]);
    if root_dir != "/" {
        cmd_no_fs.args(["--root", root_dir]);
    }
    let no_fs = cmd_no_fs.output().map_err(mx::ErrorKind::IOError)?;

    let full_str = String::from_utf8(full.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;
    let no_fs_str = String::from_utf8(no_fs.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let no_fs_lines: std::collections::HashSet<&str> = no_fs_str.lines().collect();
    let diff: Vec<&str> = full_str
        .lines()
        .filter(|line| !no_fs_lines.contains(line))
        .collect();

    Ok(diff.join("\n"))
}

pub fn def_filesystem_from_unix_fstab_no_transaction(fstab: &mut NixFile) -> mx::Result<()> {
    let content: &mut String = fstab.get_mut_file_content()?;
    let new_file: String = get_filesystem_from_fstab("/")?;
    *content = format!("{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n", new_file);
    Ok(())
}

pub fn def_filesystem_from_unix_fstab(config_dir: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Reset filesystem",
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        |file| def_filesystem_from_unix_fstab_no_transaction(file),
    )
}
