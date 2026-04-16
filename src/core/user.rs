use std::{io::Stdout, process};

use serde::Deserialize;

use crate::mx;

#[derive(Debug, Clone)]
pub struct User {
    uid: u32,
    unix_username: String,
    home_path: String,
}

#[derive(Deserialize)]
struct UserRecord {
    #[serde(rename = "userName")]
    user_name: String,
    #[serde(rename = "homeDirectory")]
    home_directory: Option<String>,
    uid: Option<u32>,
}

impl User {
    pub fn new(uid: u32, unix_username: &str, home_path: &str) -> Self {
        User {
            uid,
            unix_username: unix_username.to_string(),
            home_path: home_path.to_string(),
        }
    }

    pub fn list_all_real_user() -> mx::Result<Vec<User>> {
        let output = process::Command::new("userdbctl")
            .args([
                "user",
                "--no-pager",
                "--no-legend",
                "-RB",
                "--output=json",
                "--json=short",
            ])
            .output()
            .map_err(mx::ErrorKind::IOError)?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut users = Vec::new();
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let record: UserRecord =
                serde_json::from_str(line).map_err(|e| mx::ErrorKind::IOError(e.into()))?;

            if record.home_directory.as_deref() == Some("/var/empty") {
                continue;
            }
            let uid = record.uid.unwrap_or(0);
            let home = record.home_directory.unwrap_or_default();
            users.push(User::new(uid, &record.user_name, &home));
        }

        Ok(users)
    }

    pub fn get_user_id(&self) -> u32 {
        self.uid
    }

    pub fn get_user_name(&self) -> &str {
        &self.unix_username
    }

    pub fn get_user_home(&self) -> &str {
        &self.home_path
    }
}

pub fn for_all_users(f: impl Fn(&User) -> mx::Result<()>) -> mx::Result<()> {
    let users = User::list_all_real_user()?;
    for user in users {
        f(&user)?;
    }
    Ok(())
}
