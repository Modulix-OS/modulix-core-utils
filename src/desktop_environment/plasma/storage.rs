//! Parks and restores the Plasma data living under the user's
//! `.local/share/plasma` (look-and-feel, desktop themes, layouts).

use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

/// Copies `.local/share/plasma` into the user's config store, under
/// `plasma/share`.
///
/// # Parameters
/// * `user` - whose share directory is read.
/// * `store` - destination store.
///
/// # Post-conditions
/// A no-op when the directory does not exist. Paths are stored relative to
/// `.local/share`, so the tree structure is preserved; the home is left
/// untouched.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be read or written. Entries the
/// walk cannot read are skipped silently.
pub fn save(user: &User, store: &ConfigStore) -> mx::Result<()> {
    let local_share = Path::new(user.get_user_home()).join(".local/share");
    let plasma_share = local_share.join("plasma");
    if !plasma_share.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&plasma_share).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&local_share).unwrap();
            store.save(
                Path::new("plasma/share").join(rel),
                fs::read(entry.path()).map_err(mx::ErrorKind::IOError)?,
            )?;
        }
    }
    Ok(())
}

/// Writes the parked share data back under the user's `.local/share`.
///
/// # Parameters
/// * `user` - whose share directory is written to.
/// * `store` - store the data is read from.
///
/// # Post-conditions
/// A no-op when the store holds nothing under `plasma/share`. Missing
/// directories are created and existing files overwritten; files the store does
/// not carry are left alone.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a directory or a file cannot be written.
pub fn load(user: &User, store: &ConfigStore) -> mx::Result<()> {
    let local_share = Path::new(user.get_user_home()).join(".local/share");
    let prefix = store.get("plasma/share")?;
    if !prefix.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&prefix).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&prefix).unwrap();
            let dest = local_share.join(rel);
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

/// Deletes every file of `.local/share/plasma`.
///
/// # Parameters
/// * `user` - whose share data is deleted.
///
/// # Post-conditions
/// A no-op when the directory does not exist. Files are removed but the
/// directories are kept, and the data is lost unless [`save`] ran first.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be removed.
pub fn clean(user: &User) -> mx::Result<()> {
    let local_share = Path::new(user.get_user_home()).join(".local/share");
    let plasma_share = local_share.join("plasma");
    if !plasma_share.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&plasma_share).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&plasma_share).unwrap();
            fs::remove_file(local_share.join(rel)).map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}

/// Moves every file of `.local/share/plasma` into another directory.
///
/// # Parameters
/// * `user` - whose share data is moved away.
/// * `dest` - destination directory, which must already exist.
///
/// # Post-conditions
/// A no-op when the source directory does not exist. The tree is flattened -
/// each file lands in `dest` under its own base name - so same-named files in
/// different subdirectories overwrite one another. Files are renamed, not
/// copied, so the move fails across filesystems.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be renamed.
pub fn move_to(user: &User, dest: &Path) -> mx::Result<()> {
    let local_share = Path::new(user.get_user_home()).join(".local/share");
    let plasma_share = local_share.join("plasma");
    if !plasma_share.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&plasma_share).into_iter().flatten() {
        if entry.file_type().is_file() {
            fs::rename(entry.path(), dest.join(entry.file_name()))
                .map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}
