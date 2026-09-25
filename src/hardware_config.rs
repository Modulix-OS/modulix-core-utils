//! Generates the configuration's `hardware-configuration.nix` from what the
//! machine actually is: the `nixos-generate-config` output plus the
//! `nixos-hardware` modules [`crate::detect_hardware`] selects.

use std::process;

use crate::{
    core::{
        list::List as mxList,
        param::NixParam,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    detect_hardware::driver_config::DriverConfig,
    error::io_message,
    mx,
};

/// Configuration file, relative to the config directory, holding the generated
/// hardware configuration.
const HARDWARE_CONFIG_PATH: &str = "hardware-configuration.nix";

/// Regenerates `hardware-configuration.nix` in an already-open file.
///
/// # Parameters
/// * `root_path` - root the detection runs against; anything other than `/` is
///   passed as `--root`, which is how the installer targets the system being
///   installed rather than the live one.
/// * `hardware_file` - the open configuration file, whose whole content is
///   replaced.
///
/// # Post-conditions
/// The file becomes `nixos-generate-config --show-hardware-config
/// --no-filesystems` (mount points stay in `fstab.nix`, see
/// [`crate::filesystem`]), takes `nixos-hardware` as a module parameter, and
/// imports one `nixos-hardware.nixosModules.*` per driver module detected.
/// Anything the file held before is discarded.
///
/// # Pre-conditions
/// `nixos-generate-config` must be on `PATH`, and the hardware probes
/// [`crate::detect_hardware`] runs need `pciutils`/`usbutils`/`cpuid`.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] if the generator cannot be spawned
/// (names it) or exits non-zero (carries its stderr),
/// [`mx::ErrorKind::InvalidFile`] if its output is not UTF-8, plus any error
/// from hardware detection.
pub fn write_hardware_config_no_transaction(
    root_path: &str,
    hardware_file: &mut NixFile,
) -> mx::Result<()> {
    let hardware_output = {
        let mut cmd = process::Command::new("nixos-generate-config");
        cmd.args(["--show-hardware-config", "--no-filesystems"]);
        if root_path != "/" {
            cmd.args(["--root", root_path]);
        }
        cmd.output().map_err(|e| {
            mx::ErrorKind::NixCommandError(format!("nixos-generate-config: {}", io_message(&e)))
        })?
    };
    if !hardware_output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(format!(
            "nixos-generate-config exited with {}: {}",
            hardware_output.status,
            String::from_utf8_lossy(&hardware_output.stderr)
        )));
    }

    let hardware_no_fs =
        String::from_utf8(hardware_output.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let file_content = hardware_file.get_mut_file_content()?;
    *file_content = hardware_no_fs;

    let config = DriverConfig::new()?;

    let param = NixParam::new();
    param.add(hardware_file, "nixos-hardware")?;

    let imports = mxList::new("imports", true);

    for import in config.get_module() {
        imports.add(
            hardware_file,
            &format!("nixos-hardware.nixosModules.{}", &import),
        )?;
    }
    Ok(())
}

/// Regenerates `hardware-configuration.nix` and rebuilds the system.
///
/// # Parameters
/// * `root_path` - root the detection runs against, as in
///   [`write_hardware_config_no_transaction`].
/// * `config_dir` - configuration repository to edit.
///
/// # Post-conditions
/// On success the regenerated hardware configuration is part of the active
/// generation; on error the previous file is restored by the rollback. Blocks
/// for the whole `nixos-rebuild switch`.
pub fn write_hardware(root_path: &str, config_dir: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Reset hardware config",
        config_dir,
        HARDWARE_CONFIG_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| write_hardware_config_no_transaction(root_path, file),
    )
}
