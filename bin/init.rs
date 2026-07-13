//! `mx-init` — installer entry point.
//!
//! Reads the answers the ModulixOS installer (Calamares) collected from `MX_*`
//! environment variables and calls [`modulix_core_utils::init::init`] to write
//! the initial configuration under `<MX_ROOT>/etc/modulix-os`.

use std::env;
use std::process::ExitCode;

use modulix_core_utils::init::{InitParams, init};

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> ExitCode {
    let username = env_or("MX_USERNAME", "user");
    let full_name = {
        let fn_ = env_or("MX_FULLNAME", "");
        if fn_.is_empty() { username.clone() } else { fn_ }
    };

    let params = InitParams {
        root: env_or("MX_ROOT", "/mnt"),
        hostname: env_or("MX_HOSTNAME", "modulix"),
        username,
        full_name,
        desktop: env_or("MX_DESKTOP", "gnome"),
        locale: env_or("MX_LOCALE", "en_US.UTF-8"),
        timezone: env_or("MX_TIMEZONE", "UTC"),
        kb_layout: env_or("MX_KB_LAYOUT", "us"),
        kb_variant: env_or("MX_KB_VARIANT", ""),
        console_keymap: env_or("MX_CONSOLE_KEYMAP", "us"),
    };

    match init(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mx-init: {e:?}");
            ExitCode::FAILURE
        }
    }
}
