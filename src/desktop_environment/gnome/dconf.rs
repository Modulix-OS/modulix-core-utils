use crate::{config_store::ConfigStore, core::user::User, mx};
use std::{
    io::Write,
    process::{self, Stdio},
};

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
