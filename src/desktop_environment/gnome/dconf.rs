//! Parks and restores a user's whole dconf database, through the `dconf`
//! command run as that user.

use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{
    io::Write,
    process::{self, Stdio},
};

/// Dumps the user's dconf database into their config store.
///
/// # Parameters
/// * `user` - whose database is dumped; `dconf` runs as them through `sudo`.
/// * `store` - destination store, where the dump lands as `gnome/dconf.ini`.
///
/// # Pre-conditions
/// `sudo` and `dconf` must be on `PATH`, and the caller must be allowed to run
/// commands as `user` without a password prompt.
///
/// # Post-conditions
/// An empty dump writes nothing, so a previously saved file is kept rather than
/// overwritten with nothing. A non-empty dump replaces it wholesale.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the command cannot be spawned, or the file
/// cannot be written. A `dconf` failure is not reported: it shows up as an
/// empty dump.
pub fn save(user: &User, store: &ConfigStore) -> mx::Result<()> {
    let output = process::Command::new("sudo")
        .args(["-u", &user.get_user_name(), "--", "dconf", "dump", "/"])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.stdout.is_empty() {
        store.save("gnome/dconf.ini", &output.stdout)?;
    }

    Ok(())
}

/// Feeds a previously saved dump back into the user's dconf database.
///
/// # Parameters
/// * `user` - whose database is written; `dconf` runs as them through `sudo`.
/// * `store` - store the dump is read from.
///
/// # Post-conditions
/// A no-op when the store holds no dump. The keys are merged into the current
/// database: keys the dump does not mention keep their current value, so pair
/// this with [`reset`] for a clean restore.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the dump cannot be read, the command cannot be
/// spawned, or its stdin cannot be written. `dconf`'s own exit status is waited
/// for but not checked.
pub fn load(user: &User, store: &ConfigStore) -> mx::Result<()> {
    if !store.exists("gnome/dconf.ini") {
        return Ok(());
    }

    let text = store.load_string("gnome/dconf.ini")?;

    let mut child = process::Command::new("sudo")
        .args(["-u", &user.get_user_name(), "--", "dconf", "load", "/"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(mx::ErrorKind::IOError)?;

    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .map_err(mx::ErrorKind::IOError)?;

    child.wait().map_err(mx::ErrorKind::IOError)?;

    Ok(())
}

/// Resets the user's whole dconf database to the defaults.
///
/// # Parameters
/// * `user` - whose database is reset; `dconf` runs as them through `sudo`.
///
/// # Post-conditions
/// Every key under `/` is gone, GNOME's and any other application's alike.
/// Unsaved settings are lost.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the command cannot be spawned; a `dconf`
/// failure is not reported.
pub fn reset(user: &User) -> mx::Result<()> {
    process::Command::new("sudo")
        .args([
            "-u",
            &user.get_user_name(),
            "--",
            "dconf",
            "reset",
            "-f",
            "/",
        ])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    Ok(())
}
