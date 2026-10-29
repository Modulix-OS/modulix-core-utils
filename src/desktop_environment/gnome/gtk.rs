//! Parks and restores the GTK `settings.ini` files that carry a user's theme,
//! icon set and font choices.

use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{fs, path::Path};

/// The GTK settings files handled here, relative to the user's `.config`.
const GTK_CONFIGS: &[&str] = &["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"];

/// Copies the user's GTK settings files into their config store, under
/// `gnome/`.
///
/// # Parameters
/// * `user` - whose home the files are read from.
/// * `store` - destination store.
///
/// # Post-conditions
/// Files that do not exist are skipped, not reported. The home is left
/// untouched.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be read or written.
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

/// Writes the GTK settings files back into the user's `.config`.
///
/// # Parameters
/// * `user` - whose home the files are written to.
/// * `store` - store the files are read from.
///
/// # Post-conditions
/// Missing parent directories are created; an existing file is overwritten
/// wholesale. Files the store does not hold are skipped.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a directory or a file cannot be written.
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

/// Deletes the GTK settings files from the user's `.config`.
///
/// # Parameters
/// * `user` - whose files are deleted.
///
/// # Post-conditions
/// Files that are absent are skipped. Their content is lost unless [`save`] ran
/// first; the containing directories are left in place.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if a file cannot be removed.
pub fn clean(user: &User) -> mx::Result<()> {
    for path in GTK_CONFIGS {
        let target = Path::new(user.get_user_home()).join(".config").join(path);
        if target.exists() {
            fs::remove_file(&target).map_err(mx::ErrorKind::IOError)?;
        }
    }
    Ok(())
}
