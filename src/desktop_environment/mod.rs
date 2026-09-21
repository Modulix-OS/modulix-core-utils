//! Switching a user from one desktop environment to another while keeping each
//! one's settings.
//!
//! Each supported environment implements `DesktopEnvironment`, which knows
//! where its settings live, how to copy them into the user's
//! `crate::config_store` and how to put them back. Switching saves the
//! outgoing environment's settings, wipes them from the user's home, then
//! restores the incoming one's.

use std::fmt;

use crate::{core::user::User, mx};

mod gnome;
mod plasma;

/// One supported desktop environment, seen as a set of per-user settings that
/// can be parked and restored.
///
/// `Display` yields the environment's key (`gnome`, `plasma`), the same string
/// [`make_desktop_environment`] takes.
trait DesktopEnvironment: fmt::Display {
    /// Builds the handle. Stateless: the environment is identified by its type,
    /// and every operation takes the user it applies to.
    fn new() -> Self
    where
        Self: Sized;

    /// Copies the user's settings for this environment into their config store.
    ///
    /// # Parameters
    /// * `user` - whose home the settings are read from.
    ///
    /// # Post-conditions
    /// The home is left untouched; settings that are absent are skipped rather
    /// than reported.
    fn save(&self, user: &User) -> mx::Result<()>;

    /// Removes this environment's settings from the user's home.
    ///
    /// # Parameters
    /// * `user` - whose settings are wiped.
    ///
    /// # Post-conditions
    /// Unsaved settings are lost: call [`DesktopEnvironment::save`] first, or
    /// use [`DesktopEnvironment::save_and_clean`].
    fn clean(&self, user: &User) -> mx::Result<()>;

    /// Saves the settings, then removes them from the user's home.
    ///
    /// # Parameters
    /// * `user` - whose settings are parked.
    ///
    /// # Post-conditions
    /// What [`DesktopEnvironment::load`] needs is in the config store, and the
    /// home no longer carries this environment's settings.
    fn save_and_clean(&self, user: &User) -> mx::Result<()>;

    /// Restores into the user's home the settings a previous save parked.
    ///
    /// # Parameters
    /// * `user` - whose settings are restored.
    ///
    /// # Post-conditions
    /// A store holding nothing for this environment leaves the home as it is -
    /// a first switch is not an error, the environment just starts on its
    /// defaults.
    fn load(&self, user: &User) -> mx::Result<()>;
}

/// Builds the handle for a desktop environment named by its key.
///
/// # Parameters
/// * `name` - environment key, `"gnome"` or `"plasma"`.
///
/// # Returns
/// The boxed implementation.
///
/// # Errors
/// [`mx::ErrorKind::InvalidArgument`] for any other name.
fn make_desktop_environment(name: &str) -> mx::Result<Box<dyn DesktopEnvironment>> {
    match name {
        "gnome" => Ok(Box::new(gnome::Gnome::new())),
        "plasma" => Ok(Box::new(plasma::Plasma::new())),
        _ => Err(
            mx::ErrorKind::InvalidArgument("Invalid desktop environment name".to_string()).into(),
        ),
    }
}

/// Switches a user from one desktop environment to another, carrying each one's
/// settings along.
///
/// # Parameters
/// * `current` - key of the environment being left, whose settings are parked.
/// * `new` - key of the environment being entered, whose settings are restored.
/// * `user` - the user this applies to; only their own files are touched.
///
/// # Pre-conditions
/// Meant to run while the user is not in a session of either environment, since
/// a running session rewrites its settings on exit. Reading and writing another
/// user's home requires privileges.
///
/// # Post-conditions
/// `current`'s settings are in the config store and gone from the home;
/// `new`'s are back in place, or absent if it was never saved. A failure
/// midway can leave the home with neither environment's settings, the parked
/// copy of `current` being the recovery path.
///
/// # Errors
/// [`mx::ErrorKind::InvalidArgument`] for an unknown key, plus any I/O error
/// from moving the settings.
pub fn switch_desktop_environment(current: &str, new: &str, user: &User) -> mx::Result<()> {
    let current = make_desktop_environment(current)?;
    let new = make_desktop_environment(new)?;
    current.save_and_clean(user)?;
    new.load(user)?;
    Ok(())
}
