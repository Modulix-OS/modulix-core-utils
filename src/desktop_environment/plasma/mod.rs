//! Plasma's side of the desktop switch: the `k*`/`plasma*` config files plus
//! the `.local/share/plasma` data.

use super::DesktopEnvironment;
use crate::{config_store::ConfigStore, core::user::User, mx};
use std::fmt;

pub mod config;
pub mod storage;

/// Plasma as a [`DesktopEnvironment`]. Stateless marker: every operation takes
/// the user it applies to.
pub struct Plasma;

impl DesktopEnvironment for Plasma {
    /// Builds the marker; no I/O.
    fn new() -> Self {
        Plasma
    }

    /// Copies the user's Plasma config files and share data into their config
    /// store.
    ///
    /// # Parameters
    /// * `user` - whose settings are read.
    ///
    /// # Post-conditions
    /// The user's home is left untouched.
    fn save(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::save(user, &store)?;
        storage::save(user, &store)?;
        Ok(())
    }

    /// Restores into the user's home the config files and share data a previous
    /// save parked.
    ///
    /// # Parameters
    /// * `user` - whose settings are restored.
    ///
    /// # Post-conditions
    /// A store holding nothing for Plasma leaves the home as it is. Restored
    /// files overwrite their counterparts; files the store does not carry are
    /// left alone.
    fn load(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::load(user, &store)?;
        storage::load(user, &store)?;
        Ok(())
    }

    /// Wipes Plasma's settings from the user's home without saving them first.
    ///
    /// # Parameters
    /// * `user` - whose settings are wiped.
    ///
    /// # Post-conditions
    /// Anything not previously saved is lost.
    fn clean(&self, user: &User) -> mx::Result<()> {
        config::clean(user)?;
        storage::clean(user)?;
        Ok(())
    }

    /// Parks Plasma's settings by moving them into the config store, rather
    /// than copying and then deleting them.
    ///
    /// # Parameters
    /// * `user` - whose settings are parked.
    ///
    /// # Post-conditions
    /// The files land in the store's root, not under the `plasma/config` and
    /// `plasma/share` prefixes [`Plasma::load`] reads from, so what this parks
    /// is not what a later load restores. The config half only moves
    /// `.config/plasma/config`, not the `k*`/`plasma*` files [`Plasma::save`]
    /// collects, so it is a no-op on a home that has no such directory.
    fn save_and_clean(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::move_to(user, &store.get_path())?;
        storage::move_to(user, &store.get_path())?;
        Ok(())
    }
}

impl fmt::Display for Plasma {
    /// Writes the environment's key.
    ///
    /// # Returns
    /// `"plasma"`, the name [`super::make_desktop_environment`] accepts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plasma")
    }
}
