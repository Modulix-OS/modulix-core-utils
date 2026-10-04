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
    error::io_message,
    mx,
};

/// Configuration file, relative to the config directory, holding
/// `fileSystems` and `swapDevices`.
const FILE_SYSTEM_PATH: &str = "fstab.nix";

/// LUKS container backing an encrypted mount point.
///
/// # Fields
/// * `name` - `dm-crypt` mapper name. Becomes the
///   `boot.initrd.luks.devices."<name>"` key and the `/dev/mapper/<name>` the
///   mount point is declared on, so it must be the name the container is
///   actually opened as. [`default_luks_name`] renders the `luks-<uuid>`
///   spelling that used to be hardcoded here.
/// * `tpm2` - a TPM2 token is enrolled on the container, so the initrd must
///   try it (`crypttabExtraOpts = [ "tpm2-device=auto" ]`). Requires
///   `boot.initrd.systemd.enable`, which this module does not set.
pub struct LuksEntry<'a> {
    pub name: &'a str,
    pub tpm2: bool,
}

/// Renders the default mapper name for a LUKS container.
///
/// # Parameters
/// * `device` - the `/dev/disk/by-uuid/<uuid>` path of the LUKS container.
///
/// # Returns
/// `luks-<uuid>`, the spelling [`add_entry_no_transaction`] hardcoded before
/// [`LuksEntry::name`] made it a parameter.
///
/// # Errors
/// [`mx::ErrorKind::InvalidUuid`] when `device` is not a `/dev/disk/by-uuid/`
/// path.
pub fn default_luks_name(device: &str) -> mx::Result<String> {
    let uuid = device
        .strip_prefix("/dev/disk/by-uuid/")
        .ok_or(mx::ErrorKind::InvalidUuid)?;
    Ok(format!("luks-{}", uuid))
}

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
/// * `luks` - `Some` for an encrypted volume: also declares
///   `boot.initrd.luks.devices."<luks.name>"` for `device` and mounts the
///   resulting `/dev/mapper/<luks.name>` instead of `device` itself, with
///   `crypttabExtraOpts` when `luks.tpm2`. `None` mounts `device` directly.
///
/// # Post-conditions
/// Any options previously declared for `mount_point` are reset before the new
/// ones are added, so the resulting list holds exactly `option`.
///
/// # Errors
/// [`mx::ErrorKind::InvalidUuid`] when `luks` is `Some` and `device` is not a
/// `/dev/disk/by-uuid/` path, plus any error from editing the file.
pub fn add_entry_no_transaction(
    fstab: &mut NixFile,
    mount_point: &str,
    device: &str,
    fs_type: &str,
    option: &[&str],
    luks: Option<&LuksEntry<'_>>,
) -> mx::Result<()> {
    let root_option = format!("fileSystems.\"{}\"", mount_point);
    if let Some(luks) = luks {
        if !device.starts_with("/dev/disk/by-uuid/") {
            return Err(mx::ErrorKind::InvalidUuid);
        }
        let luks_path = format!("/dev/mapper/{}", luks.name);
        mxOption::new(&format!("{}.device", luks_option(luks.name)))
            .set(fstab, format!("\"{}\"", device).as_str())?;

        mxOption::new(format!("{}.device", root_option).as_str())
            .set(fstab, format!("\"{}\"", luks_path).as_str())?;

        if luks.tpm2 {
            set_luks_tpm2_no_transaction(fstab, luks.name)?;
        }
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

/// Dotted option path of one `boot.initrd.luks.devices` entry.
///
/// # Parameters
/// * `luks_name` - mapper name, used verbatim as the attribute key.
///
/// # Returns
/// `boot.initrd.luks.devices."<luks_name>"`, quotes included, ready to be
/// suffixed with `.device` or `.crypttabExtraOpts`.
fn luks_option(luks_name: &str) -> String {
    format!("boot.initrd.luks.devices.\"{}\"", luks_name)
}

/// Adds `tpm2-device=auto` to a LUKS entry's `crypttabExtraOpts` in an
/// already-open `fstab.nix`.
///
/// Split out of [`add_entry_no_transaction`] because an `fstab.nix` generated
/// by `nixos-generate-config` already carries the entry's `.device` — it
/// detects the open mapper and writes it under the live `dm/name` — but never
/// emits `crypttabExtraOpts`. An installer that lets the generator describe
/// the encrypted root therefore only has this one attribute left to add, and
/// must *not* declare `.device` a second time: two definitions of the same
/// `types.str` option make the NixOS module system fail, even at equal
/// values.
///
/// # Parameters
/// * `fstab` - the open configuration file to edit.
/// * `luks_name` - mapper name of an entry already declared in the file, or
///   one this call brings into being with `crypttabExtraOpts` as its only
///   attribute.
///
/// # Pre-conditions
/// The installed system enables `boot.initrd.systemd.enable` and
/// `boot.initrd.systemd.tpm2.enable`; neither is written here.
///
/// # Post-conditions
/// `boot.initrd.luks.devices."<luks_name>".crypttabExtraOpts` contains
/// `"tpm2-device=auto"` exactly once — the list is unique-valued, so a second
/// call is a no-op.
///
/// Not durable against [`def_filesystem_from_unix_fstab_no_transaction`],
/// which replaces the whole file with generator output and so drops this
/// attribute.
///
/// # Returns
/// `Ok(())` once the edit is applied to `fstab`'s in-memory buffer.
///
/// # Errors
/// [`mx::ErrorKind::OptionIsNotList`] if `crypttabExtraOpts` already holds a
/// non-list value, plus any error `mxList::add` propagates.
pub fn set_luks_tpm2_no_transaction(fstab: &mut NixFile, luks_name: &str) -> mx::Result<()> {
    mxList::new(
        &format!("{}.crypttabExtraOpts", luks_option(luks_name)),
        true,
    )
    .add(fstab, "\"tpm2-device=auto\"")?;
    Ok(())
}

/// Declares one mount point and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit (see
///   [`crate::CONFIG_DIRECTORY`]).
/// * `mount_point`, `device`, `fs_type`, `option`, `luks` - as in
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
    luks: Option<&LuksEntry<'_>>,
) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add {} entry with device: {} in fstab", mount_point, device),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| add_entry_no_transaction(file, mount_point, device, fs_type, option, luks),
    )
}

/// Adds `tpm2-device=auto` to a LUKS entry's `crypttabExtraOpts` and rebuilds
/// the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `luks_name` - as in [`set_luks_tpm2_no_transaction`].
///
/// # Post-conditions
/// On success the new `crypttabExtraOpts` is part of the active generation; on
/// error the previous `fstab.nix` is restored by the rollback. A call that
/// changes nothing commits nothing and rebuilds nothing.
///
/// # Errors
/// Any error from [`set_luks_tpm2_no_transaction`], plus
/// [`mx::ErrorKind::BuildError`] if the rebuild fails.
pub fn set_luks_tpm2(config_dir: &str, luks_name: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Enroll TPM2 unlock for {} in fstab", luks_name),
        config_dir,
        FILE_SYSTEM_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| set_luks_tpm2_no_transaction(file, luks_name),
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

/// Runs `nixos-generate-config` and returns its stdout.
///
/// # Parameters
/// * `extra_args` - flags placed before `--root`, e.g.
///   `["--show-hardware-config"]`.
/// * `root_dir` - root the detection runs against; anything other than `/` is
///   passed as `--root`, which is what lets the installer probe the target
///   system instead of the live one.
///
/// # Returns
/// The generator's stdout.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] if the binary cannot be spawned or exits
/// non-zero (the message carries its exit status and stderr), and
/// [`mx::ErrorKind::InvalidFile`] if its stdout is not valid UTF-8.
fn generate_config(extra_args: &[&str], root_dir: &str) -> mx::Result<String> {
    let mut cmd = process::Command::new("nixos-generate-config");
    cmd.args(extra_args);
    if root_dir != "/" {
        cmd.args(["--root", root_dir]);
    }
    let output = cmd.output().map_err(|e| {
        mx::ErrorKind::NixCommandError(format!("nixos-generate-config: {}", io_message(&e)))
    })?;
    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(format!(
            "nixos-generate-config exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| mx::ErrorKind::InvalidFile)
}

/// Isolates the block `nixos-generate-config` adds when it is allowed to look
/// at filesystems.
///
/// The generator renders one single template, in which the `fileSystems`,
/// LUKS and `swapDevices` declarations form one contiguous chunk that
/// `--no-filesystems` simply leaves out. The chunk is therefore recovered by
/// trimming the longest common prefix and the longest common suffix of the two
/// outputs, which keeps every line of the block even when it also occurs
/// elsewhere. Comparing the two line *sets* instead would drop such lines: a
/// non-empty `swapDevices` closes on `    ];`, exactly like the `imports` list
/// present in both outputs, and losing that line leaves an unbalanced `[`.
///
/// # Parameters
/// * `full` - output of `nixos-generate-config --show-hardware-config`.
/// * `no_fs` - output of the same command plus `--no-filesystems`.
///
/// # Returns
/// The lines of `full` between the two common ends, joined by newlines and
/// stripped of the blank lines at either end; empty when both outputs are
/// identical.
///
/// # Post-conditions
/// The result is a sub-slice of `full`'s lines in their original order, so a
/// block that was balanced in `full` stays balanced.
fn extract_fs_block(full: &str, no_fs: &str) -> String {
    let full_lines: Vec<&str> = full.lines().collect();
    let no_fs_lines: Vec<&str> = no_fs.lines().collect();

    let mut start = 0;
    while start < full_lines.len()
        && start < no_fs_lines.len()
        && full_lines[start] == no_fs_lines[start]
    {
        start += 1;
    }

    let mut end_full = full_lines.len();
    let mut end_no_fs = no_fs_lines.len();
    while end_full > start
        && end_no_fs > start
        && full_lines[end_full - 1] == no_fs_lines[end_no_fs - 1]
    {
        end_full -= 1;
        end_no_fs -= 1;
    }

    let mut block = &full_lines[start..end_full];
    while block.first().is_some_and(|line| line.trim().is_empty()) {
        block = &block[1..];
    }
    while block.last().is_some_and(|line| line.trim().is_empty()) {
        block = &block[..block.len() - 1];
    }

    block.join("\n")
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
/// The `fileSystems`, LUKS and `swapDevices` declarations only, as the
/// generator rendered them and without the surrounding module braces (see
/// [`extract_fs_block`]).
///
/// # Pre-conditions
/// `nixos-generate-config` must be on `PATH`, and detecting a root other than
/// the live one usually requires privileges.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] if either invocation cannot be spawned
/// or exits non-zero (the message carries its stderr), and
/// [`mx::ErrorKind::InvalidFile`] if its output is not valid UTF-8.
pub(super) fn get_filesystem_from_fstab(root_dir: &str) -> mx::Result<String> {
    let full_str = generate_config(&["--show-hardware-config"], root_dir)?;
    let no_fs_str = generate_config(&["--show-hardware-config", "--no-filesystems"], root_dir)?;

    Ok(extract_fs_block(&full_str, &no_fs_str))
}

/// Renders the whole `fstab.nix` module for `root_dir`.
///
/// Single source of the file's skeleton, shared by the reset operation and by
/// the installer, so the three call sites cannot drift apart.
///
/// # Parameters
/// * `root_dir` - root the detection runs against, as in
///   [`get_filesystem_from_fstab`].
///
/// # Returns
/// A complete NixOS module wrapping the declarations
/// [`get_filesystem_from_fstab`] found.
///
/// # Post-conditions
/// The returned text parses as Nix; a generator output that would produce a
/// broken file is reported instead of being written.
///
/// # Errors
/// Any error from [`get_filesystem_from_fstab`], plus
/// [`mx::ErrorKind::InvalidFile`] if the rendered module does not parse.
pub(crate) fn fstab_module(root_dir: &str) -> mx::Result<String> {
    let module = format!(
        "{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n",
        get_filesystem_from_fstab(root_dir)?
    );
    if !rnix::Root::parse(&module).errors().is_empty() {
        return Err(mx::ErrorKind::InvalidFile);
    }
    Ok(module)
}

/// Regenerates `fstab.nix` from what the running system actually has mounted.
///
/// # Parameters
/// * `fstab` - the open configuration file, whose whole content is replaced.
///
/// # Post-conditions
/// Every previous declaration in the file is discarded, LUKS entries added by
/// [`add_entry_no_transaction`] included, and replaced by a freshly generated
/// NixOS module ([`fstab_module`]) for `/`.
pub fn def_filesystem_from_unix_fstab_no_transaction(fstab: &mut NixFile) -> mx::Result<()> {
    let new_file: String = fstab_module("/")?;
    let content: &mut String = fstab.get_mut_file_content()?;
    *content = new_file;
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

#[cfg(test)]
mod tests {
    use super::extract_fs_block;

    /// Output of `nixos-generate-config --show-hardware-config --no-filesystems`,
    /// reduced to the lines that matter for the extraction.
    const NO_FS: &str = "\
# Do not modify this file!
{ config, lib, pkgs, modulesPath, ... }:

{
  imports =
    [ (modulesPath + \"/installer/scan/not-detected.nix\")
    ];

  boot.initrd.availableKernelModules = [ \"nvme\" ];
  boot.initrd.kernelModules = [ ];
  boot.kernelModules = [ ];
  boot.extraModulePackages = [ ];

  nixpkgs.hostPlatform = lib.mkDefault \"x86_64-linux\";
}
";

    /// Builds the full output by splicing `block` where the generator inserts
    /// its filesystem section, i.e. after `boot.extraModulePackages`.
    fn full_with(block: &str) -> String {
        NO_FS.replace(
            "  boot.extraModulePackages = [ ];\n",
            &format!("  boot.extraModulePackages = [ ];\n{}", block),
        )
    }

    /// Counts the opening and closing occurrences of a bracket pair.
    fn balance(text: &str, open: char, close: char) -> (usize, usize) {
        (
            text.chars().filter(|c| *c == open).count(),
            text.chars().filter(|c| *c == close).count(),
        )
    }

    #[test]
    fn keeps_multiline_swap_closing_bracket() {
        let block = "\n  fileSystems.\"/\" =\n    { device = \"/dev/disk/by-uuid/aaaa\";\n      fsType = \"ext4\";\n    };\n\n  swapDevices =\n    [ { device = \"/dev/disk/by-uuid/bbbb\"; }\n    ];\n";
        let extracted = extract_fs_block(&full_with(block), NO_FS);

        assert!(extracted.contains("swapDevices ="));
        assert!(extracted.contains("    ];"));
        assert_eq!(
            balance(&extracted, '[', ']').0,
            balance(&extracted, '[', ']').1
        );
        assert_eq!(
            balance(&extracted, '{', '}').0,
            balance(&extracted, '{', '}').1
        );
    }

    #[test]
    fn keeps_empty_swap_and_filesystems() {
        let block = "\n  fileSystems.\"/\" =\n    { device = \"/dev/disk/by-uuid/aaaa\";\n      fsType = \"ext4\";\n    };\n\n  swapDevices = [ ];\n";
        let extracted = extract_fs_block(&full_with(block), NO_FS);

        assert!(extracted.contains("fileSystems.\"/\" ="));
        assert!(extracted.contains("swapDevices = [ ];"));
        assert!(!extracted.contains("nixpkgs.hostPlatform"));
        assert!(!extracted.contains("boot.extraModulePackages"));
    }

    #[test]
    fn keeps_luks_line_between_two_filesystems() {
        let block = "\n  fileSystems.\"/\" =\n    { device = \"/dev/mapper/luks-cccc\";\n      fsType = \"ext4\";\n    };\n\n  boot.initrd.luks.devices.\"luks-cccc\".device = \"/dev/disk/by-uuid/cccc\";\n\n  fileSystems.\"/boot\" =\n    { device = \"/dev/disk/by-uuid/5BA4-ED5B\";\n      fsType = \"vfat\";\n      options = [ \"fmask=0077\" \"dmask=0077\" ];\n    };\n\n  swapDevices = [ ];\n";
        let extracted = extract_fs_block(&full_with(block), NO_FS);

        assert!(extracted.contains("boot.initrd.luks.devices.\"luks-cccc\".device"));
        assert_eq!(extracted.matches("fileSystems.").count(), 2);
        assert_eq!(
            balance(&extracted, '{', '}').0,
            balance(&extracted, '{', '}').1
        );
    }

    #[test]
    fn reduces_to_swap_when_no_filesystem_found() {
        let block = "\n  swapDevices = [ ];\n";
        let extracted = extract_fs_block(&full_with(block), NO_FS);

        assert_eq!(extracted, "  swapDevices = [ ];");
    }

    #[test]
    fn yields_nothing_when_outputs_match() {
        assert_eq!(extract_fs_block(NO_FS, NO_FS), "");
    }
}

#[cfg(test)]
mod luks_tests {
    use super::{
        LuksEntry, add_entry_no_transaction, default_luks_name, set_luks_tpm2_no_transaction,
    };
    use crate::core::transaction::Transaction;
    use crate::core::transaction::transaction::{BuildCommand, TransactionPermission};
    use std::fs;
    use tempfile::TempDir;

    /// `fstab.nix` as `nixos-generate-config` leaves it for an encrypted root:
    /// the LUKS entry is already there, keyed on the live mapper name, and
    /// carries no `crypttabExtraOpts`.
    const GENERATED: &str = "\
{config, lib, pkgs, ...}:
{
  fileSystems.\"/\" =
    { device = \"/dev/mapper/modulixroot\";
      fsType = \"ext4\";
    };

  boot.initrd.luks.devices.\"modulixroot\".device = \"/dev/disk/by-uuid/cafe\";

  swapDevices = [ ];
}
";

    /// Runs `edit` against a real `fstab.nix` seeded with `seed`, inside a
    /// writable transaction that is rolled back, and returns the resulting
    /// buffer.
    fn with_fstab(
        seed: &str,
        edit: impl FnOnce(&mut crate::core::transaction::file_lock::NixFile),
    ) -> String {
        let dir = TempDir::new().expect("failed to create temporary directory");
        git2::Repository::init(dir.path()).expect("git init failed");
        fs::write(dir.path().join("fstab.nix"), seed).expect("failed to write fstab.nix");
        fs::write(
            dir.path().join("configuration.nix"),
            "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
        )
        .expect("failed to write configuration.nix");

        let repo_path = format!("{}/", dir.path().to_str().unwrap());
        let mut tx = Transaction::new(
            &repo_path,
            "test",
            BuildCommand::Boot,
            TransactionPermission::Writtable,
        )
        .expect("Transaction::new failed");
        tx.add_file("fstab.nix").expect("add_file failed");
        tx.begin().expect("begin failed");

        let file = tx.get_file_mut("fstab.nix").expect("get_file_mut failed");
        edit(file);
        let content = file
            .get_file_content()
            .expect("get_file_content failed")
            .clone();
        tx.rollback().expect("rollback failed");
        content
    }

    #[test]
    fn default_luks_name_keeps_the_historical_spelling() {
        assert_eq!(
            default_luks_name("/dev/disk/by-uuid/cafe").unwrap(),
            "luks-cafe"
        );
        assert!(default_luks_name("/dev/sda2").is_err());
    }

    #[test]
    fn add_entry_keys_the_luks_entry_on_the_given_name() {
        let content = with_fstab("{config, lib, pkgs, ...}:\n{\n}\n", |file| {
            add_entry_no_transaction(
                file,
                "/",
                "/dev/disk/by-uuid/cafe",
                "ext4",
                &[],
                Some(&LuksEntry {
                    name: "modulixroot",
                    tpm2: false,
                }),
            )
            .expect("add_entry_no_transaction failed");
        });

        // `mxOption::set` renders a brand-new dotted path as nested attribute
        // sets, so the entry is keyed under `devices` instead of spelled flat.
        assert!(content.contains("luks = {"));
        assert!(content.contains("\"modulixroot\" = {"));
        assert!(content.contains("device = \"/dev/disk/by-uuid/cafe\";"));
        assert!(!content.contains("luks-cafe"));
        assert!(content.contains("/dev/mapper/modulixroot"));
        assert!(!content.contains("crypttabExtraOpts"));
    }

    #[test]
    fn add_entry_enrolls_tpm2_only_when_asked() {
        let content = with_fstab("{config, lib, pkgs, ...}:\n{\n}\n", |file| {
            add_entry_no_transaction(
                file,
                "/",
                "/dev/disk/by-uuid/cafe",
                "ext4",
                &[],
                Some(&LuksEntry {
                    name: "modulixroot",
                    tpm2: true,
                }),
            )
            .expect("add_entry_no_transaction failed");
        });

        assert!(content.contains("crypttabExtraOpts"));
        assert!(content.contains("\"tpm2-device=auto\""));
    }

    #[test]
    fn add_entry_refuses_a_luks_device_that_is_not_by_uuid() {
        let content = with_fstab("{config, lib, pkgs, ...}:\n{\n}\n", |file| {
            let err = add_entry_no_transaction(
                file,
                "/",
                "/dev/sda2",
                "ext4",
                &[],
                Some(&LuksEntry {
                    name: "modulixroot",
                    tpm2: false,
                }),
            )
            .expect_err("a non-by-uuid LUKS device must be refused");
            assert!(matches!(err, crate::mx::ErrorKind::InvalidUuid));
        });

        assert!(!content.contains("boot.initrd.luks.devices"));
    }

    #[test]
    fn tpm2_extends_the_entry_the_generator_already_wrote() {
        let content = with_fstab(GENERATED, |file| {
            set_luks_tpm2_no_transaction(file, "modulixroot")
                .expect("set_luks_tpm2_no_transaction failed");
        });

        // One single `.device` definition: a second one would make the NixOS
        // module system fail, which is the whole point of not re-declaring it.
        assert_eq!(
            content
                .matches("boot.initrd.luks.devices.\"modulixroot\".device")
                .count(),
            1
        );
        assert!(content.contains("\"tpm2-device=auto\""));
    }

    #[test]
    fn tpm2_enrollment_is_idempotent() {
        let content = with_fstab(GENERATED, |file| {
            for _ in 0..3 {
                set_luks_tpm2_no_transaction(file, "modulixroot")
                    .expect("set_luks_tpm2_no_transaction failed");
            }
        });

        assert_eq!(content.matches("\"tpm2-device=auto\"").count(), 1);
    }
}
