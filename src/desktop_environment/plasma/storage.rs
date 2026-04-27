use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

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

// Clean all share file in .local/share/plasma
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

// Move all share file in .local/share/plasma in config store
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
