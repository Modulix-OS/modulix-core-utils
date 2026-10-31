//! Bootstraps the NixOS configuration repository for a brand-new Modulix
//! system: `init_repo` is used by the installer to seed a minimal repo
//! ahead of hardware detection, and `init` is the installer's actual
//! engine, writing every base file (flake, configuration, hardware,
//! detected `mx.hardware.*` options, fstab, locale, user) in a single
//! transaction and sealing them immutable.
//!
//! Only [`init`] writes `hardware.nix`. [`init_repo`] deliberately does not:
//! its own `CONFIG_FILE` template imports nothing but
//! `hardware-configuration.nix` and `fstab.nix`, and adding the import
//! without also registering the file in its transaction would leave a
//! configuration importing a file that does not exist.
//!
//! Everything here is local-only: no network fetch happens (in particular
//! `crate::REMOTE_CONFIG_URL` is declared in `lib.rs` but never used by
//! this module — `flake.nix`/`configuration.nix` are rendered from the
//! `FLAKE_FILE`/`CONFIG_FILE` templates and `nixos-generate-config`,
//! not downloaded). [`init`] lists `result` and the legacy cache directory in
//! the repo's `.git/info/exclude` so a build artifact is never stashed or
//! committed. Whether a call targets the live system (`/`) or
//! an installation root (e.g. `/mnt`) is entirely up to the `root_path`/
//! `params.root` argument passed in; this module does not choose it.
//! Root privileges are required in release builds only where the target
//! paths (`/etc/modulix-os/` via [`crate::CONFIG_DIRECTORY`]) are
//! root-owned and where `nixos-install`/`nixos-generate-config` need them;
//! in debug builds every rebuild is forced to `build-vm`
//! (`BuildCommand::as_str`), so a debug run never touches the host system.

use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::TransactionPermission;
use crate::core::transaction::transaction::{BuildCommand, UpdateInput};
use crate::core::transaction::{Transaction, TransactionOp, make_group_transaction};
use crate::detect_hardware::system_info::gpu_info::HardwareTopology;
use crate::detect_os::OtherOs;
use crate::error::io_message;
use crate::{
    CONFIG_DIRECTORY, boot_config, detect_os, filesystem, hardware_config, install_module,
    install_package, locale, mx, secure_boot, user,
};
use std::path::{Component, Path};
use std::{fs, process};

/// Login shell assigned to the user created by [`init`].
const DEFAULT_SHELL: &str = "/run/current-system/sw/bin/bash";

/// Template written to `flake.nix` by both [`init_repo`] and [`init`].
///
/// Declares the `mxpkgs`/`nixos-hardware` flake inputs and one
/// `nixosConfigurations.default` output (see [`crate::CONFIG_NAME`])
/// built with `mxpkgs.lib.modulixosSystem`, importing `./configuration.nix`.
/// The Nix `system` string is baked in at compile time from the
/// `TARGET_NIX` build-time env var (set by `build.rs`).
const FLAKE_FILE: &str = concat!(
    r#"{
  description = "Modulix OS";
  inputs = {
    mxpkgs.url = "github:Modulix-OS/mxpkgs";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
    mxpkgs.inputs.nixpkgs-unstable.follows = "nixpkgs-unstable";
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

/// Minimal `configuration.nix` written by [`init_repo`] only.
///
/// Imports `hardware-configuration.nix` and `fstab.nix`; unlike the
/// version [`init`] renders (see [`configuration_nix`]), it sets no
/// `networking.hostName` or `mx.desktop` and does not import
/// `locale.nix`/`users.nix`, since [`init_repo`] does not create those
/// files.
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
/// which is the sole case where an immutable flag can be in the way. Symlinks
/// and other non-regular, non-directory entries are removed directly with
/// `fs::remove_file`, without touching the immutable flag.
///
/// # Parameters
/// * `path` - directory to empty and remove. Must exist and be readable.
///
/// # Pre-conditions
/// `path` exists and is a directory the caller has permission to list and
/// modify.
///
/// # Post-conditions
/// On success, `path` and everything under it no longer exist on disk.
/// Regular files that were immutable (root-owned) have their immutable flag
/// cleared before deletion via [`NixFile::delete`].
///
/// # Returns
/// `Ok(())` once `path` itself has been removed.
///
/// # Errors
/// * `mx::ErrorKind::IOError` - reading a directory entry, its file type, or
///   removing a non-regular entry/the now-empty directory failed.
/// * Any error [`NixFile::delete`] returns (e.g. `mx::ErrorKind::InvalidFile`
///   if an entry's path is not valid UTF-8, or an I/O error clearing the
///   immutable flag / unlinking) for regular files.
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
///
/// # Parameters
/// * `root_path` - installation root the config repo is nested under in
///   release builds (`/` for the live system, or an install root such as
///   `/mnt`). Ignored in debug builds.
/// * `config_dir` - explicit override for the config repo path, bypassing
///   both `root_path` and [`crate::CONFIG_DIRECTORY`] when given.
///
/// # Returns
/// The resolved config repo directory path, always ending in `/`.
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

/// Seeds a minimal NixOS config repo ahead of the full [`init`] flow (or
/// re-adopts one that already exists as a git repo).
///
/// Resolves the repo path with `resolve_config_path` (no `config_dir`
/// override — always driven by `root_path`/[`crate::CONFIG_DIRECTORY`]). If
/// the directory already exists it is wiped with `remove_dir_recursive`
/// first, then recreated. If it is already a git repository, the function
/// returns immediately without touching its contents. Otherwise it
/// `git init`s the directory (initial branch `main`), runs
/// `nixos-generate-config --show-hardware-config --no-filesystems`
/// (with `--root root_path` unless `root_path` is `/`) to capture the
/// hardware config without a filesystem section, builds `fstab.nix` from
/// `filesystem::fstab_module`, and commits `flake.nix`
/// (`FLAKE_FILE`), `configuration.nix` (`CONFIG_FILE`),
/// `hardware-configuration.nix` and `fstab.nix` in one `Transaction`
/// (`BuildCommand::Install` in release, `BuildCommand::Boot` in debug —
/// both resolve to `build-vm` in debug builds) with
/// `UpdateInput::UpdateAll`, so `flake.lock` is generated fresh.
///
/// Unlike [`init`], this does not create the `.cache` directory and does not
/// call `seal_base_files`
/// afterwards (files are sealed immutable by `Transaction::commit`/
/// `NixFile::commit` for the files it owns, but `flake.lock` is left as
/// written by `nix flake update`).
///
/// # Parameters
/// * `root_path` - installation root (`/` for the live system, or an
///   install root such as `/mnt`) forwarded to `resolve_config_path` and
///   to `nixos-generate-config --root`.
///
/// # Pre-conditions
/// Caller has permission to create/wipe the resolved config directory, and
/// (in release builds, for a non-`/` `root_path`) `nixos-generate-config`
/// can read that root. Requires root in release builds against `/etc/`
/// paths.
///
/// # Post-conditions
/// On success, the resolved config directory is a git repository on branch
/// `main` containing at least `flake.nix`, `configuration.nix`,
/// `hardware-configuration.nix`, `fstab.nix` and a generated `flake.lock`,
/// committed as "initial commit" (or is left untouched if it was already a
/// git repo). On failure partway through the transaction, the transaction
/// is rolled back (files it created are removed, pre-existing ones restored)
/// per `Transaction::rollback`; the directory itself is not removed again.
///
/// # Returns
/// `Ok(())` once the repo exists and, when freshly created, the initial
/// commit has been made.
///
/// # Errors
/// * `mx::ErrorKind::IOError` - removing/creating the directory, or other I/O
///   failed.
/// * `mx::ErrorKind::NixCommandError` - `nixos-generate-config` could not be
///   spawned or exited non-zero (the message carries its stderr).
/// * `mx::ErrorKind::InvalidFile` - `nixos-generate-config`'s stdout is not
///   valid UTF-8, or a path could not be converted (via
///   `remove_dir_recursive`).
/// * `mx::ErrorKind::GitError` - `git2` failed to open or init the repo.
/// * Any error from `filesystem::fstab_module`,
///   `Transaction::new`, `Transaction::add_file`, `Transaction::begin`,
///   `Transaction::get_file_mut`, or `Transaction::commit`.
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

    let fs = filesystem::fstab_module(root_path)?;

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
///
/// # Variants
/// * `Gnome` - GNOME desktop (`mx.desktop = "gnome"`).
/// * `Plasma` - KDE Plasma desktop (`mx.desktop = "plasma"`).
/// * `Lxqt` - LXQt desktop (`mx.desktop = "lxqt"`).
/// * `Cli` - no graphical stack, headless system (`mx.desktop = "cli"`).
pub enum Desktop {
    Gnome,
    Plasma,
    Lxqt,
    Cli,
}

impl Desktop {
    /// Renders the variant as the Nix string literal's inner value.
    ///
    /// # Returns
    /// `"gnome"`, `"plasma"`, `"lxqt"` or `"cli"`, matching the variant.
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
    ///
    /// # Parameters
    /// * `name` - candidate desktop name, expected to be one of `"gnome"`,
    ///   `"plasma"`, `"lxqt"`, `"cli"`.
    ///
    /// # Returns
    /// The matching [`Desktop`] variant.
    ///
    /// # Errors
    /// * `mx::ErrorKind::InvalidArgument` - `name` is none of the four known
    ///   desktop names.
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

/// Every user-facing parameter [`init`] needs to seed a first-boot config.
///
/// # Fields
/// * `root` - installation root passed to `resolve_config_path`,
///   `filesystem::fstab_module` and
///   `hardware_config::write_hardware_config_no_transaction` (`/` for the
///   live system, or an install root such as `/mnt`).
/// * `hostname` - value written to `networking.hostName` in
///   `configuration.nix` (see `configuration_nix`).
/// * `username` - login name of the user created via
///   `user::add_no_transaction`.
/// * `full_name` - `description` (GECOS full name) of that user.
/// * `desktop` - selected desktop, written to `mx.desktop` (see [`Desktop`]).
/// * `locale` - default locale passed to `locale::set_locale_no_transaction`.
/// * `timezone` - timezone passed to `locale::set_locale_no_transaction`.
/// * `kb_layout` - X11 keyboard layout passed to
///   `locale::set_keyboard_no_transaction`.
/// * `kb_variant` - X11 keyboard variant passed to
///   `locale::set_keyboard_no_transaction`.
/// * `console_keymap` - console keymap passed to
///   `locale::set_locale_no_transaction`.
/// * `packages` - nixpkgs attribute names seeded into `package.nix`.
/// * `modules` - Modulix module names seeded into `module.nix`.
/// * `luks` - the LUKS containers of the target, root first.
/// * `resume_device` - swap device hibernation resumes from, when the target
///   is set up for hibernation.
/// * `secure_boot` - whether to enable Secure Boot when the firmware allows
///   it.
/// * `config_dir` - overrides where the config repo is written (see
///   `resolve_config_path`).
/// * `debug` - debug/test mode: the seed commit runs `nixos-rebuild build-vm`
///   instead of `nixos-install`, so it installs nothing.
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
    /// nixpkgs attribute names seeded into `package.nix` through
    /// `install_package::install_no_transaction`, so they carry the same
    /// `pkgs.<attr>` spelling a later `install_package::uninstall` matches on.
    /// Empty creates no file and adds no import.
    pub packages: Vec<String>,
    /// Modulix module names, dotted as in mxpkgs' `modules/index.json`
    /// (e.g. `services.flatpak`), each seeded into `module.nix` as
    /// `mx.<name>.enable = true`. Not validated against that index — an
    /// unknown name only fails at build time. Empty creates no file and adds
    /// no import.
    pub modules: Vec<String>,
    /// LUKS containers of the target, empty when it is not encrypted. The
    /// root container only needs `crypttabExtraOpts`, since
    /// `nixos-generate-config` already declares its `.device`; a swap-only
    /// container needs both, because the generator never reports it (see
    /// `LuksInit`).
    pub luks: Vec<LuksInit>,
    /// Swap device `boot.resumeDevice` is set to, when the target is set up
    /// for hibernation. `None` writes nothing, which leaves hibernation unable
    /// to find its image: with a systemd initrd NixOS only passes `resume=`
    /// when this option is set.
    pub resume_device: Option<String>,
    /// Whether to enable Secure Boot when the firmware allows it. Enabling
    /// is still conditional on the firmware being in Setup Mode, which
    /// [`init`] probes itself through
    /// [`crate::secure_boot::firmware_state`] - keys cannot be enrolled in
    /// any other state, and nixpkgs' limine installer ignores the failure.
    /// This flag only lets a caller opt out entirely.
    pub secure_boot: bool,
    /// Overrides where the config repo is written (see `resolve_config_path`).
    pub config_dir: Option<String>,
    /// Debug/test mode: the seed commit runs `nixos-rebuild build-vm` instead
    /// of `nixos-install`, so it installs nothing.
    pub debug: bool,
}

/// One LUKS container of an install target, for the LUKS facts
/// `nixos-generate-config` cannot report.
///
/// # Fields
/// * `name` - mapper name the container is currently opened as. The generator
///   reads it straight out of `/sys/class/block/<dm>/dm/name` and keys its own
///   `boot.initrd.luks.devices` entry on it, so this must be the same name or
///   the options land on a second, unused entry.
/// * `container` - the raw container device as a `/dev/disk/by-uuid/<uuid>`
///   path, for a container the generator left out of the configuration, i.e.
///   one holding swap alone: it only emits `boot.initrd.luks.devices` entries
///   for the mount points it walks. `None` for the root container, whose
///   `.device` the generator already declared — a second definition of that
///   `types.str` option makes the NixOS module system fail even at equal
///   values.
/// * `tpm2` - a TPM2 token is enrolled on the container, so
///   `crypttabExtraOpts = [ "tpm2-device=auto" ]` is added to that entry.
pub struct LuksInit {
    pub name: String,
    pub container: Option<String>,
    pub tpm2: bool,
}

/// Resolves the config repo path for an [`init`] call.
///
/// Thin wrapper over [`resolve_config_path`] extracting `root`/`config_dir`
/// from `params`.
///
/// # Parameters
/// * `params` - init parameters; only `root` and `config_dir` are used.
///
/// # Returns
/// The resolved config repo directory path, always ending in `/`.
fn config_path(params: &InitParams) -> String {
    resolve_config_path(&params.root, params.config_dir.as_deref())
}

/// Renders `configuration.nix`.
///
/// The `imports` list is spelled out here rather than left to
/// `Transaction::begin`: `begin` injects newly created files into the `imports`
/// of `configuration.nix`, but [`init`] then overwrites that file's whole
/// buffer with this template, which would drop the injection. [`init`] always
/// creates `hardware.nix`, `locale.nix` and `users.nix`, so listing them is
/// exact; it creates `package.nix` and `module.nix` only for a non-empty
/// `packages`/`modules`, and `boot.nix` only when there is something to put in
/// it — another operating system was detected, or Secure Boot is being enabled
/// — which is exactly when they are listed here.
///
/// # Parameters
/// * `p` - init parameters; `hostname` and `desktop` are interpolated into
///   the template, and `packages`/`modules` decide two of the optional
///   imports.
/// * `with_boot_file` - whether `boot.nix` is written at all, i.e. whether the
///   detection found another operating system or Secure Boot is being
///   enabled. Passed in rather than read off `p`, since neither the detection
///   nor the firmware state is an init parameter.
///
/// # Returns
/// The full `configuration.nix` source, importing
/// `hardware-configuration.nix`, `hardware.nix`, `fstab.nix`, `locale.nix`,
/// `users.nix` and — when there is something to put in them — `package.nix`,
/// `module.nix` and `boot.nix`, and setting `networking.hostName` and
/// `mx.desktop`.
fn configuration_nix(p: &InitParams, with_boot_file: bool) -> String {
    let mut extra = String::new();
    if !p.packages.is_empty() {
        extra.push_str("    ./");
        extra.push_str(install_package::FILE_PACKAGE_PATH);
        extra.push('\n');
    }
    if !p.modules.is_empty() {
        extra.push_str("    ./");
        extra.push_str(install_module::FILE_MODULE_PATH);
        extra.push('\n');
    }
    if with_boot_file {
        extra.push_str("    ./");
        extra.push_str(boot_config::FILE_BOOT_PATH);
        extra.push('\n');
    }
    format!(
        r#"{{ config, lib, pkgs, ... }}:
{{
  imports = [
    ./hardware-configuration.nix
    ./{hardware}
    ./fstab.nix
    ./locale.nix
    ./users.nix
{extra}  ];

  networking.hostName = "{hostname}";

  mx.desktop = "{desktop}";
}}
"#,
        hardware = hardware_config::HARDWARE_OPTION_PATH,
        extra = extra,
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
///
/// `package.nix` and `module.nix` are deliberately absent: they are the two
/// files a user keeps editing after the install, through transactions of
/// their own, and `NixFile::create_file`/`commit` already seal them. Adding
/// them here would seal nothing extra.
///
/// `boot.nix` is in the list, unlike those two, because it is generated from
/// what the EFI partitions hold, exactly like `hardware.nix`: it is rewritten
/// by a redetection, never hand-maintained. [`seal_base_files`] skips an entry
/// that does not exist, so listing it costs nothing on a machine where no
/// other operating system was found.
const BASE_FILES: &[&str] = &[
    "flake.nix",
    "flake.lock",
    "configuration.nix",
    "hardware-configuration.nix",
    hardware_config::HARDWARE_OPTION_PATH,
    "fstab.nix",
    locale::LOCALE_FILE_PATH,
    user::USER_FILE_PATH,
    boot_config::FILE_BOOT_PATH,
];

/// Keeps git from ever seeing the build artifacts that may land in the config
/// repo.
///
/// `Transaction::begin` stashes with `INCLUDE_UNTRACKED`, so any untracked
/// entry in the repo would be stashed away for the duration of every build -
/// ignored paths are left alone, untracked ones are not. Two entries need that
/// protection:
///
/// * `result`, the garbage-collection root `nixos-rebuild build`/`build-vm`
///   drops in its working directory, which is the repo itself for a plain
///   transaction.
/// * [`crate::LEGACY_CACHE_DIRECTORY_NAME`], which nothing writes any more -
///   the cache moved to [`crate::DEFAULT_CACHE_DIRECTORY`], outside the repo,
///   because `nix` resolved a staged build under it as a `git+file://` flake.
///   A machine installed before that move still carries the directory, and it
///   must stay invisible here.
///
/// # Parameters
/// * `repo_path` - root of the config git repo; `.git/info/exclude` is created
///   under it.
///
/// # Pre-conditions
/// `repo_path` is an existing, initialized git repository.
///
/// # Post-conditions
/// `repo_path/.git/info/exclude` exists and contains
/// `LEGACY_CACHE_DIRECTORY_NAME/` and `result`, one per line, overwriting any
/// previous content of that file. No cache directory is created: every
/// consumer of [`crate::cache_dir`] creates it on first use, and it no longer
/// lives under `repo_path`.
///
/// # Returns
/// `Ok(())` once the exclude file is written.
///
/// # Errors
/// * `mx::ErrorKind::IOError` - creating `.git/info`, or writing the exclude
///   file, failed.
fn init_git_exclude(repo_path: &Path) -> mx::Result<()> {
    let info_dir = repo_path.join(".git/info");
    fs::create_dir_all(&info_dir).map_err(mx::ErrorKind::IOError)?;
    fs::write(
        info_dir.join("exclude"),
        format!("{}/\nresult\n", crate::LEGACY_CACHE_DIRECTORY_NAME),
    )
    .map_err(mx::ErrorKind::IOError)
}

/// Re-applies the immutable flag to every file in [`BASE_FILES`].
///
/// `NixFile::commit` already does it for the files it owns, but only those:
/// this catches `flake.lock` and any file a future step adds outside a
/// transaction. Missing files are skipped — the set is a superset of what a
/// given run writes. No-op on files not owned by root (dev checkouts).
///
/// # Parameters
/// * `path_config` - config repo directory the files in [`BASE_FILES`] are
///   resolved against.
///
/// # Post-conditions
/// Every file in [`BASE_FILES`] that exists under `path_config` has the ext2
/// immutable flag set (a no-op for files not owned by root). Files absent
/// from `path_config` are left untouched.
///
/// # Returns
/// `Ok(())` once every existing file in [`BASE_FILES`] has been sealed.
///
/// # Errors
/// * Any error [`NixFile::make_immutable`] returns, e.g.
///   `mx::ErrorKind::InvalidFile` if a resolved path is not valid UTF-8, or
///   an I/O error from the underlying `ioctl`.
fn seal_base_files(path_config: &str) -> mx::Result<()> {
    for name in BASE_FILES {
        let path = Path::new(path_config).join(name);
        if path.is_file() {
            NixFile::make_immutable(path.to_str().ok_or(mx::ErrorKind::InvalidFile)?)?;
        }
    }
    Ok(())
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
///
/// # Parameters
/// * `path_config` - resolved config path (output of [`resolve_config_path`]
///   / [`config_path`]), about to be handed to [`remove_dir_recursive`].
///
/// # Returns
/// `Ok(())` if `path_config` contains at least one normal (named) path
/// component and no `..` component.
///
/// # Errors
/// * `mx::ErrorKind::InvalidFile` - `path_config` normalizes to a root-only
///   path (no named component, e.g. `"/"`), or contains a `..` component.
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

/// Adds to a generated `fstab.nix` the LUKS and hibernation facts
/// `nixos-generate-config` leaves out.
///
/// Called once per [`init`], after the file has been replaced wholesale by
/// `filesystem::fstab_module`'s output, so the entries the generator did
/// declare are already in the buffer.
///
/// # Parameters
/// * `fstab` - the open `fstab.nix` of the transaction.
/// * `params` - init parameters; only `luks` and `resume_device` are read.
///
/// # Pre-conditions
/// Every [`LuksInit::container`] that is `Some` names a container the
/// generator did *not* declare, and every `None` one it did — the generator
/// only walks mount points, so in practice the root container is `None` and a
/// swap-only container is `Some`.
///
/// # Post-conditions
/// Each container has its `.device` declared when it carried one, and
/// `crypttabExtraOpts = [ "tpm2-device=auto" ]` when it is TPM2-enrolled;
/// `boot.resumeDevice` is set when `resume_device` is `Some`. Nothing is
/// written for an empty `luks` list and a `None` `resume_device`.
///
/// # Returns
/// `Ok(())` once every edit is applied to `fstab`'s in-memory buffer; the
/// caller commits or rolls back.
///
/// # Errors
/// Any error from `filesystem::set_luks_container_no_transaction`,
/// `filesystem::set_luks_tpm2_no_transaction` or
/// `filesystem::set_resume_device_no_transaction`.
/// Scans the machine's EFI partitions for the other operating systems
/// installed on it.
///
/// Never fails: a boot menu entry is a convenience, and refusing to install a
/// system because `lsblk` is missing or an EFI partition would not mount would
/// be out of all proportion. A failed scan is reported on stderr and treated
/// as "found nothing", which only costs the extra entries.
///
/// # Parameters
/// * `params` - init parameters; only `root` is read, to locate the EFI
///   partition of the system being installed.
///
/// # Pre-conditions
/// Called before the config directory is wiped, like the hardware probe: it
/// reads nothing under it. The target's EFI partition is expected mounted at
/// `<root>/boot`, which is where the installer puts it.
///
/// # Post-conditions
/// Nothing is written anywhere; no partition is left mounted.
///
/// # Returns
/// The entries to declare in `boot.nix`, empty when nothing else is installed
/// or when the scan could not run.
fn detect_other_os(params: &InitParams) -> Vec<OtherOs> {
    let root = params.root.trim_end_matches('/');
    let esp_mount = format!("{root}/boot");

    match detect_os::detect(&esp_mount) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("init: no boot entry for the other operating systems: {e}");
            Vec::new()
        }
    }
}

fn write_fstab_extras(fstab: &mut NixFile, params: &InitParams) -> mx::Result<()> {
    for luks in &params.luks {
        if let Some(container) = &luks.container {
            filesystem::set_luks_container_no_transaction(fstab, &luks.name, container)?;
        }
        if luks.tpm2 {
            filesystem::set_luks_tpm2_no_transaction(fstab, &luks.name)?;
        }
    }

    if let Some(device) = &params.resume_device {
        filesystem::set_resume_device_no_transaction(fstab, device)?;
    }

    Ok(())
}

/// The installer's engine: creates the NixOS config repo for a brand-new
/// Modulix system from scratch and seeds it with everything a first boot
/// needs (flake, base configuration, detected hardware, filesystem table,
/// locale/keyboard, and one user).
///
/// Probes the machine's graphics topology with
/// `HardwareTopology::detect` first of all — before the path is even
/// resolved, so a detection failure leaves the existing configuration
/// untouched rather than wiping it and then failing. Then resolves the repo
/// path with `config_path` and rejects it with `validate_config_path` before
/// anything is deleted. Unlike [`init_repo`], any pre-existing directory at
/// that path — git repo or not — is always wiped with `remove_dir_recursive`
/// and replaced by a fresh `git init` (branch `main`); there is no
/// re-adoption path. Then, in order:
/// 1. `init_git_exclude` excludes `result` (and the legacy `.cache`) via
///    `.git/info/exclude`.
/// 2. `fstab.nix` is rendered from `filesystem::fstab_module`
///    and `configuration.nix` from `configuration_nix`.
/// 3. The build command is `BuildCommand::BuildVm` in debug and
///    `BuildCommand::Install` in release — the install this function seeds the
///    repository for is run by the commit itself, as
///    `nixos-install --root /mnt` (per `BuildCommand::as_str` a debug build
///    resolves either variant to `build-vm`, so a debug run never touches the
///    host system).
/// 4. Everything is written by a single
///    `core::transaction::make_group_transaction` call, one
///    `core::transaction::TransactionOp` per edit, hence one git commit and
///    one rebuild for the whole seed: `flake.nix`, `configuration.nix` and
///    `fstab.nix` get their rendered content; `hardware-configuration.nix` is
///    filled by `hardware_config::write_hardware_config_no_transaction` and
///    `hardware_config::HARDWARE_OPTION_PATH` by
///    `hardware_config::write_hardware_options_no_transaction` from the
///    topology probed up front; `locale::LOCALE_FILE_PATH` by
///    `locale::set_locale_no_transaction` then
///    `locale::set_keyboard_no_transaction`; `user::USER_FILE_PATH` by
///    `user::add_no_transaction` (`params.username`, empty initial password,
///    `params.full_name`, `DEFAULT_SHELL`, groups `["wheel",
///    "networkmanager"]`, `is_normal_user = true`). The X11 keyboard layout is
///    set there — not in `configuration.nix` — so it is written whatever
///    `params.desktop` is, and stays editable afterwards through
///    `locale::set_keyboard`. Then, conditionally: `params.packages` into
///    `package.nix` via `install_package::install_no_transaction`,
///    one operation per `params.modules` entry into `module.nix` via
///    `install_module::install_no_transaction`, the detected systems into
///    `boot_config::FILE_BOOT_PATH` via
///    `boot_config::write_boot_entries_no_transaction`, the Secure Boot
///    option into that same file via
///    `secure_boot::set_secure_boot_no_transaction` — a second operation on
///    `boot.nix`, since each writer resets only its own option — and the LUKS
///    / hibernation facts into `fstab.nix` via `write_fstab_extras` — queued
///    after that file's content write, since that one replaces the whole
///    buffer. A LUKS entry the generator already declared is not re-declared:
///    a second definition of the same option would make the NixOS module
///    system fail. Any failure rolls the whole group back.
/// 5. The commit refreshes `flake.lock` (`UpdateInput::UpdateAll`).
/// 6. `seal_base_files` re-applies the immutable flag to every file in
///    `BASE_FILES` that exists. `package.nix` and `module.nix` are not in
///    that list and do not need to be — see `BASE_FILES`.
///
/// # Parameters
/// * `params` - see [`InitParams`] for the meaning of every field.
///
/// # Pre-conditions
/// Caller has permission to wipe/create the resolved config directory. In
/// release builds this normally requires root (target paths live under
/// [`crate::CONFIG_DIRECTORY`] / `params.root`); `nixos-generate-config`
/// (invoked indirectly through
/// `hardware_config::write_hardware_config_no_transaction`) must be able to
/// read `params.root`. In release builds `params.root` must be `/mnt`, the
/// root `BuildCommand::Install` installs into, and the caller must not run an
/// install of its own: the commit runs it.
///
/// The hardware probe reads the **live** machine and ignores `params.root`,
/// as `DriverConfig::new` already does: an installer running from a live
/// medium is on the hardware it is installing onto. Installing onto a disk
/// that will then be moved elsewhere therefore seeds the installing machine's
/// `hardware.nix`, which `hardware_config::redetect_hardware_options` fixes on
/// first boot. `lspci` on `PATH` is what names each GPU's generation; without
/// it every `generation` is left unset and nothing else is lost.
///
/// # Post-conditions
/// On success: the resolved config directory is a freshly initialized git
/// repo on branch `main`, containing `flake.nix`, `configuration.nix`,
/// `hardware-configuration.nix`, `hardware.nix`, `fstab.nix`, the locale and
/// user files, `package.nix`/`module.nix` when their list was non-empty, and
/// `flake.lock`, all committed as `"modulix init"`; every file in
/// `BASE_FILES` present on disk is immutable; `.cache` exists and is
/// listed in `.git/info/exclude` together with `result`. The commit has run
/// the rebuild: `nixos-install --root /mnt` in release, `nixos-rebuild
/// build-vm` in debug.
///
/// On failure: if the failure happens before the group opens its transaction,
/// the directory may already have been wiped and recreated (and, if reached,
/// `.cache`/the exclude file already written) with no commit made. Once the
/// transaction is open, any failing operation — and any failing commit — rolls
/// the whole group back, so the repository is left at its previous commit with
/// no file lock held. `seal_base_files` is only reached after a successful
/// commit.
///
/// # Returns
/// `Ok(())` once the repo has been created, seeded, committed and sealed.
///
/// # Errors
/// * `mx::ErrorKind::InvalidFile` - `validate_config_path` rejected the
///   resolved path, or a later path-to-`str` conversion failed.
/// * `mx::ErrorKind::IOError` - directory removal/creation, or any file I/O
///   in `init_git_exclude`/`seal_base_files`, failed.
/// * `mx::ErrorKind::GitError` - `git2` failed to init the repo.
/// * `mx::ErrorKind::BuildError` - the install/rebuild the commit runs exited
///   non-zero; the message carries its standard error.
/// * Any error from `HardwareTopology::detect`, `filesystem::fstab_module`,
///   `hardware_config::write_hardware_config_no_transaction`,
///   `hardware_config::write_hardware_options_no_transaction`,
///   `locale::set_locale_no_transaction`,
///   `locale::set_keyboard_no_transaction`, `user::add_no_transaction`,
///   `install_package::install_no_transaction`,
///   `install_module::install_no_transaction`,
///   `boot_config::write_boot_entries_no_transaction`,
///   `secure_boot::set_secure_boot_no_transaction`, `write_fstab_extras`,
///   `NixFile::get_mut_file_content`, or
///   `core::transaction::make_group_transaction`.
pub fn init(params: &InitParams) -> mx::Result<()> {
    // Before anything is deleted: nothing this probe reads depends on the
    // config directory, so failing here must leave the existing configuration
    // untouched rather than wipe it first.
    let hardware = HardwareTopology::detect()?;
    let other_os = detect_other_os(params);

    // Keys can only be enrolled while the firmware has no Platform Key, and
    // the limine installer ignores a failed enrollment, so a firmware that is
    // not in Setup Mode - or whose state could not be read at all - means the
    // option is not declared rather than declared and silently ineffective.
    let secure_boot = params.secure_boot
        && matches!(
            secure_boot::firmware_state(),
            Ok(secure_boot::FirmwareState::SetupMode)
        );

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

    init_git_exclude(repo_path)?;

    let fstab = filesystem::fstab_module(&params.root)?;
    let config = configuration_nix(params, !other_os.is_empty() || secure_boot);

    let build_command = if params.debug {
        BuildCommand::BuildVm
    } else {
        BuildCommand::Install
    };

    let packages: Vec<&str> = params.packages.iter().map(String::as_str).collect();

    let mut ops = vec![
        TransactionOp::new("seed flake.nix", "flake.nix", |file| {
            *file.get_mut_file_content()? = FLAKE_FILE.to_string();
            Ok(())
        }),
        TransactionOp::new("seed configuration.nix", "configuration.nix", move |file| {
            *file.get_mut_file_content()? = config;
            Ok(())
        }),
        TransactionOp::new("seed fstab.nix", "fstab.nix", move |file| {
            *file.get_mut_file_content()? = fstab;
            Ok(())
        }),
        TransactionOp::new(
            "write hardware configuration",
            hardware_config::HARDWARE_CONFIG_PATH,
            |file| hardware_config::write_hardware_config_no_transaction(&params.root, file),
        ),
        TransactionOp::new(
            "write detected hardware options",
            hardware_config::HARDWARE_OPTION_PATH,
            |file| hardware_config::write_hardware_options_no_transaction(file, &hardware),
        ),
        TransactionOp::new(
            "set locale and keyboard",
            locale::LOCALE_FILE_PATH,
            |file| {
                locale::set_locale_no_transaction(
                    file,
                    &params.timezone,
                    &params.locale,
                    &params.console_keymap,
                )?;
                locale::set_keyboard_no_transaction(file, &params.kb_layout, &params.kb_variant)
            },
        ),
        TransactionOp::new(
            &format!("create user {}", params.username),
            user::USER_FILE_PATH,
            |file| {
                user::add_no_transaction(
                    file,
                    &params.username,
                    "",
                    &params.full_name,
                    DEFAULT_SHELL,
                    &["wheel", "networkmanager"],
                    true,
                )
            },
        ),
    ];

    // Queued after the content write above, which replaces `fstab.nix`
    // wholesale with the generator's output — the entries these options attach
    // to are in there.
    if !params.luks.is_empty() || params.resume_device.is_some() {
        ops.push(TransactionOp::new(
            "declare LUKS and hibernation facts",
            "fstab.nix",
            |file| write_fstab_extras(file, params),
        ));
    }

    if !packages.is_empty() {
        ops.push(TransactionOp::new(
            "install seed packages",
            install_package::FILE_PACKAGE_PATH,
            move |file| install_package::install_no_transaction(file, &packages),
        ));
    }

    for module in &params.modules {
        ops.push(TransactionOp::new(
            &format!("enable module {}", module),
            install_module::FILE_MODULE_PATH,
            move |file| install_module::install_no_transaction(file, module),
        ));
    }

    if !other_os.is_empty() {
        ops.push(TransactionOp::new(
            "declare the other systems' boot entries",
            boot_config::FILE_BOOT_PATH,
            move |file| boot_config::write_boot_entries_no_transaction(file, &other_os),
        ));
    }

    // A second operation on the same file rather than one combined writer:
    // the two options are independent, each writer resets only its own, and
    // `make_group_transaction` lets a later operation see the earlier edits.
    if secure_boot {
        ops.push(TransactionOp::new(
            "enable secure boot",
            secure_boot::FILE_SECURE_BOOT_PATH,
            |file| secure_boot::set_secure_boot_no_transaction(file, true),
        ));
    }

    make_group_transaction(&path_config, build_command, UpdateInput::UpdateAll, ops)?;

    seal_base_files(&path_config)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_FILES, Desktop, InitParams, boot_config, configuration_nix, hardware_config,
        secure_boot,
    };

    /// `InitParams` with every optional list empty, as a starting point the
    /// tests below only change one field of.
    fn params() -> InitParams {
        InitParams {
            root: "/mnt".to_string(),
            hostname: "modulixos".to_string(),
            username: "user".to_string(),
            full_name: "User".to_string(),
            desktop: Desktop::Gnome,
            locale: "fr_FR.UTF-8".to_string(),
            timezone: "Europe/Paris".to_string(),
            kb_layout: "fr".to_string(),
            kb_variant: String::new(),
            console_keymap: "fr".to_string(),
            packages: Vec::new(),
            modules: Vec::new(),
            luks: Vec::new(),
            resume_device: None,
            secure_boot: false,
            config_dir: None,
            debug: true,
        }
    }

    #[test]
    fn empty_lists_import_neither_optional_file() {
        let rendered = configuration_nix(&params(), false);
        assert!(!rendered.contains("./package.nix"));
        assert!(!rendered.contains("./module.nix"));
        assert!(rendered.contains("./users.nix"));
    }

    /// `init` always writes `hardware.nix`, so the import is unconditional -
    /// and it has to land inside the `imports` list, not after it.
    #[test]
    fn the_hardware_options_file_is_always_imported() {
        let rendered = configuration_nix(&params(), false);
        let import = format!("./{}", hardware_config::HARDWARE_OPTION_PATH);
        let position = rendered.find(&import).expect("import missing");
        let closing = rendered.find("];").expect("imports list unterminated");
        assert!(position < closing, "outside the imports list:\n{rendered}");
    }

    /// The import list and the sealing list have to name the same file: an
    /// imported file left mutable is a configuration editable outside a
    /// transaction.
    #[test]
    fn the_hardware_options_file_is_sealed() {
        assert!(BASE_FILES.contains(&hardware_config::HARDWARE_OPTION_PATH));
    }

    /// `boot.nix` only exists when there is something to put in it, so its
    /// import must follow the detection and the firmware state, not the
    /// parameters.
    #[test]
    fn a_detected_system_adds_the_boot_import() {
        let import = format!("./{}", boot_config::FILE_BOOT_PATH);

        assert!(!configuration_nix(&params(), false).contains(&import));

        let rendered = configuration_nix(&params(), true);
        let position = rendered.find(&import).expect("import missing");
        let closing = rendered.find("];").expect("imports list unterminated");
        assert!(position < closing, "outside the imports list:\n{rendered}");
    }

    /// Same rule as `hardware.nix`: a generated file that is imported must be
    /// sealed, or the configuration is editable outside a transaction.
    #[test]
    fn the_boot_entries_file_is_sealed() {
        assert!(BASE_FILES.contains(&boot_config::FILE_BOOT_PATH));
    }

    /// Secure Boot is the second reason `boot.nix` exists. It must write into
    /// the same file the entries do, or the configuration would need a second
    /// import, and that file must be sealed like every other imported one.
    #[test]
    fn the_secure_boot_option_shares_the_sealed_boot_file() {
        assert_eq!(
            secure_boot::FILE_SECURE_BOOT_PATH,
            boot_config::FILE_BOOT_PATH
        );
        assert!(BASE_FILES.contains(&secure_boot::FILE_SECURE_BOOT_PATH));
    }

    #[test]
    fn a_package_list_adds_its_import() {
        let mut p = params();
        p.packages = vec!["firefox".to_string()];
        let rendered = configuration_nix(&p, false);
        assert!(rendered.contains("./package.nix"));
        assert!(!rendered.contains("./module.nix"));
    }

    #[test]
    fn a_module_list_adds_its_import() {
        let mut p = params();
        p.modules = vec!["services.flatpak".to_string()];
        let rendered = configuration_nix(&p, false);
        assert!(rendered.contains("./module.nix"));
        assert!(!rendered.contains("./package.nix"));
    }

    #[test]
    fn optional_imports_land_inside_the_imports_list() {
        let mut p = params();
        p.packages = vec!["firefox".to_string()];
        p.modules = vec!["services.flatpak".to_string()];
        let rendered = configuration_nix(&p, false);

        let close = rendered.find("];").expect("the imports list must close");
        assert!(rendered.find("./package.nix").unwrap() < close);
        assert!(rendered.find("./module.nix").unwrap() < close);
        assert!(rendered.contains("networking.hostName = \"modulixos\";"));
    }
}
