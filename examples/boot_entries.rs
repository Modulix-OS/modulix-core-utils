//! Detects the operating systems installed next to Modulix and, with
//! `--write`, declares them in the configuration.
//!
//! Run with `--features boot-config`. Needs root: the EFI system partitions
//! are root-owned, and mounting the ones that are not mounted is privileged.
//! Without root the scan simply finds nothing.
//!
//! ```sh
//! cargo build --example boot_entries --features boot-config
//! sudo ./target/debug/examples/boot_entries
//! sudo ./target/debug/examples/boot_entries --write
//! ```
//!
//! In a debug build `CONFIG_DIRECTORY` is the crate's `test/` fixture and
//! every rebuild is forced to `build-vm`, so `--write` never touches the host.

use modulix_core_utils::{CONFIG_DIRECTORY, boot_config, detect_os};

/// Where a running system mounts its own EFI system partition.
const OWN_ESP_MOUNT: &str = "/boot";

fn main() {
    let write = std::env::args().any(|a| a == "--write");

    let entries = match detect_os::detect(OWN_ESP_MOUNT) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("detection failed: {e}");
            std::process::exit(1);
        }
    };

    if entries.is_empty() {
        println!("no other operating system found");
    }
    for entry in &entries {
        let esp = match &entry.esp {
            Some(partuuid) => partuuid.as_str(),
            None => "(our own ESP)",
        };
        println!("{} -> {} on {}", entry.name, entry.efi_path, esp);
    }

    if !write {
        println!("\nre-run with --write to declare them in {CONFIG_DIRECTORY}");
        return;
    }

    match boot_config::write_boot_entries(CONFIG_DIRECTORY, &entries) {
        Ok(()) => println!(
            "\nwritten to {CONFIG_DIRECTORY}{}",
            boot_config::FILE_BOOT_PATH
        ),
        Err(e) => {
            eprintln!("\nwrite failed: {e}");
            std::process::exit(1);
        }
    }
}
