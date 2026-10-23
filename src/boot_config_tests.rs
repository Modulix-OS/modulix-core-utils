//! Tests for the `boot.nix` writer in [`super`].
//!
//! Each case runs the writer against a real file inside a real writable
//! transaction over a real git repository, then rolls the transaction back and
//! inspects the buffer - the same harness shape `crate::hardware_config`'s
//! tests use, since the entries are spliced into Nix source and only the
//! rendered text proves it was done right.

use std::fs;

use tempfile::TempDir;

use super::{FILE_BOOT_PATH, OPTION_EXTRA_ENTRIES, write_boot_entries_no_transaction};
use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::{BuildCommand, TransactionPermission};
use crate::detect_os::OtherOs;

/// The stub `NixFile::create_file` writes, i.e. what the writer starts from on
/// a fresh configuration.
const EMPTY: &str = "{config, lib, pkgs, ...}:\n{\n}\n";

/// Runs `edit` against a real `boot.nix` seeded with `seed`, inside a writable
/// transaction that is rolled back, and returns the resulting buffer.
fn with_boot(seed: &str, edit: impl FnOnce(&mut NixFile)) -> String {
    let dir = TempDir::new().expect("failed to create temporary directory");
    git2::Repository::init(dir.path()).expect("git init failed");
    fs::write(dir.path().join(FILE_BOOT_PATH), seed).expect("failed to write boot.nix");
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
    tx.add_file(FILE_BOOT_PATH).expect("add_file failed");
    tx.begin().expect("begin failed");

    let file = tx.get_file_mut(FILE_BOOT_PATH).expect("get_file_mut");
    edit(file);
    let content = file
        .get_file_content()
        .expect("get_file_content failed")
        .clone();
    tx.rollback().expect("rollback failed");
    content
}

/// Renders `entries` into a file seeded with `seed`.
fn render(seed: &str, entries: &[OtherOs]) -> String {
    with_boot(seed, |file| {
        write_boot_entries_no_transaction(file, entries).expect("write failed");
    })
}

/// An entry on another EFI system partition.
fn on_other_esp(name: &str, efi_path: &str, esp: &str) -> OtherOs {
    OtherOs {
        name: name.to_string(),
        efi_path: efi_path.to_string(),
        esp: Some(esp.to_string()),
    }
}

/// An entry on this installation's own EFI system partition.
fn on_own_esp(name: &str, efi_path: &str) -> OtherOs {
    OtherOs {
        name: name.to_string(),
        efi_path: efi_path.to_string(),
        esp: None,
    }
}

mod unit {
    use super::*;

    #[test]
    fn an_entry_on_another_partition_names_it() {
        let content = render(
            EMPTY,
            &[on_other_esp(
                "Windows Boot Manager",
                "/EFI/Microsoft/Boot/bootmgfw.efi",
                "49eaf743-20fe-44fa-b9fe-b0944153ac72",
            )],
        );

        assert!(content.contains(r#"name = "Windows Boot Manager";"#));
        assert!(content.contains(r#"efiPath = "/EFI/Microsoft/Boot/bootmgfw.efi";"#));
        assert!(content.contains(r#"esp = "49eaf743-20fe-44fa-b9fe-b0944153ac72";"#));
    }

    #[test]
    fn an_entry_on_our_own_partition_omits_the_partition() {
        let content = render(EMPTY, &[on_own_esp("Fedora", "/EFI/fedora/shimx64.efi")]);

        assert!(content.contains(r#"name = "Fedora";"#));
        assert!(!content.contains("esp ="));
    }

    #[test]
    fn every_entry_is_written() {
        let content = render(
            EMPTY,
            &[
                on_own_esp("Fedora", "/EFI/fedora/shimx64.efi"),
                on_other_esp(
                    "Windows Boot Manager",
                    "/EFI/Microsoft/Boot/bootmgfw.efi",
                    "49eaf743-20fe-44fa-b9fe-b0944153ac72",
                ),
            ],
        );

        assert!(content.contains(r#"name = "Fedora";"#));
        assert!(content.contains(r#"name = "Windows Boot Manager";"#));
    }

    #[test]
    fn the_rendered_file_still_parses_as_nix() {
        let content = render(
            EMPTY,
            &[on_other_esp(
                "Windows Boot Manager",
                "/EFI/Microsoft/Boot/bootmgfw.efi",
                "49eaf743-20fe-44fa-b9fe-b0944153ac72",
            )],
        );

        assert!(rnix::Root::parse(&content).errors().is_empty(), "{content}");
    }

    #[test]
    fn no_entry_declares_no_option() {
        let content = render(EMPTY, &[]);

        assert!(!content.contains(OPTION_EXTRA_ENTRIES));
        assert!(!content.contains("extraEntries"));
    }
}

mod no_regression {
    use super::*;

    /// Writing is a replacement, not a merge: the detection is the only source
    /// of truth, so an entry that no longer exists must disappear instead of
    /// lingering as a menu item that fails to boot.
    #[test]
    fn a_second_write_replaces_the_first() {
        let first = render(
            EMPTY,
            &[on_other_esp(
                "Windows Boot Manager",
                "/EFI/Microsoft/Boot/bootmgfw.efi",
                "49eaf743-20fe-44fa-b9fe-b0944153ac72",
            )],
        );
        let second = render(&first, &[on_own_esp("Fedora", "/EFI/fedora/shimx64.efi")]);

        assert!(second.contains(r#"name = "Fedora";"#));
        assert!(!second.contains("Windows Boot Manager"));
        assert_eq!(second.matches("extraEntries").count(), 1);
    }

    /// Re-running the detection with the same result must not accumulate: the
    /// option is cleared before the list is rebuilt.
    #[test]
    fn rewriting_the_same_entries_is_idempotent() {
        let entries = [on_other_esp(
            "Windows Boot Manager",
            "/EFI/Microsoft/Boot/bootmgfw.efi",
            "49eaf743-20fe-44fa-b9fe-b0944153ac72",
        )];

        let once = render(EMPTY, &entries);
        let twice = render(&once, &entries);

        assert_eq!(once, twice);
        assert_eq!(twice.matches("bootmgfw.efi").count(), 1);
    }

    /// An empty detection must take the option away, which is what drops the
    /// menu timeout back to 1 second on a machine whose other system was
    /// removed.
    #[test]
    fn an_empty_detection_removes_a_previous_list() {
        let seeded = render(EMPTY, &[on_own_esp("Fedora", "/EFI/fedora/shimx64.efi")]);
        let cleared = render(&seeded, &[]);

        assert!(!cleared.contains("extraEntries"));
        assert!(rnix::Root::parse(&cleared).errors().is_empty(), "{cleared}");
    }

    /// A label or a path carrying a quote, a backslash or a Nix interpolation
    /// must not be able to break out of its string literal.
    #[test]
    fn a_hostile_label_stays_inside_its_string() {
        let content = render(
            EMPTY,
            &[on_own_esp(
                r#"Win"dows ${evil} \ back"#,
                "/EFI/frob/load.efi",
            )],
        );

        assert!(rnix::Root::parse(&content).errors().is_empty(), "{content}");
        assert!(content.contains(r#"\"dows"#));
        assert!(content.contains(r#"\${evil}"#));
    }

    /// The option path is the one mxpkgs declares; a typo here is a silent
    /// no-op at build time, since an unknown `mx.*` option is an evaluation
    /// error only if nothing else defines it.
    #[test]
    fn the_option_path_is_the_one_mxpkgs_reads() {
        assert_eq!(OPTION_EXTRA_ENTRIES, "mx.bootloader.extraEntries");
    }

    /// The writer must not care how the list was previously laid out: a
    /// hand-written one-line declaration is replaced just as well as one it
    /// wrote itself.
    #[test]
    fn a_hand_written_list_is_replaced() {
        let seed = concat!(
            "{config, lib, pkgs, ...}:\n",
            "{\n",
            "  mx.bootloader.extraEntries = [ { name = \"Old\"; efiPath = \"/EFI/old/old.efi\"; } ];\n",
            "}\n"
        );
        let content = render(seed, &[on_own_esp("Fedora", "/EFI/fedora/shimx64.efi")]);

        assert!(!content.contains("Old"));
        assert!(content.contains(r#"name = "Fedora";"#));
        assert!(rnix::Root::parse(&content).errors().is_empty(), "{content}");
    }
}
