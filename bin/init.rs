//! `mx-init` — installer entry point.
//!
//! Parses command-line arguments and calls [`modulix_core_utils::init::init`] to write
//! the initial configuration.

use std::process::ExitCode;
use std::env;

use modulix_core_utils::init::{InitParams, init};

fn print_usage(program: &str) {
    eprintln!(
        "Usage: {} [OPTIONS]",
        program
    );
    eprintln!();
    eprintln!(
        "Options:
    --root <PATH>\t	Root directory for the system (default: /mnt)
    --hostname <NAME>\tHostname (default: modulix)
    --username <NAME>\tUsername (default: user)
    --fullname <NAME>\tFull name (default: same as username if empty)
    --desktop <DESKTOP>\tDesktop environment: gnome, plasma, or lxqt (default: gnome)
    --locale <LOCALE>\tLocale (default: en_US.UTF-8)
    --timezone <TIMEZONE>\tTimezone (default: UTC)
    --kb-layout <LAYOUT>\tKeyboard layout (default: us)
    --kb-variant <VARIANT>\tKeyboard variant (default: empty)
    --console-keymap <KEYMAP>\tConsole keymap (default: us)"
    );
}

fn parse_args() -> InitParams {
    let args: Vec<String> = env::args().collect();
    let mut root = String::new();
    let mut hostname = String::new();
    let mut username = String::new();
    let mut full_name = String::new();
    let mut desktop = String::new();
    let mut locale = String::new();
    let mut timezone = String::new();
    let mut kb_layout = String::new();
    let mut kb_variant = String::new();
    let mut console_keymap = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i < args.len() {
                    root = args[i].clone();
                }
            }
            "--hostname" => {
                i += 1;
                if i < args.len() {
                    hostname = args[i].clone();
                }
            }
            "--username" => {
                i += 1;
                if i < args.len() {
                    username = args[i].clone();
                }
            }
            "--fullname" => {
                i += 1;
                if i < args.len() {
                    full_name = args[i].clone();
                }
            }
            "--desktop" => {
                i += 1;
                if i < args.len() {
                    desktop = args[i].clone();
                }
            }
            "--locale" => {
                i += 1;
                if i < args.len() {
                    locale = args[i].clone();
                }
            }
            "--timezone" => {
                i += 1;
                if i < args.len() {
                    timezone = args[i].clone();
                }
            }
            "--kb-layout" => {
                i += 1;
                if i < args.len() {
                    kb_layout = args[i].clone();
                }
            }
            "--kb-variant" => {
                i += 1;
                if i < args.len() {
                    kb_variant = args[i].clone();
                }
            }
            "--console-keymap" => {
                i += 1;
                if i < args.len() {
                    console_keymap = args[i].clone();
                }
            }
            "--help" | "-h" => {
                print_usage(&args[0]);
                std::process::exit(0);
            }
            _ => {
                eprintln!("Error: Unknown option '{}'", args[i]);
                print_usage(&args[0]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // full_name is empty -> use username as full_name (same behavior as env variables)
    let full_name = if full_name.is_empty() { username.clone() } else { full_name };

    InitParams {
        root: if root.is_empty() { "/mnt".to_string() } else { root },
        hostname: if hostname.is_empty() { "modulix".to_string() } else { hostname },
        username,
        full_name,
        desktop,
        locale,
        timezone,
        kb_layout,
        kb_variant,
        console_keymap,
    }
}

fn main() -> ExitCode {
    let params = parse_args();

    match init(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mx-init: {e:?}");
            ExitCode::FAILURE
        }
    }
}
