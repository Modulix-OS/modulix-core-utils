use super::DesktopEnvironment;
use crate::{config_store::ConfigStore, core::user::User, mx};
use std::fmt;

pub mod dconf;
pub mod gtk;

pub struct Gnome;

impl DesktopEnvironment for Gnome {
    fn new() -> Self {
        Gnome
    }

    fn save(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::save(user, &store)?;
        gtk::save(user, &store)?;
        Ok(())
    }

    fn save_and_clean(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::save(user, &store)?;
        gtk::save(user, &store)?;
        dconf::reset(user)?;
        gtk::clean(user)?;
        Ok(())
    }

    fn clean(&self, user: &User) -> mx::Result<()> {
        dconf::reset(user)?;
        gtk::clean(user)?;
        Ok(())
    }

    fn load(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        dconf::load(user, &store)?;
        gtk::load(user, &store)?;
        Ok(())
    }
}

impl fmt::Display for Gnome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gnome")
    }
}
