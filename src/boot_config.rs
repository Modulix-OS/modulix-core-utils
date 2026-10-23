//! `boot.nix`, the configuration file carrying the boot menu entries of the
//! other operating systems installed on the machine.
//!
//! The only option written here is `mx.bootloader.extraEntries`, which
//! mxpkgs' `modulixos/boot.nix` renders into Limine's `extraEntries` as one
//! `protocol: efi_chainload` block per entry - and which, when it is not
//! empty, also raises the boot menu timeout from 1 to 5 seconds.
//!
//! Writing is a **replacement**, never a merge: the list is whatever the last
//! detection found. A caller that wants to add or drop one entry by hand edits
//! `boot.nix`, and the next redetection overwrites it.

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    detect_os::{self, OtherOs},
    mx,
};

/// Configuration file, relative to the config directory, holding the boot menu
/// entries of the other operating systems.
pub const FILE_BOOT_PATH: &str = "boot.nix";

/// Option mxpkgs reads the extra boot entries from.
const OPTION_EXTRA_ENTRIES: &str = "mx.bootloader.extraEntries";

/// Where a running Modulix system mounts its own EFI system partition.
const OWN_ESP_MOUNT: &str = "/boot";

/// Renders a value as a Nix string literal.
///
/// # Parameters
/// * `value` - the raw text to quote.
///
/// # Returns
/// `value` wrapped in double quotes, with `\`, `"` and `$` escaped - `$` so a
/// path that happens to contain `${` cannot turn into an interpolation.
fn nix_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for c in value.chars() {
        if matches!(c, '\\' | '"' | '$') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('"');
    escaped
}

/// Renders one boot entry as a single-line Nix attribute set.
///
/// Single-line because [`mxList::add`] puts each element of a list on its own
/// line and indents it; a multi-line value would be spliced in as is and come
/// out misaligned.
///
/// # Parameters
/// * `entry` - the entry to render.
///
/// # Returns
/// `{ name = "…"; efiPath = "…"; esp = "…"; }`, with `esp` left out entirely
/// when the entry lives on this installation's own ESP - the option defaults
/// to `null` there, which mxpkgs renders as Limine's `boot()`.
fn render_entry(entry: &OtherOs) -> String {
    let mut rendered = format!(
        "{{ name = {}; efiPath = {};",
        nix_string(&entry.name),
        nix_string(&entry.efi_path)
    );
    if let Some(esp) = &entry.esp {
        rendered.push_str(&format!(" esp = {};", nix_string(esp)));
    }
    rendered.push_str(" }");
    rendered
}

/// Collapses every run of whitespace into a single space.
///
/// # Parameters
/// * `text` - the Nix source fragment to normalise.
///
/// # Returns
/// `text` with its whitespace runs reduced to one space and its ends trimmed,
/// so two renderings of the same list that differ only in layout compare
/// equal.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Whether the file already declares exactly these entries.
///
/// Asked before writing, because clearing the option and splicing the list
/// back in does not reproduce the previous layout byte for byte: a redetection
/// that found nothing new would still dirty the file, and a dirtied file is a
/// commit and a `nixos-rebuild` for nothing.
///
/// # Parameters
/// * `file` - the open configuration file to read.
/// * `rendered` - the entries as [`render_entry`] spells them.
///
/// # Returns
/// `true` when the option is declared and holds those elements, in that order,
/// up to whitespace. `false` when it is absent, holds something else, or
/// cannot be read.
fn already_declares(file: &NixFile, rendered: &[String]) -> bool {
    let Ok(current) = mxOption::new(OPTION_EXTRA_ENTRIES).get(file) else {
        return false;
    };

    collapse_whitespace(current) == format!("[ {} ]", rendered.join(" "))
}

/// Replaces the extra boot entries in an already-open `boot.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `entries` - the entries to declare; whatever the file declared before is
///   dropped first.
///
/// # Post-conditions
/// `mx.bootloader.extraEntries` holds exactly `entries`, in the given order.
/// An empty `entries` leaves the option **absent** rather than `[ ]`: mxpkgs
/// defaults it to the empty list anyway, and an absent option keeps the file
/// free of a declaration that says nothing.
///
/// A file that already declares exactly `entries` is left untouched, byte for
/// byte - clearing and re-splicing the list does not reproduce the previous
/// layout, and a file dirtied for nothing is a commit and a rebuild for
/// nothing.
///
/// # Errors
/// Any error from [`mxOption::set_option_all_instance_to_default`] or
/// [`mxList::add`],
/// notably `mx::ErrorKind::PermissionDenied` on a read-only transaction and
/// `mx::ErrorKind::TransactionNotBegin` on a file that is not open.
pub fn write_boot_entries_no_transaction(
    file: &mut NixFile,
    entries: &[OtherOs],
) -> mx::Result<()> {
    let rendered: Vec<String> = entries.iter().map(render_entry).collect();
    if !rendered.is_empty() && already_declares(file, &rendered) {
        return Ok(());
    }

    mxOption::new(OPTION_EXTRA_ENTRIES).set_option_all_instance_to_default(file)?;

    let list = mxList::new(OPTION_EXTRA_ENTRIES, false);
    for value in &rendered {
        list.add(file, value)?;
    }

    Ok(())
}

/// Writes the extra boot entries and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `entries` - the entries to declare. Taken as a parameter rather than
///   probed here so a caller that has already probed - [`crate::init`] does -
///   does not probe twice, and so a caller can supply a list of its own.
///
/// # Post-conditions
/// On success the entries are part of the active generation; on error the
/// configuration is rolled back. Blocks for the whole `nixos-rebuild switch`.
/// A list identical to what the file already declares produces no commit and
/// therefore no rebuild.
///
/// # Errors
/// Any error from the transaction or from
/// [`write_boot_entries_no_transaction`].
pub fn write_boot_entries(config_dir: &str, entries: &[OtherOs]) -> mx::Result<()> {
    transaction::make_transaction(
        "Update the boot entries of the other operating systems",
        config_dir,
        FILE_BOOT_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| write_boot_entries_no_transaction(file, entries),
    )
}

/// Re-scans the EFI system partitions and applies what they hold.
///
/// The entry point for a caller that reacts to another operating system being
/// installed or removed next to Modulix.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
///
/// # Pre-conditions
/// Scans the **live** machine, whose own ESP is expected at `/boot`; an
/// installation targeting another root goes through
/// [`detect_os::detect`] + [`write_boot_entries`] instead, as
/// [`crate::init`] does. Needs root and `lsblk` on `PATH`.
///
/// # Post-conditions
/// As in [`write_boot_entries`]. A scan that finds nothing removes the option,
/// which drops the menu timeout back to 1 second.
///
/// # Errors
/// Any error from [`detect_os::detect`] or [`write_boot_entries`].
pub fn redetect_boot_entries(config_dir: &str) -> mx::Result<()> {
    let entries = detect_os::detect(OWN_ESP_MOUNT)?;
    write_boot_entries(config_dir, &entries)
}

#[cfg(test)]
#[path = "boot_config_tests.rs"]
mod tests;
