//! `mx-init` — installer entry point for Modulix OS.
//!
//! Bootstraps a brand-new NixOS configuration repository (`flake.nix`, `configuration.nix`,
//! `hardware-configuration.nix`, `fstab.nix`, plus locale and user files) by parsing CLI flags
//! into a `modulix_core_utils::init::InitParams` and calling
//! [`modulix_core_utils::init::init`], which writes every file in a single git-committed,
//! immutable-sealed transaction.
//!
//! # Usage
//!
//! `mx-init [OPTIONS]`. Every option is optional; each falls back to the default listed below
//! when omitted:
//!
//! - `--root <PATH>`: installation root the generated config targets (default `/mnt`).
//! - `--hostname <NAME>`: `networking.hostName` (default `modulix`).
//! - `--username <NAME>`: account created in the generated user file (default `user`).
//! - `--fullname <NAME>`: account's full name (default: same as the resolved username).
//! - `--desktop <DESKTOP>`: `gnome`, `plasma`, `lxqt`, or `cli` (default `gnome`); an
//!   unrecognized value is rejected (process exits with status `1`) rather than silently
//!   producing a headless system.
//! - `--locale <LOCALE>`: system locale (default `en_US.UTF-8`).
//! - `--timezone <TIMEZONE>`: `time.timeZone` (default `UTC`).
//! - `--kb-layout <LAYOUT>`: keyboard layout (default `us`).
//! - `--kb-variant <VARIANT>`: keyboard variant (default: empty string).
//! - `--console-keymap <KEYMAP>`: console keymap (default `us`).
//! - `--config-dir <PATH>`: writes the config repo here instead of the default location
//!   (`<root>/etc/modulix-os/` in release builds); unlike every other flag, omitting its value
//!   is a hard error (prints usage, exits with status `1`) rather than falling back to a
//!   default, since `init` deletes whatever directory this resolves to.
//! - `--debug`: seeds the repo with `nixos-rebuild build-vm` instead of installing/switching
//!   the host.
//! - `--help` / `-h`: prints usage to stderr and exits with status `0`.
//!
//! Any other argument is rejected with an "Unknown option" error, the usage text, and exit
//! status `1`.
//!
//! # Requirements
//!
//! No file is downloaded by this tool itself. In release builds it needs root privileges,
//! since the target paths under the resolved config directory are root-owned and
//! `nixos-install`/`nixos-rebuild` (invoked while committing the transaction) need them; that
//! commit may also run `nix flake update`, which needs network access when `flake.lock` is not
//! already present. Debug builds never touch the host system: every rebuild is forced to
//! `nixos-rebuild build-vm` regardless of `--debug`.
//!
//! # On failure
//!
//! If the resolved config directory already existed, it is deleted and recreated as an empty
//! git repository before any file content is written, so prior content there is gone even if
//! the run subsequently fails. Files the transaction had begun creating are then rolled back
//! (deleted) rather than left partially written; see [`main`].

use std::env;
use std::process::ExitCode;

use modulix_core_utils::init::{Desktop, InitParams, init};

/// Prints the `mx-init` usage banner (recognized flags, their argument placeholders, and
/// defaults) to stderr.
///
/// # Parameters
/// - `program`: program name shown in the `Usage: <program> [OPTIONS]` line; callers pass
///   `args[0]` (the invoked binary path).
///
/// # Post-conditions
/// Writes the usage text to stderr. Does not exit the process; callers that need to terminate
/// after printing do so themselves.
fn print_usage(program: &str) {
    eprintln!("Usage: {} [OPTIONS]", program);
    eprintln!();
    eprintln!(
        "Options:
    --root <PATH>\t	Root directory for the system (default: /mnt)
    --hostname <NAME>\tHostname (default: modulix)
    --username <NAME>\tUsername (default: user)
    --fullname <NAME>\tFull name (default: same as username if empty)
    --desktop <DESKTOP>\tDesktop environment: gnome, plasma, lxqt, or cli (default: gnome)
    --locale <LOCALE>\tLocale (default: en_US.UTF-8)
    --timezone <TIMEZONE>\tTimezone (default: UTC)
    --kb-layout <LAYOUT>\tKeyboard layout (default: us)
    --kb-variant <VARIANT>\tKeyboard variant (default: empty)
    --console-keymap <KEYMAP>\tConsole keymap (default: us)
    --config-dir <PATH>\tWrite the config repo here instead of the default location
    --debug\t\tSeed with `nixos-rebuild build-vm` instead of installing/switching"
    );
}

/// Consumes the value following a flag at `args[*i]`.
///
/// # Parameters
/// * `args` - the full argument list.
/// * `i` - index of the flag itself; advanced to the value's index on success.
/// * `flag` - the flag's spelling, used only to name it in the error message.
///
/// # Returns
/// The value, cloned out of `args`.
///
/// # Errors
/// An error naming `flag` when it is the last argument, i.e. no value follows it.
fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    if *i >= args.len() {
        Err(format!("{flag} requires a value"))
    } else {
        Ok(args[*i].clone())
    }
}

/// Parses `std::env::args()` into an `InitParams`, applying each flag's documented default
/// (see the crate-level docs) to any value left unset.
///
/// # Pre-conditions
/// None beyond a valid process argument list; reads the live `env::args()` on every call, so
/// repeated calls can observe different results only if the process arguments themselves
/// change (they do not, in practice, within one run).
///
/// # Recognized flags
/// `--root`, `--hostname`, `--username`, `--fullname`, `--desktop`, `--locale`, `--timezone`,
/// `--kb-layout`, `--kb-variant`, `--console-keymap`, `--config-dir`, `--debug`, `--help`/`-h`
/// (see the crate-level docs for each flag's effect and default). `--desktop` is additionally
/// validated via `Desktop::parse`.
///
/// # Returns
/// The populated `InitParams` to pass to `modulix_core_utils::init::init`.
///
/// # Panics
/// Never panics. Instead, `parse_args` terminates the process directly (via
/// `std::process::exit`) in four cases: a value-taking flag given without a following value
/// (after printing usage, status `1`), an unrecognized `--desktop` value (status `1`), an unknown
/// option (after printing usage, status `1`), or `--help`/`-h` (after printing usage, status
/// `0`).
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
    let mut config_dir: Option<String> = None;
    let mut debug = false;

    let mut i = 1;
    while i < args.len() {
        let flag = args[i].as_str();
        macro_rules! value {
            () => {
                take_value(&args, &mut i, flag).unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    print_usage(&args[0]);
                    std::process::exit(1);
                })
            };
        }
        match flag {
            "--root" => root = value!(),
            "--hostname" => hostname = value!(),
            "--username" => username = value!(),
            "--fullname" => full_name = value!(),
            "--desktop" => desktop = value!(),
            "--locale" => locale = value!(),
            "--timezone" => timezone = value!(),
            "--kb-layout" => kb_layout = value!(),
            "--kb-variant" => kb_variant = value!(),
            "--console-keymap" => console_keymap = value!(),
            "--config-dir" => config_dir = Some(value!()),
            "--debug" => {
                debug = true;
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

    let username = if username.is_empty() {
        "user".to_string()
    } else {
        username
    };

    let full_name = if full_name.is_empty() {
        username.clone()
    } else {
        full_name
    };

    let desktop = if desktop.is_empty() {
        Desktop::Gnome
    } else {
        match Desktop::parse(&desktop) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    };

    InitParams {
        root: if root.is_empty() {
            "/mnt".to_string()
        } else {
            root
        },
        hostname: if hostname.is_empty() {
            "modulix".to_string()
        } else {
            hostname
        },
        username,
        full_name,
        desktop,
        locale: if locale.is_empty() {
            "en_US.UTF-8".to_string()
        } else {
            locale
        },
        timezone: if timezone.is_empty() {
            "UTC".to_string()
        } else {
            timezone
        },
        kb_layout: if kb_layout.is_empty() {
            "us".to_string()
        } else {
            kb_layout
        },
        kb_variant,
        console_keymap: if console_keymap.is_empty() {
            "us".to_string()
        } else {
            console_keymap
        },
        config_dir,
        debug,
    }
}

/// Entry point for the `mx-init` binary.
///
/// Parses CLI arguments via [`parse_args`] and calls `modulix_core_utils::init::init` to write
/// the initial NixOS configuration repository as a single, immutable, git-committed
/// transaction (see the crate-level docs for what is written, what is required, and what is
/// left behind on failure).
///
/// # Pre-conditions
/// Same as the crate as a whole: in release builds, root privileges and (when `flake.lock` is
/// absent) network access for `nix flake update`; none of that is required in debug builds,
/// where every rebuild is forced to `nixos-rebuild build-vm`.
///
/// # Returns
/// `ExitCode::SUCCESS` on success. On failure, prints `mx-init: <error message>` to stderr and
/// returns `ExitCode::FAILURE`.
fn main() -> ExitCode {
    let params = parse_args();

    match init(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mx-init: {e}");
            ExitCode::FAILURE
        }
    }
}
