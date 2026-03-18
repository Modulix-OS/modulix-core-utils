use std::process;

use crate::{
    core::{
        list::List as mxList,
        param::NixParam,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    detect_hardware::driver_config::DriverConfig,
    mx,
};

const HARDWARE_CONFIG_PATH: &str = "hardware-configuration.nix";

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
        cmd.output().map_err(mx::ErrorKind::IOError)?
    };

    let hardware_no_fs =
        String::from_utf8(hardware_output.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let file_content = hardware_file.get_mut_file_content()?;
    *file_content = hardware_no_fs;

    let config = DriverConfig::new()?;

    let param = NixParam::new();
    param.add(hardware_file, "nixos-hardware")?;

    dbg!(config.get_module());

    let imports = mxList::new("imports", true);

    for import in config.get_module() {
        imports.add(
            hardware_file,
            &format!("nixos-hardware.nixosModules.{}", &import),
        )?;
    }
    Ok(())
}

pub fn write_hardware(root_path: &str, config_dir: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Reset hardware config",
        config_dir,
        HARDWARE_CONFIG_PATH,
        BuildCommand::Switch,
        |file| write_hardware_config_no_transaction(root_path, file),
    )
}
