use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::LOCK_SKIP_REBUILD_FILE;
use crate::core::transaction::transaction::TransactionPermission;
use crate::core::transaction::transaction::{BuildCommand, UpdateInput};
use crate::{CONFIG_DIRECTORY, filesystem, hardware_config, locale, mx, user};
use std::path::Path;
use std::{fs, process};

const DEFAULT_SHELL: &str = "/run/current-system/sw/bin/bash";

const FLAKE_FILE: &str = concat!(
    r#"{
  description = "Modulix OS";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/release-25.11";
    nixos-hardware.url = "github:NixOS/nixos-hardware";
  };
  outputs = { self, nixpkgs, nixos-hardware, ... }@inputs: {
    nixosConfigurations =
    {
      "default" = let
        system = ""#,
    env!("TARGET_NIX"),
    r#"";
      in nixpkgs.lib.nixosSystem
      {
        system = system;
        specialArgs = { inherit self nixos-hardware inputs; };
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

fn remove_dir_recursive(path: &Path) -> mx::Result<()> {
    for entry in fs::read_dir(path).map_err(mx::ErrorKind::IOError)? {
        let entry = entry.map_err(mx::ErrorKind::IOError)?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            remove_dir_recursive(&entry_path)?;
        } else {
            NixFile::delete(entry_path.to_str().ok_or(mx::ErrorKind::InvalidFile)?)?;
        }
    }
    fs::remove_dir(path).map_err(mx::ErrorKind::IOError)?;
    Ok(())
}

pub fn init_repo(root_path: &str) -> mx::Result<()> {
    let path_config = root_path.to_owned() + "/" + CONFIG_DIRECTORY;
    let repo_path = Path::new(path_config.as_str());
    if repo_path.exists() {
        remove_dir_recursive(repo_path)?;
    }
    fs::create_dir_all(repo_path).map_err(mx::ErrorKind::IOError)?;

    if git2::Repository::open(repo_path).is_ok() {
        return Ok(());
    }

    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(repo_path, &opts).map_err(mx::ErrorKind::GitError)?;

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

    let fs = format!(
        "{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n",
        filesystem::get_filesystem_from_fstab(root_path)?
    );

    #[cfg(debug_assertions)]
    let mut initial_transaction = Transaction::new(
        &path_config,
        "initial commit",
        BuildCommand::Boot,
        TransactionPermission::Writtable,
    )?;
    #[cfg(not(debug_assertions))]
    let mut initial_transaction = Transaction::new(
        &path_config,
        "initial commit",
        BuildCommand::Install,
        TransactionPermission::Writtable,
    )?;

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

    // Associate each file with its content

    for (filename, content) in files {
        let file_content = match initial_transaction.get_file_mut(filename) {
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

    initial_transaction.commit(UpdateInput::UpdateAll)?;

    Ok(())
}

pub struct InitParams {
    pub root: String,
    pub hostname: String,
    pub username: String,
    pub full_name: String,
    pub desktop: String,
    pub locale: String,
    pub timezone: String,
    pub kb_layout: String,
    pub kb_variant: String,
    pub console_keymap: String,
}

fn desktop_block(desktop: &str, kb_layout: &str, kb_variant: &str) -> String {
    if !matches!(desktop, "gnome" | "plasma" | "lxqt") {
        return String::new();
    }
    let base = format!(
        "  services.xserver.enable = true;\n  \
         services.xserver.xkb.layout = \"{kb_layout}\";\n  \
         services.xserver.xkb.variant = \"{kb_variant}\";\n"
    );
    let de = match desktop {
        "gnome" => {
            "  services.displayManager.gdm.enable = true;\n  \
             services.desktopManager.gnome.enable = true;\n"
        }
        "plasma" => {
            "  services.displayManager.sddm.enable = true;\n  \
             services.displayManager.sddm.wayland.enable = true;\n  \
             services.desktopManager.plasma6.enable = true;\n"
        }
        "lxqt" => {
            "  services.displayManager.lightdm.enable = true;\n  \
             services.xserver.desktopManager.lxqt.enable = true;\n"
        }
        _ => "",
    };
    format!("{base}{de}")
}

fn configuration_nix(p: &InitParams) -> String {
    format!(
        r#"{{ config, lib, pkgs, ... }}:
{{
  imports = [
    ./hardware-configuration.nix
    ./fstab.nix
  ];
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  boot.loader.limine.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  networking.hostName = "{hostname}";

{desktop}
  system.stateVersion = "25.11";
}}
"#,
        hostname = p.hostname,
        desktop = desktop_block(&p.desktop, &p.kb_layout, &p.kb_variant),
    )
}

fn hold_skip_rebuild_lock() -> mx::Result<fs::File> {
    let file = fs::File::create(LOCK_SKIP_REBUILD_FILE).map_err(mx::ErrorKind::IOError)?;
    file.try_lock().map_err(|_| mx::ErrorKind::FailToLock)?;
    Ok(file)
}

pub fn init(params: &InitParams) -> mx::Result<()> {
    let path_config = params.root.clone() + "/" + CONFIG_DIRECTORY;
    let repo_path = Path::new(path_config.as_str());
    if repo_path.exists() {
        remove_dir_recursive(repo_path)?;
    }
    fs::create_dir_all(repo_path).map_err(mx::ErrorKind::IOError)?;

    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(repo_path, &opts).map_err(mx::ErrorKind::GitError)?;

    let fstab = format!(
        "{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n",
        filesystem::get_filesystem_from_fstab(&params.root)?
    );
    let config = configuration_nix(params);

    let _skip_rebuild = hold_skip_rebuild_lock()?;

    let mut tx = Transaction::new(
        &path_config,
        "modulix init",
        BuildCommand::Boot,
        TransactionPermission::Writtable,
    )?;

    const HARDWARE_FILE: &str = "hardware-configuration.nix";
    for f in ["flake.nix", HARDWARE_FILE, "fstab.nix"] {
        tx.add_file(f)?;
    }

    tx.begin()?;

    for (name, content) in [
        ("flake.nix", FLAKE_FILE),
        ("configuration.nix", config.as_str()),
        ("fstab.nix", fstab.as_str()),
    ] {
        match tx.get_file_mut(name) {
            Ok(file) => match file.get_mut_file_content() {
                Ok(c) => *c = content.to_string(),
                Err(e) => {
                    tx.rollback()?;
                    return Err(e);
                }
            },
            Err(e) => {
                tx.rollback()?;
                return Err(e);
            }
        }
    }

    // Hardware config: detected system + matching nixos-hardware modules.
    match tx.get_file_mut(HARDWARE_FILE) {
        Ok(file) => {
            if let Err(e) = hardware_config::write_hardware_config_no_transaction(&params.root, file)
            {
                tx.rollback()?;
                return Err(e);
            }
        }
        Err(e) => {
            tx.rollback()?;
            return Err(e);
        }
    }

    tx.commit(UpdateInput::UpdateAll)?;

    locale::set_locale(
        &path_config,
        &params.timezone,
        &params.locale,
        &params.console_keymap,
    )?;
    user::add(
        &path_config,
        &params.username,
        "",
        &params.full_name,
        DEFAULT_SHELL,
        &["wheel", "networkmanager"],
        true,
    )?;

    Ok(())
}
