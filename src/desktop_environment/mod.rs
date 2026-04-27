use std::fmt;

use crate::{core::user::User, mx};

mod gnome;
mod plasma;

trait DesktopEnvironment: fmt::Display {
    fn new() -> Self
    where
        Self: Sized;
    fn save(&self, user: &User) -> mx::Result<()>;
    fn clean(&self, user: &User) -> mx::Result<()>;
    fn save_and_clean(&self, user: &User) -> mx::Result<()>;
    fn load(&self, user: &User) -> mx::Result<()>;
}

fn make_desktop_environment(name: &str) -> mx::Result<Box<dyn DesktopEnvironment>> {
    match name {
        "gnome" => Ok(Box::new(gnome::Gnome::new())),
        "plasma" => Ok(Box::new(plasma::Plasma::new())),
        _ => Err(
            mx::ErrorKind::InvalidArgument("Invalid desktop environment name".to_string()).into(),
        ),
    }
}

pub fn switch_desktop_environment(current: &str, new: &str, user: &User) -> mx::Result<()> {
    let current = make_desktop_environment(current)?;
    let new = make_desktop_environment(new)?;
    current.save_and_clean(user)?;
    new.load(user)?;
    Ok(())
}
