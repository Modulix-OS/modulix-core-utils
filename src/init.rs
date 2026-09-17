use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::LOCK_SKIP_REBUILD_FILE;
use crate::core::transaction::transaction::TransactionPermission;
use crate::core::transaction::transaction::{BuildCommand, UpdateInput};
use crate::{CONFIG_DIRECTORY, filesystem, hardware_config, locale, mx, user};
use std::path::{Component, Path};
use std::{fs, process};

const DEFAULT_SHELL: &str = "/run/current-system/sw/bin/bash";

const FLAKE_FILE: &str = concat!(
    r#"{
  description = "Modulix OS";
  inputs = {
    mxpkgs.url = "github:Modulix-OS/mxpkgs";
    nixos-hardware.url = "github:NixOS/nixos-hardware";
  };
  outputs = { self, mxpkgs, nixos-hardware, ... }@inputs: {
    nixosConfigurations =
    {
      "default" = mxpkgs.lib.modulixosSystem
      {
        system = ""#,
    env!("TARGET_NIX"),
    r#"";
        specialArgs = { inherit nixos-hardware; };
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
}
"#;

/// Recursively empties `path`, then removes it.
///
/// Entry kinds are read with [`std::fs::DirEntry::file_type`], which does not
/// follow symlinks — deliberately. A config directory routinely holds symlinks
/// into the read-only Nix store (`environment.etc` entries, a `result` from
/// `build-vm`); following them would make [`NixFile::delete`] clear the
/// immutable flag on the *target*, which fails with `EPERM` on the store and
/// aborts the whole init. Only regular files go through `NixFile::delete`,
/// which is the sole case where an immutable flag can be in the way.
fn remove_dir_recursive(path: &Path) -> mx::Result<()> {
    for entry in fs::read_dir(path).map_err(mx::ErrorKind::IOError)? {
        let entry = entry.map_err(mx::ErrorKind::IOError)?;
        let entry_path = entry.path();
        let file_type = entry.file_type().map_err(mx::ErrorKind::IOError)?;
        if file_type.is_dir() {
            remove_dir_recursive(&entry_path)?;
        } else if file_type.is_file() {
            NixFile::delete(entry_path.to_str().ok_or(mx::ErrorKind::InvalidFile)?)?;
        } else {
            fs::remove_file(&entry_path).map_err(mx::ErrorKind::IOError)?;
        }
    }
    fs::remove_dir(path).map_err(mx::ErrorKind::IOError)?;
    Ok(())
}

/// Resolves the NixOS config repo path.
///
/// `config_dir` takes priority when given (normalized with a trailing `/`).
/// Otherwise: `root_path + "/" + CONFIG_DIRECTORY` in release (`CONFIG_DIRECTORY`
/// is relative to the install root there), or `CONFIG_DIRECTORY` alone in debug
/// (it is already an absolute fixed test path there, so prefixing `root_path`
/// produces a nonexistent nested path).
fn resolve_config_path(root_path: &str, config_dir: Option<&str>) -> String {
    if let Some(dir) = config_dir {
        let mut d = dir.to_string();
        if !d.ends_with('/') {
            d.push('/');
        }
        return d;
    }
    #[cfg(debug_assertions)]
    {
        let _ = root_path;
        CONFIG_DIRECTORY.to_string()
    }
    #[cfg(not(debug_assertions))]
    {
        root_path.to_owned() + "/" + CONFIG_DIRECTORY
    }
}

pub fn init_repo(root_path: &str) -> mx::Result<()> {
    let path_config = resolve_config_path(root_path, None);
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

/// Mirrors the Nix option `mx.desktop`, an `enum [ "gnome" "plasma" "lxqt" "cli" ]`
/// declared by mxpkgs (`modulixos/desktop/default.nix`). Selecting a desktop is the
/// only thing the generated config has to say about the graphical stack: mxpkgs
/// derives `services.xserver`, the video drivers and the display manager from it.
pub enum Desktop {
    Gnome,
    Plasma,
    Lxqt,
    Cli,
}

impl Desktop {
    pub fn as_str(&self) -> &str {
        match self {
            Desktop::Gnome => "gnome",
            Desktop::Plasma => "plasma",
            Desktop::Lxqt => "lxqt",
            Desktop::Cli => "cli",
        }
    }

    /// Rejects anything the Nix enum would reject, rather than letting an unknown
    /// name through and silently producing a headless system.
    pub fn parse(name: &str) -> mx::Result<Self> {
        match name {
            "gnome" => Ok(Desktop::Gnome),
            "plasma" => Ok(Desktop::Plasma),
            "lxqt" => Ok(Desktop::Lxqt),
            "cli" => Ok(Desktop::Cli),
            other => Err(mx::ErrorKind::InvalidArgument(format!(
                "unknown desktop environment '{other}' (expected gnome, plasma, lxqt or cli)"
            ))),
        }
    }
}

pub struct InitParams {
    pub root: String,
    pub hostname: String,
    pub username: String,
    pub full_name: String,
    pub desktop: Desktop,
    pub locale: String,
    pub timezone: String,
    pub kb_layout: String,
    pub kb_variant: String,
    pub console_keymap: String,
    /// Overrides where the config repo is written (see [`resolve_config_path`]).
    pub config_dir: Option<String>,
    /// Debug/test mode: seeds the repo with `nixos-rebuild build-vm` instead of
    /// `nixos-install`/`switch`, and skips the skip-rebuild lock so the build
    /// actually runs.
    pub debug: bool,
}

fn config_path(params: &InitParams) -> String {
    resolve_config_path(&params.root, params.config_dir.as_deref())
}

/// Renders `configuration.nix`.
///
/// The `imports` list is spelled out here rather than left to
/// `Transaction::begin`: `begin` injects newly created files into the `imports`
/// of `configuration.nix`, but [`init`] then overwrites that file's whole
/// buffer with this template, which would drop the injection. [`init`] always
/// creates `locale.nix` and `users.nix`, so listing them is exact.
fn configuration_nix(p: &InitParams) -> String {
    format!(
        r#"{{ config, lib, pkgs, ... }}:
{{
  imports = [
    ./hardware-configuration.nix
    ./fstab.nix
    ./locale.nix
    ./users.nix
  ];

  networking.hostName = "{hostname}";

  mx.desktop = "{desktop}";
}}
"#,
        hostname = p.hostname,
        desktop = p.desktop.as_str(),
    )
}

/// Files the config repo is built from. Every one of them is written by
/// [`init`] and must end up immutable, so a stray editor or script cannot
/// change the system configuration outside a transaction.
///
/// `flake.lock` is in the list even though no [`NixFile`] owns it: it is
/// produced by `nix flake update` during the commit, and would otherwise be
/// the one writable file left behind.
const BASE_FILES: &[&str] = &[
    "flake.nix",
    "flake.lock",
    "configuration.nix",
    "hardware-configuration.nix",
    "fstab.nix",
    locale::LOCALE_FILE_PATH,
    user::USER_FILE_PATH,
];

/// Creates the cache directory and keeps git from ever seeing it.
///
/// The indexes are rebuilt artifacts living inside the config repo. Without
/// the exclude, `Transaction::begin` (which stashes with `INCLUDE_UNTRACKED`)
/// would stash the whole directory away for the duration of every build —
/// ignored paths are left alone, untracked ones are not. `result` gets the
/// same treatment: `nixos-rebuild build-vm` may drop it here.
fn init_cache_dir(repo_path: &Path) -> mx::Result<()> {
    fs::create_dir_all(repo_path.join(crate::CACHE_DIRECTORY_NAME))
        .map_err(mx::ErrorKind::IOError)?;

    let info_dir = repo_path.join(".git/info");
    fs::create_dir_all(&info_dir).map_err(mx::ErrorKind::IOError)?;
    fs::write(
        info_dir.join("exclude"),
        format!("{}/\nresult\n", crate::CACHE_DIRECTORY_NAME),
    )
    .map_err(mx::ErrorKind::IOError)
}

/// Re-applies the immutable flag to every file in [`BASE_FILES`].
///
/// `NixFile::commit` already does it for the files it owns, but only those:
/// this catches `flake.lock` and any file a future step adds outside a
/// transaction. Missing files are skipped — the set is a superset of what a
/// given run writes. No-op on files not owned by root (dev checkouts).
fn seal_base_files(path_config: &str) -> mx::Result<()> {
    for name in BASE_FILES {
        let path = Path::new(path_config).join(name);
        if path.is_file() {
            NixFile::make_immutable(path.to_str().ok_or(mx::ErrorKind::InvalidFile)?)?;
        }
    }
    Ok(())
}

fn hold_skip_rebuild_lock() -> mx::Result<fs::File> {
    let file = fs::File::create(LOCK_SKIP_REBUILD_FILE)
        .map_err(|e| crate::error::io_error_at(LOCK_SKIP_REBUILD_FILE, e))?;
    file.try_lock().map_err(|_| mx::ErrorKind::FailToLock)?;
    Ok(file)
}

/// Rejects a config path that [`remove_dir_recursive`] must never be handed.
///
/// [`init`] deletes `path_config` outright before recreating it, and runs as
/// root during an install, so a path normalizing to the filesystem root would
/// wipe the system. Only the *resolved* path can be checked:
/// [`resolve_config_path`] appends a trailing `/`, which turns `--config-dir ""`
/// into `"/"`, so the raw argument says nothing. `..` components are refused
/// too — they make the deleted directory something other than what the string
/// reads as (`/etc/nixos/..` is `/etc`).
fn validate_config_path(path_config: &str) -> mx::Result<()> {
    let mut has_name = false;
    for component in Path::new(path_config).components() {
        match component {
            Component::Normal(_) => has_name = true,
            Component::ParentDir => return Err(mx::ErrorKind::InvalidFile),
            _ => {}
        }
    }
    if has_name {
        Ok(())
    } else {
        Err(mx::ErrorKind::InvalidFile)
    }
}

pub fn init(params: &InitParams) -> mx::Result<()> {
    let path_config = config_path(params);
    validate_config_path(&path_config)?;
    let repo_path = Path::new(path_config.as_str());
    if repo_path.exists() {
        remove_dir_recursive(repo_path)?;
    }
    fs::create_dir_all(repo_path).map_err(mx::ErrorKind::IOError)?;

    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(repo_path, &opts).map_err(mx::ErrorKind::GitError)?;

    init_cache_dir(repo_path)?;

    let fstab = format!(
        "{{config, lib, pkgs, ...}}:\n{{\n{}\n}}\n",
        filesystem::get_filesystem_from_fstab(&params.root)?
    );
    let config = configuration_nix(params);

    // In debug mode the build actually runs (nixos-rebuild build-vm): don't
    // skip it. The real installer path (Calamares / modulixos-installer)
    // keeps holding the lock so the rebuild queue is skipped there.
    let _skip_rebuild = if params.debug {
        None
    } else {
        Some(hold_skip_rebuild_lock()?)
    };

    let build_command = if params.debug {
        BuildCommand::BuildVm
    } else {
        BuildCommand::Boot
    };

    let mut tx = Transaction::new(
        &path_config,
        "modulix init",
        build_command,
        TransactionPermission::Writtable,
    )?;

    const HARDWARE_FILE: &str = "hardware-configuration.nix";
    for f in [
        "flake.nix",
        HARDWARE_FILE,
        "fstab.nix",
        locale::LOCALE_FILE_PATH,
        user::USER_FILE_PATH,
    ] {
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
            if let Err(e) =
                hardware_config::write_hardware_config_no_transaction(&params.root, file)
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

    // Locale and keyboard: folded into the init transaction so the seed repo carries
    // them from the first commit, instead of a separate transaction + rebuild. The
    // X11 layout goes here rather than in `configuration.nix` so it is written
    // whatever `mx.desktop` is, and so it stays editable afterwards through
    // `locale::set_keyboard`.
    match tx.get_file_mut(locale::LOCALE_FILE_PATH) {
        Ok(file) => {
            let written = locale::set_locale_no_transaction(
                file,
                &params.timezone,
                &params.locale,
                &params.console_keymap,
            )
            .and_then(|()| {
                locale::set_keyboard_no_transaction(file, &params.kb_layout, &params.kb_variant)
            });
            if let Err(e) = written {
                tx.rollback()?;
                return Err(e);
            }
        }
        Err(e) => {
            tx.rollback()?;
            return Err(e);
        }
    }

    // User: same reasoning as locale above.
    match tx.get_file_mut(user::USER_FILE_PATH) {
        Ok(file) => {
            if let Err(e) = user::add_no_transaction(
                file,
                &params.username,
                "",
                &params.full_name,
                DEFAULT_SHELL,
                &["wheel", "networkmanager"],
                true,
            ) {
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

    seal_base_files(&path_config)?;

    Ok(())
}
