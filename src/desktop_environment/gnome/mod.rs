//! GNOME's side of the desktop switch: the dconf database plus the GTK
//! `settings.ini` files.

use super::DesktopEnvironment;
use crate::{config_store::ConfigStore, core::user::User, mx};
use std::fmt;

pub mod dconf;
pub mod gtk;

/// GNOME as a [`DesktopEnvironment`]. Stateless marker: every operation takes
/// the user it applies to.
pub struct Gnome;

impl DesktopEnvironment for Gnome {
    /// Builds the marker; no I/O.
    fn new() -> Self {
        Gnome
    }

    /// Copies the user's dconf dump and GTK settings into their config store.
    ///
    /// # Parameters
    /// * `user` - whose settings are read.
    ///
    /// # Post-conditions
    /// The user's home is left untouched, GNOME included.
    ///
    /// # Errors
    /// Any error from opening the store, running `dconf dump` or reading the
    /// GTK files.
    fn save(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::save(user, &store)?;
        gtk::save(user, &store)?;
        Ok(())
    }

    /// Saves both settings sets, then wipes them from the user's home.
    ///
    /// # Parameters
    /// * `user` - whose settings are parked.
    ///
    /// # Post-conditions
    /// The dconf database is reset and the GTK files are deleted, so GNOME
    /// starts on its defaults until [`Gnome::load`] puts them back.
    fn save_and_clean(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::save(user, &store)?;
        gtk::save(user, &store)?;
        dconf::reset(user)?;
        gtk::clean(user)?;
        Ok(())
    }

    /// Wipes GNOME's settings from the user's home without saving them first.
    ///
    /// # Parameters
    /// * `user` - whose settings are wiped.
    ///
    /// # Post-conditions
    /// Anything not previously saved is lost.
    fn clean(&self, user: &User) -> mx::Result<()> {
        dconf::reset(user)?;
        gtk::clean(user)?;
        Ok(())
    }

    /// Restores into the user's home the dconf dump and GTK files a previous
    /// save parked.
    ///
    /// # Parameters
    /// * `user` - whose settings are restored.
    ///
    /// # Post-conditions
    /// A store holding nothing for GNOME leaves the home as it is. The restored
    /// dconf keys are merged into the current database rather than replacing it.
    fn load(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::load(user, &store)?;
        gtk::load(user, &store)?;
        Ok(())
    }
}

impl fmt::Display for Gnome {
    /// Writes the environment's key.
    ///
    /// # Returns
    /// `"gnome"`, the name [`super::make_desktop_environment`] accepts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gnome")
    }
}
