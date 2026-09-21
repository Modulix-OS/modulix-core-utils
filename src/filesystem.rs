//! Declares mount points and swap devices in the configuration's `fstab.nix`.
//!
//! Each operation comes in two flavours: a `*_no_transaction` one editing an
//! already-open `NixFile`, for a caller batching several changes into one
//! transaction, and a wrapper opening its own transaction (edit plus
//! `nixos-rebuild switch`, revertible as a whole).

use std::process;

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    mx,
};

/// Configuration file, relative to the config directory, holding
/// `fileSystems` and `swapDevices`.
const FILE_SYSTEM_PATH: &str = "fstab.nix";

/// Declares one mount point in an already-open `fstab.nix`.
///
/// # Parameters
/// * `fstab` - the open configuration file to edit.
/// * `mount_point` - absolute mount path, used as the `fileSystems` key.
/// * `device` - device to mount; for an encrypted volume it must be the
///   `/dev/disk/by-uuid/<uuid>` form of the *LUKS container*.
/// * `fs_type` - filesystem type as NixOS names it (`ext4`, `btrfs`, …).
/// * `option` - mount options, written as the option list; an empty slice
///   leaves NixOS' defaults.
/// * `encrypted` - when true, also declares
///   `boot.initrd.luks.devices."luks-<uuid>"` for `device` and mounts the
///   resulting `/dev/mapper/luks-<uuid>` instead of `device` itself.
///
/// # Post-conditions
/// Any options previously declared for `mount_point` are reset before the new
/// ones are added, so the resulting list holds exactly `option`.
///
/// # Errors
/// [`mx::ErrorKind::InvalidUuid`] when `encrypted` is true and `device` is not
/// a `/dev/disk/by-uuid/` path, plus any error from editing the file.
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

/// Declares one mount point and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit (see
///   [`crate::CONFIG_DIRECTORY`]).
/// * `mount_point`, `device`, `fs_type`, `option`, `encrypted` - as in
///   [`add_entry_no_transaction`].
///
/// # Post-conditions
/// On success the mount point is part of the active generation; on error the
/// configuration is left as it was, the transaction having been rolled back.
/// Blocks for the whole `nixos-rebuild switch`.
///
/// # Errors
/// Any error from [`add_entry_no_transaction`], plus
/// [`mx::ErrorKind::BuildError`] if the rebuild fails.
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
        UpdateInput::Keep,
        |file| add_entry_no_transaction(file, mount_point, device, fs_type, option, encrypted),
    )
}

/// Removes a mount point from an already-open `fstab.nix`.
///
/// # Parameters
/// * `fstab` - the open configuration file to edit.
/// * `mount_point` - mount path whose `fileSystems` entry must go.
///
/// # Returns
/// `true` if at least one declaration was found and removed, `false` if the
/// mount point was not declared (which is not an error).
///
/// # Post-conditions
/// Every instance of the entry is removed, including duplicates. A LUKS
/// declaration added by [`add_entry_no_transaction`] is *not* removed.
pub fn remove_entry_no_transaction(fstab: &mut NixFile, mount_point: &str) -> mx::Result<bool> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);
    let found = mxOption::new(&root_option).set_option_all_instance_to_default(fstab)?;
    Ok(found)
}

/// Removes a mount point and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `mount_point` - mount path whose entry must go.
///
/// # Returns
/// `true` if a declaration was removed, `false` if there was nothing to
/// remove - in which case the rebuild still runs.
///
/// # Errors
/// [`mx::ErrorKind::BuildError`] if the rebuild fails; the configuration is
/// then rolled back.
pub fn remove_entry(config_dir: &str, mount_point: &str) -> mx::Result<bool> {
    transaction::make_transaction(
        &format!("remove {} entry in fstab", mount_point),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_entry_no_transaction(file, mount_point),
    )
}

/// Appends a swap device to `swapDevices` in an already-open `fstab.nix`.
///
/// # Parameters
/// * `fstab` - the open configuration file to edit.
/// * `device` - device expression as it must appear in Nix, i.e. already
///   quoted (`"/dev/sda2"`); it is inserted verbatim into `{device=…;}`.
///
/// # Post-conditions
/// The list is declared if absent, and the entry is added only once:
/// re-adding the same device is a no-op.
pub fn add_swap_no_transaction(fstab: &mut NixFile, device: &str) -> mx::Result<()> {
    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.add(fstab, &new_entry)?;
    Ok(())
}

/// Declares a swap device and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `device` - device expression, as in [`add_swap_no_transaction`].
///
/// # Post-conditions
/// On success the swap device is part of the active generation; on error the
/// configuration is rolled back.
pub fn add_swap(config_dir: &str, device: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add swap device: {}", device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| add_swap_no_transaction(file, device),
    )
}

/// Removes a swap device from `swapDevices` in an already-open `fstab.nix`.
///
/// # Parameters
/// * `fstab` - the open configuration file to edit.
/// * `device` - device expression, spelled exactly as it was passed to
///   [`add_swap_no_transaction`]: the entry is matched by text.
///
/// # Post-conditions
/// A device that is not declared is silently ignored; the whole option is
/// dropped when its last entry goes.
pub fn remove_swap_no_transaction(fstab: &mut NixFile, device: &str) -> mx::Result<()> {
    let list_swap = mxList::new("swapDevices", true);
    let new_entry = format!("{{device={};}}", device);
    list_swap.remove(fstab, &new_entry)?;
    Ok(())
}

/// Removes a swap device and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `device` - device expression, as in [`remove_swap_no_transaction`].
///
/// # Post-conditions
/// On success the swap device is gone from the active generation; on error the
/// configuration is rolled back.
pub fn remove_swap(config_dir: &str, device: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Remove swap device: {}", device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_swap_no_transaction(file, device),
    )
}

/// Extracts the filesystem part of the hardware configuration
/// `nixos-generate-config` would emit.
///
/// # Parameters
/// * `root_dir` - root the detection runs against; anything other than `/` is
///   passed as `--root`, which is what lets the installer probe the target
///   system instead of the live one.
///
/// # Returns
/// The lines present in `--show-hardware-config` but absent from
/// `--show-hardware-config --no-filesystems`, i.e. the `fileSystems` and
/// `swapDevices` declarations only, joined by newlines and without the
/// surrounding module braces.
///
/// # Pre-conditions
/// `nixos-generate-config` must be on `PATH`, and detecting a root other than
/// the live one usually requires privileges.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if either invocation cannot be spawned, and
/// [`mx::ErrorKind::InvalidFile`] if its output is not valid UTF-8. A non-zero
/// exit status is not reported: it surfaces as an empty or partial result.
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

/// Regenerates `fstab.nix` from what the running system actually has mounted.
///
/// # Parameters
/// * `fstab` - the open configuration file, whose whole content is replaced.
///
/// # Post-conditions
/// Every previous declaration in the file is discarded, LUKS entries added by
/// [`add_entry_no_transaction`] included, and replaced by a freshly generated
/// NixOS module wrapping the output of `get_filesystem_from_fstab` for `/`.
pub fn def_filesystem_from_unix_fstab_no_transaction(fstab: &mut NixFile) -> mx::Result<()> {
    let content: &mut String = fstab.get_mut_file_content()?;
    let new_file: String = get_filesystem_from_fstab("/")?;
    *content = format!("{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n", new_file);
    Ok(())
}

/// Regenerates `fstab.nix` from the running system and rebuilds.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
///
/// # Post-conditions
/// On success the regenerated declarations are part of the active generation;
/// on error the previous `fstab.nix` is restored by the rollback.
pub fn def_filesystem_from_unix_fstab(config_dir: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Reset filesystem",
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| def_filesystem_from_unix_fstab_no_transaction(file),
    )
}
