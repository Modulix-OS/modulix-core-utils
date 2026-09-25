//! Bootstraps the NixOS configuration repository for a brand-new Modulix
//! system: `init_repo` is used by the installer to seed a minimal repo
//! ahead of hardware detection, and `init` is the installer's actual
//! engine, writing every base file (flake, configuration, hardware,
//! fstab, locale, user) in a single transaction and sealing them
//! immutable.
//!
//! Everything here is local-only: no network fetch happens (in particular
//! `crate::REMOTE_CONFIG_URL` is declared in `lib.rs` but never used by
//! this module — `flake.nix`/`configuration.nix` are rendered from the
//! `FLAKE_FILE`/`CONFIG_FILE` templates and `nixos-generate-config`,
//! not downloaded). [`init`] lists [`crate::CACHE_DIRECTORY_NAME`] in the
//! repo's `.git/info/exclude` so the generated-index cache is never
//! stashed or committed. Whether a call targets the live system (`/`) or
//! an installation root (e.g. `/mnt`) is entirely up to the `root_path`/
//! `params.root` argument passed in; this module does not choose it.
//! Root privileges are required in release builds only where the target
//! paths (`/etc/modulix-os/` via [`crate::CONFIG_DIRECTORY`]) are
//! root-owned and where `nixos-install`/`nixos-generate-config` need them;
//! in debug builds every rebuild is forced to `build-vm`
//! (`BuildCommand::as_str`), so a debug run never touches the host system.

use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::LOCK_SKIP_REBUILD_FILE;
use crate::core::transaction::transaction::TransactionPermission;
use crate::core::transaction::transaction::{BuildCommand, UpdateInput};
use crate::error::io_message;
use crate::{CONFIG_DIRECTORY, filesystem, hardware_config, locale, mx, user};
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
/// Unlike [`init`], this does not create the `.cache` directory, does not
/// hold the skip-rebuild lock, and does not call `seal_base_files`
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
/// * `config_dir` - overrides where the config repo is written (see
///   `resolve_config_path`).
/// * `debug` - debug/test mode: seeds the repo with `nixos-rebuild build-vm`
///   instead of `nixos-install`/`switch`, and skips the skip-rebuild lock so
///   the build actually runs.
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
    /// Overrides where the config repo is written (see `resolve_config_path`).
    pub config_dir: Option<String>,
    /// Debug/test mode: seeds the repo with `nixos-rebuild build-vm` instead of
    /// `nixos-install`/`switch`, and skips the skip-rebuild lock so the build
    /// actually runs.
    pub debug: bool,
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
/// creates `locale.nix` and `users.nix`, so listing them is exact.
///
/// # Parameters
/// * `p` - init parameters; `hostname` and `desktop` are interpolated into
///   the template.
///
/// # Returns
/// The full `configuration.nix` source, importing
/// `hardware-configuration.nix`, `fstab.nix`, `locale.nix` and `users.nix`,
/// and setting `networking.hostName` and `mx.desktop`.
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
///
/// # Parameters
/// * `repo_path` - root of the config git repo; the cache directory
///   ([`crate::CACHE_DIRECTORY_NAME`]) and `.git/info/exclude` are created
///   under it.
///
/// # Pre-conditions
/// `repo_path` is an existing, initialized git repository.
///
/// # Post-conditions
/// `repo_path/CACHE_DIRECTORY_NAME` exists. `repo_path/.git/info/exclude`
/// exists and contains `CACHE_DIRECTORY_NAME/` and `result`, one per line,
/// overwriting any previous content of that file.
///
/// # Returns
/// `Ok(())` once both the cache directory and the exclude file are written.
///
/// # Errors
/// * `mx::ErrorKind::IOError` - creating either directory, or writing the
///   exclude file, failed.
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

/// Takes an exclusive lock on `LOCK_SKIP_REBUILD_FILE`, telling every commit
/// made while the returned handle is alive to skip its `nixos-rebuild` (see
/// `Transaction::commit_impl`'s build-serialization step).
///
/// Used by [`init`] outside of debug/test mode: the real installer
/// (Calamares / modulixos-installer) holds this lock for the whole install,
/// so the seed transaction here only writes files and commits — no rebuild
/// runs until the installer releases the lock and drives its own rebuild.
///
/// # Pre-conditions
/// No other process holds the lock (a concurrent [`init`]/install run, or a
/// test relying on the same sentinel file).
///
/// # Post-conditions
/// `LOCK_SKIP_REBUILD_FILE` exists and is exclusively locked by the returned
/// `File` handle; the lock is released when that handle is dropped.
///
/// # Returns
/// The open, locked `File` handle. Callers must keep it alive for as long as
/// commits should skip their rebuild.
///
/// # Errors
/// * `mx::ErrorKind::IOError` - `LOCK_SKIP_REBUILD_FILE` could not be
///   created/opened.
/// * `mx::ErrorKind::FailToLock` - the lock is already held elsewhere.
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

/// The installer's engine: creates the NixOS config repo for a brand-new
/// Modulix system from scratch and seeds it with everything a first boot
/// needs (flake, base configuration, detected hardware, filesystem table,
/// locale/keyboard, and one user).
///
/// Resolves the repo path with `config_path` and rejects it with
/// `validate_config_path` before anything is deleted. Unlike
/// [`init_repo`], any pre-existing directory at that path — git repo or
/// not — is always wiped with `remove_dir_recursive` and replaced by a
/// fresh `git init` (branch `main`); there is no re-adoption path. Then, in
/// order:
/// 1. `init_cache_dir` creates `.cache` and excludes it (and `result`) via
///    `.git/info/exclude`.
/// 2. `fstab.nix` is rendered from `filesystem::fstab_module`
///    and `configuration.nix` from `configuration_nix`.
/// 3. Outside debug mode, `hold_skip_rebuild_lock` is taken and held for
///    the rest of the call (assigned to `_skip_rebuild`, dropped — and thus
///    released — when `init` returns), so the transaction below only writes
///    and commits; the real installer (Calamares / modulixos-installer)
///    keeps its own lock held and drives the actual rebuild itself. In debug
///    mode no lock is taken and the build genuinely runs, as `BuildVm` (see
///    next point).
/// 4. The build command passed to the `Transaction` is `BuildCommand::Boot`
///    in release and `BuildCommand::BuildVm` in debug (which, per
///    `BuildCommand::as_str`, resolves to `build-vm` either way in debug
///    builds — so a debug run never touches the host system regardless of
///    which variant is named here).
/// 5. A single `Transaction` (`TransactionPermission::Writtable`) is opened
///    over `flake.nix`, `hardware-configuration.nix`, `fstab.nix`,
///    `locale::LOCALE_FILE_PATH` and `user::USER_FILE_PATH`
///    (`configuration.nix` is auto-added by `Transaction::begin`); within it,
///    `flake.nix`/`configuration.nix`/`fstab.nix` get their rendered content,
///    the hardware file is filled by
///    `hardware_config::write_hardware_config_no_transaction`, the locale
///    file by `locale::set_locale_no_transaction` then
///    `locale::set_keyboard_no_transaction`, and the user file by
///    `user::add_no_transaction` (`params.username`, empty initial password,
///    `params.full_name`, `DEFAULT_SHELL`, groups `["wheel",
///    "networkmanager"]`, `is_normal_user = true`). Locale and keyboard are
///    folded into this same transaction, rather than a separate one, so the
///    seed repo carries them from the first commit instead of needing a
///    second transaction and rebuild; the X11 keyboard layout is set here —
///    not in `configuration.nix` — so it is written whatever `params.desktop`
///    is, and stays editable afterwards through `locale::set_keyboard`. The
///    user file is filled for the same reason: one seed transaction, not one
///    per file. Any failure at this stage calls `tx.rollback()` and returns
///    the original error.
/// 6. `tx.commit(UpdateInput::UpdateAll)` commits and refreshes `flake.lock`.
/// 7. `seal_base_files` re-applies the immutable flag to every file in
///    `BASE_FILES` that exists.
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
/// read `params.root`. No other process holds the skip-rebuild lock
/// (relevant outside debug mode).
///
/// # Post-conditions
/// On success: the resolved config directory is a freshly initialized git
/// repo on branch `main`, containing `flake.nix`, `configuration.nix`,
/// `hardware-configuration.nix`, `fstab.nix`, the locale and user files, and
/// `flake.lock`, all committed as `"modulix init"`; every file in
/// `BASE_FILES` present on disk is immutable; `.cache` exists and is
/// listed in `.git/info/exclude` together with `result`. Outside debug mode,
/// no `nixos-rebuild`/`nixos-install` has actually run (the skip-rebuild
/// lock suppresses it) — the caller is expected to drive that separately. In
/// debug mode, `nixos-rebuild build-vm` has run as part of the commit.
///
/// On failure: if the failure happens before `tx.begin()` succeeds, the
/// directory may already have been wiped and recreated (and, if reached,
/// `.cache`/the exclude file already written) with no commit made. If the
/// failure happens after `begin()` while filling file contents, the
/// transaction is explicitly rolled back (`tx.rollback()`) before the error
/// is returned. If `tx.commit` itself fails, `Transaction::commit_impl`
/// rolls back internally. In every rollback case the held skip-rebuild lock
/// (if any) is still released when `_skip_rebuild` drops at function return.
/// `seal_base_files` is only reached after a successful commit.
///
/// # Returns
/// `Ok(())` once the repo has been created, seeded, committed and sealed.
///
/// # Errors
/// * `mx::ErrorKind::InvalidFile` - `validate_config_path` rejected the
///   resolved path, or a later path-to-`str` conversion failed.
/// * `mx::ErrorKind::IOError` - directory removal/creation, or any file I/O
///   in `init_cache_dir`/`seal_base_files`, failed.
/// * `mx::ErrorKind::GitError` - `git2` failed to init the repo.
/// * `mx::ErrorKind::FailToLock` - `hold_skip_rebuild_lock` could not
///   acquire its lock (non-debug builds only).
/// * Any error from `filesystem::fstab_module`,
///   `hardware_config::write_hardware_config_no_transaction`,
///   `locale::set_locale_no_transaction`,
///   `locale::set_keyboard_no_transaction`, `user::add_no_transaction`,
///   `Transaction::new`, `Transaction::add_file`, `Transaction::begin`,
///   `Transaction::get_file_mut`, `NixFile::get_mut_file_content`, or
///   `Transaction::commit`.
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

    let fstab = filesystem::fstab_module(&params.root)?;
    let config = configuration_nix(params);

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

    /// Name of the hardware config file registered in the transaction below,
    /// factored out so [`Transaction::add_file`] and
    /// [`Transaction::get_file_mut`] stay in sync.
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
