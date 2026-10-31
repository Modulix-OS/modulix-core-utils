//! Tests for [`super`]'s two halves.
//!
//! The state half is pure byte handling over an `efivarfs` payload, so it is
//! tested on the bytes directly rather than on the live firmware - a test
//! suite cannot pick which mode the machine running it booted in.
//!
//! The writer half runs against a real file inside a real writable
//! transaction over a real git repository, which is then rolled back and the
//! buffer inspected - the same harness shape [`crate::boot_config`]'s tests
//! use, since the option is spliced into Nix source and only the rendered
//! text proves it was done right.

use std::fs;

use tempfile::TempDir;

use super::{
    EFIVARFS_ATTRIBUTES_SIZE, FILE_SECURE_BOOT_PATH, FirmwareState, OPTION_SECURE_BOOT,
    parse_setup_mode, set_secure_boot_no_transaction,
};
use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::{BuildCommand, TransactionPermission};

/// The stub `NixFile::create_file` writes, i.e. what the writer starts from on
/// a fresh configuration.
const EMPTY: &str = "{config, lib, pkgs, ...}:\n{\n}\n";

/// An `efivarfs` payload: the four-byte attribute word, then `data`.
fn efivar(data: &[u8]) -> Vec<u8> {
    let mut raw = vec![6, 0, 0, 0];
    assert_eq!(raw.len(), EFIVARFS_ATTRIBUTES_SIZE);
    raw.extend_from_slice(data);
    raw
}

/// Runs `edit` against a real `boot.nix` seeded with `seed`, inside a writable
/// transaction that is rolled back, and returns the resulting buffer.
fn with_boot(seed: &str, edit: impl FnOnce(&mut NixFile)) -> String {
    let dir = TempDir::new().expect("failed to create temporary directory");
    git2::Repository::init(dir.path()).expect("git init failed");
    fs::write(dir.path().join(FILE_SECURE_BOOT_PATH), seed).expect("failed to write boot.nix");
    fs::write(
        dir.path().join("configuration.nix"),
        "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
    )
    .expect("failed to write configuration.nix");

    let repo_path = format!("{}/", dir.path().to_str().unwrap());
    let mut tx = Transaction::new(
        &repo_path,
        "test",
        BuildCommand::Boot,
        TransactionPermission::Writtable,
    )
    .expect("Transaction::new failed");
    tx.add_file(FILE_SECURE_BOOT_PATH).expect("add_file failed");
    tx.begin().expect("begin failed");

    let file = tx
        .get_file_mut(FILE_SECURE_BOOT_PATH)
        .expect("get_file_mut failed");
    edit(file);
    let content = file
        .get_file_content()
        .expect("get_file_content failed")
        .clone();
    tx.rollback().expect("rollback failed");
    content
}

/// Applies the writer to a file seeded with `seed`.
fn render(seed: &str, enable: bool) -> String {
    with_boot(seed, |file| {
        set_secure_boot_no_transaction(file, enable).expect("write failed");
    })
}

mod unit {
    use super::*;

    #[test]
    fn a_non_zero_data_byte_is_setup_mode() {
        assert_eq!(parse_setup_mode(&efivar(&[1])), FirmwareState::SetupMode);
    }

    #[test]
    fn a_zero_data_byte_is_user_mode() {
        assert_eq!(parse_setup_mode(&efivar(&[0])), FirmwareState::UserMode);
    }

    #[test]
    fn a_payload_with_no_data_byte_is_user_mode() {
        assert_eq!(parse_setup_mode(&efivar(&[])), FirmwareState::UserMode);
        assert_eq!(parse_setup_mode(&[]), FirmwareState::UserMode);
    }

    #[test]
    fn only_setup_mode_allows_enrollment() {
        assert!(FirmwareState::SetupMode.allows_enrollment());
        assert!(!FirmwareState::UserMode.allows_enrollment());
        assert!(!FirmwareState::NotUefi.allows_enrollment());
    }

    #[test]
    fn enabling_declares_the_option() {
        let content = render(EMPTY, true);

        assert!(content.contains("secureBoot"));
        assert!(content.contains("true"));
    }

    #[test]
    fn disabling_a_fresh_file_declares_nothing() {
        let content = render(EMPTY, false);

        assert!(!content.contains("secureBoot"));
    }

    #[test]
    fn the_rendered_file_still_parses_as_nix() {
        let content = render(EMPTY, true);

        assert!(
            rnix::Root::parse(&content).errors().is_empty(),
            "rendered file does not parse: {content}"
        );
    }
}

mod no_regression {
    use super::*;

    /// The option path is what `mxpkgs/modulixos/boot.nix` declares; a
    /// mismatch writes an option nothing reads, and NixOS only reports it as
    /// an unknown-option evaluation error at rebuild time.
    #[test]
    fn the_option_path_is_the_one_mxpkgs_reads() {
        assert_eq!(OPTION_SECURE_BOOT, "mx.bootloader.secureBoot.enable");
    }

    /// The whole `mx.bootloader.*` namespace lives in one file; splitting it
    /// would leave `crate::boot_config` and this module writing two files the
    /// configuration imports separately.
    #[cfg(feature = "boot-config")]
    #[test]
    fn the_option_shares_the_bootloader_file() {
        assert_eq!(FILE_SECURE_BOOT_PATH, crate::boot_config::FILE_BOOT_PATH);
    }

    /// Disabling must *remove* the assignment rather than write `false`, since
    /// mxpkgs already defaults the option to `false`. The empty attribute
    /// sets the removal leaves behind are a no-op in Nix and are what
    /// `mxOption::set_option_to_default` does everywhere in the crate.
    #[test]
    fn disabling_removes_a_previous_declaration() {
        let enabled = render(EMPTY, true);
        assert!(enabled.contains("secureBoot"));

        let disabled = render(&enabled, false);
        assert!(
            !disabled.contains("enable"),
            "the declaration survived: {disabled}"
        );
    }

    /// Writing twice must not stack two declarations of the same option: two
    /// definitions of one `types.bool` make the NixOS module system fail even
    /// at equal values.
    #[test]
    fn enabling_twice_declares_the_option_once() {
        let once = render(EMPTY, true);
        let twice = render(&once, true);

        assert_eq!(twice.matches("secureBoot").count(), 1, "{twice}");
    }

    /// `crate::boot_config` resets only its own option, so the two writers
    /// must be able to share the file without either dropping the other's
    /// work.
    #[test]
    fn the_extra_entries_option_survives() {
        let seed = concat!(
            "{config, lib, pkgs, ...}:\n",
            "{\n",
            "  mx.bootloader.extraEntries = [\n",
            "    { name = \"Windows Boot Manager\";\n",
            "      efiPath = \"/EFI/Microsoft/Boot/bootmgfw.efi\"; }\n",
            "  ];\n",
            "}\n",
        );
        let content = render(seed, true);

        assert!(content.contains("extraEntries"));
        assert!(content.contains("Windows Boot Manager"));
        assert!(content.contains("secureBoot"));
    }
}
