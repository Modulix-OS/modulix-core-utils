//! Enumerates the user accounts the running system actually has, as opposed to
//! the ones declared in the configuration ([`crate::user`]).

use std::process;

use serde::Deserialize;

use crate::mx;

/// One account of the running system.
///
/// # Fields
/// * `uid` - numeric user id.
/// * `unix_username` - login name.
/// * `home_path` - home directory; may be empty when the account declares
///   none.
#[derive(Debug, Clone)]
pub struct User {
    uid: u32,
    unix_username: String,
    home_path: String,
}

/// One `userdbctl --output=json` record, as far as this module reads it.
///
/// # Fields
/// * `user_name` - login name, always present.
/// * `home_directory` - home directory, absent for accounts that declare none.
/// * `uid` - numeric id, absent in records that omit it.
#[derive(Deserialize)]
struct UserRecord {
    #[serde(rename = "userName")]
    user_name: String,
    #[serde(rename = "homeDirectory")]
    home_directory: Option<String>,
    uid: Option<u32>,
}

impl User {
    /// Builds a user description from its parts.
    ///
    /// # Parameters
    /// * `uid` - numeric user id.
    /// * `unix_username` - login name, copied into the struct.
    /// * `home_path` - home directory, copied into the struct.
    ///
    /// # Returns
    /// An owned description; nothing is checked against the system, so the
    /// account need not exist.
    pub fn new(uid: u32, unix_username: &str, home_path: &str) -> Self {
        User {
            uid,
            unix_username: unix_username.to_string(),
            home_path: home_path.to_string(),
        }
    }

    /// Lists the machine's human accounts by querying `userdbctl`.
    ///
    /// # Returns
    /// One entry per account, in `userdbctl` order, minus the accounts whose
    /// home is `/var/empty` - the convention marking a service account. A
    /// record without a uid is reported as uid 0, and one without a home as an
    /// empty path.
    ///
    /// # Pre-conditions
    /// `userdbctl` (systemd) must be on `PATH`.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the command cannot be spawned, or if one
    /// of its JSON lines does not parse. A non-zero exit status is not reported
    /// on its own: it surfaces as an empty list.
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

    /// Numeric id of the account.
    ///
    /// # Returns
    /// The uid as reported by `userdbctl`, or 0 when the record omitted it.
    pub fn get_user_id(&self) -> u32 {
        self.uid
    }

    /// Login name of the account.
    ///
    /// # Returns
    /// The name, borrowed from `self`.
    pub fn get_user_name(&self) -> &str {
        &self.unix_username
    }

    /// Home directory of the account.
    ///
    /// # Returns
    /// The path, borrowed from `self`; empty when the account declares none.
    pub fn get_user_home(&self) -> &str {
        &self.home_path
    }
}

/// Runs `f` once per account of the machine.
///
/// # Parameters
/// * `f` - action applied to each user, in the order
///   [`User::list_all_real_user`] returns them.
///
/// # Post-conditions
/// Stops at the first error and propagates it, so `f` may already have run -
/// and had its effect - on the earlier users.
///
/// # Errors
/// Any error from [`User::list_all_real_user`] or from `f`.
pub fn for_all_users(f: impl Fn(&User) -> mx::Result<()>) -> mx::Result<()> {
    let users = User::list_all_real_user()?;
    for user in users {
        f(&user)?;
    }
    Ok(())
}
