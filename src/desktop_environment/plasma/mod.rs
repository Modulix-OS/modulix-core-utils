use super::DesktopEnvironment;
use crate::{config_store::ConfigStore, core::user::User, mx};
use std::fmt;

pub mod config;
pub mod storage;

pub struct Plasma;

impl DesktopEnvironment for Plasma {
    fn new() -> Self {
        Plasma
    }

    fn save(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::save(user, &store)?;
        storage::save(user, &store)?;
        Ok(())
    }

    fn load(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::load(user, &store)?;
        storage::load(user, &store)?;
        Ok(())
    }

    fn clean(&self, user: &User) -> mx::Result<()> {
        config::clean(user)?;
        storage::clean(user)?;
        Ok(())
    }

    fn save_and_clean(&self, user: &User) -> mx::Result<()> {
        let store = ConfigStore::new(user.get_user_home())?;
        config::move_to(user, &store.get_path())?;
        storage::move_to(user, &store.get_path())?;
        Ok(())
    }
}

impl fmt::Display for Plasma {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plasma")
    }
}
