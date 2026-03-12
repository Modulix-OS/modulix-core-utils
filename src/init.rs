use const_format::concatcp;

use crate::core::transaction::Transaction;
use crate::core::transaction::transaction::BuildCommand;
use crate::{CONFIG_DIRECTORY, filesystem, mx};
use std::path::Path;
use std::{fs, process};

const FLAKE_FILE: &str = concat!(
    r#"{
  description = "Modulix OS";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/release-25.11";
  };
  outputs = { self, nixpkgs }: {
    nixosConfigurations =
    {
      "default" = let
        system = ""#,
    env!("TARGET_NIX"),
    r#"";
      in nixpkgs.lib.nixosSystem
      {
        system = system;
        modules = [
          ./configuration.nix
        ];
      };
    };
  };
}
"#
);

const CONFIG_FILE: &str = r#"{ config, lib, pkgs, ... }:
{
  imports = [
    ./hardware-configuration.nix
    ./fstab.nix
  ];
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  boot.loader.limine.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
  services.xserver.enable = true;
  services.displayManager.gdm.enable = true;
  services.desktopManager.gnome.enable = true;
  system.stateVersion = "25.11";
}
"#;

pub fn init_repo(root_path: &str) -> mx::Result<()> {
    let path_config = root_path.to_owned() + "/" + CONFIG_DIRECTORY;
    let repo_path = Path::new(path_config.as_str());

    if !repo_path.exists() {
        fs::create_dir_all(repo_path).map_err(mx::ErrorKind::IOError)?;
    }

    if git2::Repository::open(repo_path).is_ok() {
        return Ok(());
    }

    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(repo_path, &opts).map_err(mx::ErrorKind::GitError)?;

    let hardware_output = process::Command::new("nixos-generate-config")
        .args(["--show-hardware-config", "--no-filesystems"])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    let hardware_no_fs =
        String::from_utf8(hardware_output.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let fs = format!(
        "{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n",
        filesystem::get_filesystem_from_fstab()?
    );

    let mut initial_transaction =
        Transaction::new(&path_config, "initial commit", BuildCommand::Build)?;

    let files: &[(&str, &str)] = &[
        ("flake.nix", FLAKE_FILE),
        ("configuration.nix", CONFIG_FILE),
        ("hardware-configuration.nix", &hardware_no_fs),
        ("fstab.nix", &fs),
    ];
    for (f, _) in files {
        initial_transaction.add_file(f)?;
    }

    initial_transaction.begin()?;

    // Associer chaque fichier à son contenu

    for (filename, content) in files {
        let file_content = match initial_transaction.get_file(filename) {
            Ok(file) => match file.get_mut_file_content() {
                Ok(c) => c,
                Err(e) => {
                    initial_transaction.rollback()?;
                    return Err(e);
                }
            },
            Err(e) => {
                initial_transaction.rollback()?;
                return Err(e);
            }
        };
        *file_content = content.to_string();
    }

    initial_transaction.commit()?;

    Ok(())
}
