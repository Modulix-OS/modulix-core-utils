use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

const PATTERNS: &[&str] = &["k*", "plasma*"];

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
