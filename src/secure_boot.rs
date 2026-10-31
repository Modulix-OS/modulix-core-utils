//! UEFI Secure Boot: reading the firmware's enrollment state, and declaring
//! `mx.bootloader.secureBoot.enable`.
//!
//! Two halves that belong together because one gates the other.
//!
//! The **state** half reads the UEFI global `SetupMode` variable out of
//! `efivarfs`. A platform in Setup Mode has no Platform Key, so anyone may
//! write the key databases without authentication - which is the only
//! situation in which enrolling keys can succeed. Enrolling a Platform Key is
//! what takes the platform back out of Setup Mode, so the transition is also
//! the signal that an enrollment worked.
//!
//! The **declaration** half writes the option mxpkgs reads. Turning it on
//! makes `modulixos/boot.nix` hand `autoGenerateKeys` and `autoEnrollKeys` to
//! nixpkgs' limine module, whose installer then runs `sbctl create-keys`,
//! `sbctl enroll-keys` and `sbctl sign` on the loader during the next build.
//! None of that happens here: this module only declares the intent.
//!
//! The option lands in [`FILE_SECURE_BOOT_PATH`], which is the same file
//! [`crate::boot_config`] writes `mx.bootloader.extraEntries` into - the whole
//! `mx.bootloader.*` namespace lives there. The two writers do not collide:
//! each resets only its own option before writing it.
//!
//! Writing the option without the firmware being in Setup Mode produces a
//! system whose loader is left unsigned and whose key databases are untouched,
//! because nixpkgs' `limine-install.py` runs `sbctl` through `subprocess.run`
//! **without** `check=True` and therefore ignores a non-zero exit. Callers are
//! expected to check [`firmware_state`] first; this module does not do it for
//! them, so that a caller can still declare the option deliberately.

use std::fs;
use std::path::Path;

use crate::{
    core::{
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    error::io_error_at,
    mx,
};

/// Directory `efivarfs` is mounted at.
///
/// Duplicated from [`crate::detect_os`]'s own copy rather than shared: that
/// module is behind the `detect-os` feature, which drags `serde` and
/// `serde_json` in, and this one needs neither.
const EFIVARS_DIRECTORY: &str = "/sys/firmware/efi/efivars";

/// Directory whose mere existence means the machine booted in UEFI mode.
const EFI_FIRMWARE_DIRECTORY: &str = "/sys/firmware/efi";

/// Size of the attribute word every `efivarfs` file starts with, before the
/// variable's own data.
const EFIVARFS_ATTRIBUTES_SIZE: usize = 4;

/// `efivarfs` file name of the UEFI global `SetupMode` variable: the variable
/// name, then the vendor GUID of the UEFI global variables.
const SETUP_MODE_VARIABLE: &str = "SetupMode-8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// Configuration file, relative to the config directory, carrying the
/// bootloader options.
///
/// The same file [`crate::boot_config::FILE_BOOT_PATH`] names; the literal is
/// repeated so this module stands on its own feature, exactly as
/// [`crate::hostname`] repeats `"configuration.nix"`.
pub const FILE_SECURE_BOOT_PATH: &str = "boot.nix";

/// Option mxpkgs reads to turn Secure Boot support on.
const OPTION_SECURE_BOOT: &str = "mx.bootloader.secureBoot.enable";

/// What the firmware's key databases allow right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareState {
    /// The machine did not boot in UEFI mode, so there is no Secure Boot at
    /// all. Not an error: a BIOS/CSM boot is a legitimate configuration.
    NotUefi,
    /// A Platform Key is enrolled. The key databases are authenticated and
    /// cannot be replaced without the platform's private key, so enrolling a
    /// new set would fail.
    UserMode,
    /// No Platform Key is enrolled. The key databases are writable without
    /// authentication, which is the only state in which `sbctl enroll-keys`
    /// can succeed.
    SetupMode,
}

impl FirmwareState {
    /// Whether keys can be enrolled in this state.
    ///
    /// # Returns
    /// `true` for [`FirmwareState::SetupMode`] only.
    pub fn allows_enrollment(self) -> bool {
        matches!(self, FirmwareState::SetupMode)
    }
}

/// Decodes the payload of the `SetupMode` `efivarfs` file.
///
/// # Parameters
/// * `raw` - the file's whole contents: the four-byte attribute word, then
///   the variable's data.
///
/// # Returns
/// [`FirmwareState::SetupMode`] when the first data byte is non-zero,
/// [`FirmwareState::UserMode`] otherwise - including when `raw` is too short
/// to carry a data byte, since a firmware that reports nothing is not a
/// firmware whose keys may be replaced.
fn parse_setup_mode(raw: &[u8]) -> FirmwareState {
    match raw.get(EFIVARFS_ATTRIBUTES_SIZE) {
        Some(0) | None => FirmwareState::UserMode,
        Some(_) => FirmwareState::SetupMode,
    }
}

/// Reads the firmware's Secure Boot enrollment state.
///
/// # Pre-conditions
/// Reading `efivarfs` needs root on a UEFI-booted machine. A machine that did
/// not boot in UEFI mode is reported, not refused.
///
/// # Returns
/// [`FirmwareState::NotUefi`] when `/sys/firmware/efi` is absent, otherwise
/// the state decoded from the `SetupMode` variable. A missing `SetupMode`
/// variable on a UEFI machine reads as [`FirmwareState::UserMode`]: the
/// firmware exposes no Setup Mode, so no enrollment is possible.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the variable exists but cannot be read, e.g.
/// `efivarfs` not being mounted or the caller not being root.
pub fn firmware_state() -> mx::Result<FirmwareState> {
    if !Path::new(EFI_FIRMWARE_DIRECTORY).exists() {
        return Ok(FirmwareState::NotUefi);
    }

    let path = format!("{EFIVARS_DIRECTORY}/{SETUP_MODE_VARIABLE}");
    match fs::read(&path) {
        Ok(raw) => Ok(parse_setup_mode(&raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FirmwareState::UserMode),
        Err(error) => Err(io_error_at(&path, error)),
    }
}

/// Declares or removes `mx.bootloader.secureBoot.enable` in `file`, without
/// opening or committing any transaction.
///
/// # Parameters
/// * `file` - the open file to edit, in memory only.
/// * `enable` - `true` to declare the option as enabled, `false` to drop the
///   declaration.
///
/// # Post-conditions
/// With `enable`, the option is created if absent or overwritten if already
/// declared. Without it the declaration is **removed** rather than set to
/// `false`: mxpkgs already defaults it to `false`, and an absent option keeps
/// the file free of a declaration that says nothing. Removal drops the
/// assignment and leaves the enclosing attribute sets behind, empty, which is
/// a no-op in Nix. Removing an option that was not declared leaves `file`
/// untouched and is not an error.
///
/// # Returns
/// `Ok(())` once the edit is applied to `file`'s in-memory buffer.
///
/// # Errors
/// Whatever `mxOption::set` or `mxOption::set_option_to_default` returns,
/// notably [`mx::ErrorKind::PermissionDenied`] on a read-only transaction and
/// [`mx::ErrorKind::TransactionNotBegin`] on a file that is not open.
pub fn set_secure_boot_no_transaction(file: &mut NixFile, enable: bool) -> mx::Result<()> {
    let option = mxOption::new(OPTION_SECURE_BOOT);
    if enable {
        option.set(file, "true")?;
    } else {
        option.set_option_to_default(file)?;
    }
    Ok(())
}

/// Declares or removes the Secure Boot option and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `enable` - `true` to enable Secure Boot support, `false` to drop it.
///
/// # Pre-conditions
/// Enabling only has an effect when the firmware is in
/// [`FirmwareState::SetupMode`] at the moment of the rebuild, since the key
/// enrollment happens inside the bootloader installer. Checking that is the
/// caller's job - see the module documentation for why this function does not
/// do it.
///
/// # Post-conditions
/// On success the option is part of the active generation, and enabling it
/// means the rebuild has generated a key set, enrolled it and signed the
/// loader. On error the configuration is rolled back by
/// [`transaction::make_transaction`], which owns begin, commit and rollback.
/// Blocks for the whole `nixos-rebuild switch`.
///
/// # Errors
/// Any error from the transaction or from
/// [`set_secure_boot_no_transaction`].
pub fn set_secure_boot(config_dir: &str, enable: bool) -> mx::Result<()> {
    transaction::make_transaction(
        if enable {
            "Enable secure boot"
        } else {
            "Disable secure boot"
        },
        config_dir,
        FILE_SECURE_BOOT_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| set_secure_boot_no_transaction(file, enable),
    )
}

#[cfg(test)]
#[path = "secure_boot_tests.rs"]
mod tests;
