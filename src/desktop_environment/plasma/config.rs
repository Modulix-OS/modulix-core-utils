//! Parks and restores the Plasma configuration files sitting directly in the
//! user's `.config`.

use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

/// Globs matching the Plasma config files in `.config`: everything KDE
/// (`kdeglobals`, `kwinrc`, …) and everything Plasma.
const PATTERNS: &[&str] = &["k*", "plasma*"];

/// Copies the user's Plasma config files into their config store, under
/// `plasma/config`.
///
/// # Parameters
/// * `user` - whose `.config` is read.
/// * `store` - destination store.
///
/// # Post-conditions
/// Only the files directly in `.config` are taken - the globs do not descend
/// into directories - and the home is left untouched. Non-Plasma files whose
/// name starts with `k` are collected too, the globs being that coarse.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be read or written.
///
/// # Panics
/// If one of the module's own glob patterns is invalid.
pub fn save(user: &User, store: &ConfigStore) -> mx::Result<()> {
    let config_dir = Path::new(user.get_user_home()).join(".config");
    for pattern in PATTERNS {
        let glob_pattern = config_dir.join(pattern).to_string_lossy().into_owned();
        for entry in glob::glob(&glob_pattern).unwrap().flatten() {
            if entry.is_file() {
                let rel = entry.strip_prefix(&config_dir).unwrap();
                store.save(
                    Path::new("plasma/config").join(rel),
                    fs::read(&entry).map_err(mx::ErrorKind::IOError)?,
                )?;
            }
        }
    }
    Ok(())
}

/// Writes the parked config files back into the user's `.config`.
///
/// # Parameters
/// * `user` - whose `.config` is written to.
/// * `store` - store the files are read from.
///
/// # Post-conditions
/// A no-op when the store holds nothing under `plasma/config`. Existing files
/// are overwritten; files the store does not carry are left alone, so this
/// merges rather than replaces.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be read or written.
pub fn load(user: &User, store: &ConfigStore) -> mx::Result<()> {
    let config_dir = Path::new(user.get_user_home()).join(".config");
    let prefix = store.get("plasma/config")?;
    if !prefix.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&prefix).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&prefix).unwrap();
            let dest = config_dir.join(rel);
            fs::create_dir_all(dest.parent().unwrap()).map_err(mx::ErrorKind::IOError)?;
            fs::write(
                &dest,
                fs::read(entry.path()).map_err(mx::ErrorKind::IOError)?,
            )
            .map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}

/// Deletes from `.config` the files listed under `.config/plasma/config`.
///
/// # Parameters
/// * `user` - whose files are deleted.
///
/// # Post-conditions
/// A no-op unless the home has a `.config/plasma/config` directory: the listing
/// is taken from there, not from the store and not from the `k*`/`plasma*`
/// globs [`save`] uses.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a listed file cannot be removed, which
/// includes the case where it does not exist in `.config`.
pub fn clean(user: &User) -> mx::Result<()> {
    let config_dir = Path::new(user.get_user_home()).join(".config");
    let prefix = config_dir.join("plasma/config");
    if !prefix.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&prefix).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&prefix).unwrap();
            let dest = config_dir.join(rel);
            fs::remove_file(&dest).map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}

/// Moves everything under `.config/plasma/config` to another directory, keeping
/// the tree structure.
///
/// # Parameters
/// * `user` - whose `.config/plasma/config` is emptied.
/// * `dest` - directory the files are moved into; missing parents are created.
///
/// # Post-conditions
/// A no-op when that directory does not exist. Files are renamed, not copied,
/// so the move fails across filesystems, and the emptied directories are left
/// behind.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a directory cannot be created or a file cannot
/// be renamed.
pub fn move_to(user: &User, dest: &Path) -> mx::Result<()> {
    let config_dir = Path::new(user.get_user_home()).join(".config");
    let prefix = config_dir.join("plasma/config");
    if !prefix.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&prefix).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&prefix).unwrap();
            let dest = dest.join(rel);
            fs::create_dir_all(dest.parent().unwrap()).map_err(mx::ErrorKind::IOError)?;
            fs::rename(entry.path(), dest).map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}
