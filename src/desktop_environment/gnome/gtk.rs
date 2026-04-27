use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

const GTK_CONFIGS: &[&str] = &["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"];

pub fn save(user: &User, store: &ConfigStore) -> mx::Result<()> {
    for path in GTK_CONFIGS {
        let src = Path::new(user.get_user_home()).join(".config").join(path);
        if src.exists() {
            store.save(
                format!("gnome/{path}"),
                fs::read(&src).map_err(mx::ErrorKind::IOError)?,
            )?;
        }
    }
    Ok(())
}

pub fn load(user: &User, store: &ConfigStore) -> mx::Result<()> {
    for path in GTK_CONFIGS {
        if store.exists(format!("gnome/{}", path)) {
            let dest = Path::new(user.get_user_home()).join(".config").join(path);
            fs::create_dir_all(dest.parent().unwrap()).map_err(mx::ErrorKind::IOError)?;
            fs::write(&dest, store.load(format!("gnome/{}", path))?)
                .map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}

pub fn clean(user: &User) -> mx::Result<()> {
    for path in GTK_CONFIGS {
        let target = Path::new(user.get_user_home()).join(".config").join(path);
        if target.exists() {
            fs::remove_file(&target).map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}

pub fn move_to(user: &User, dest: &Path) -> mx::Result<()> {
    let config_dir = Path::new(user.get_user_home()).join(".config");
    let prefix = config_dir.join("gnome");
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
