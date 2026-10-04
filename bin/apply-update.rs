//! `mx-apply-update` — applies a staged system update, for a shutdown unit to
//! run.
//!
//! A Modulix system update is resolved and built while the machine is in use
//! (`modulix_core_utils::staging::stage_update`, driven by the daemon) and
//! deliberately not activated there: replacing the closure of a live session is
//! exactly what the staging mechanism exists to avoid. This binary is the other
//! half - it makes the pre-built system the next boot's default by calling
//! [`modulix_core_utils::staging::apply_staged`], then commits the lockfile that
//! was applied.
//!
//! # Usage
//!
//! `mx-apply-update [--cores <N>]`
//!
//! - `--cores <N>`: caps the activation's residual build to `N` CPU cores
//!   (`nixos-rebuild --cores`); omitted, the Nix default applies. A staged
//!   update is already built, so there is normally nothing left to compile.
//!
//! # Environment
//!
//! - `MX_DAEMON_CONFIG_DIR`: configuration repository to promote the candidate
//!   into, overriding the compiled-in `CONFIG_DIRECTORY`. Must be the same
//!   value the daemon staged with, which in Modulix it is - the unit and the
//!   daemon are configured together.
//! - `MX_CACHE_DIR`: where the staging area lives, same meaning as everywhere
//!   else in the crate. Must match the daemon's too, or this finds nothing
//!   staged.
//!
//! # Exit status
//!
//! `0` whether an update was applied or there was simply nothing staged - the
//! usual case, and not a failure. `1` if the activation or the promotion
//! failed; the staging area is then kept, so the next shutdown retries without
//! downloading anything again.
//!
//! # Ordering requirements
//!
//! Needs `/nix/store` and the bootloader's filesystem still mounted, so the
//! unit running it must be ordered before `umount.target` with
//! `DefaultDependencies=no`.

use modulix_core_utils::{CONFIG_DIRECTORY, staging};

/// Reads `--cores <N>` from the command line.
///
/// # Pre-conditions
/// None.
///
/// # Post-conditions
/// Unknown arguments are ignored rather than rejected: this runs during
/// shutdown, where failing on a typo would be worse than doing the default
/// thing.
///
/// # Returns
/// `Some(n)` for a well-formed `--cores <N>`, `None` when the flag is absent or
/// its value is not a number.
fn parse_cores() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--cores" {
            return args.next().and_then(|v| v.parse().ok());
        }
    }
    None
}

/// Configuration repository to operate on.
///
/// # Returns
/// `$MX_DAEMON_CONFIG_DIR` when set to a non-empty value, else
/// [`CONFIG_DIRECTORY`]. The path is used as given and is not validated here -
/// [`staging::apply_staged`] reports an unusable repository.
fn config_dir() -> String {
    match std::env::var("MX_DAEMON_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => dir,
        _ => CONFIG_DIRECTORY.to_string(),
    }
}

fn main() {
    let config_dir = config_dir();

    match staging::apply_staged(&config_dir, parse_cores()) {
        Ok(true) => println!("mx-apply-update: staged update applied, active on next boot"),
        Ok(false) => println!("mx-apply-update: nothing staged, shutting down unchanged"),
        Err(e) => {
            eprintln!("mx-apply-update: {e:?}");
            std::process::exit(1);
        }
    }
}
