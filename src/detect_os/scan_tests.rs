//! Tests for [`super`].
//!
//! [`super::scan_esp`] reads a mounted EFI system partition, so the seam is a
//! fixture tree rather than a mocked `lsblk` or a real mount: [`Esp`] builds
//! the `EFI/<vendor>/…` directories and the `.efi` files a real partition
//! exposes. The privileged half - `lsblk`, `mount(2)` - lives in
//! `super::super::esp` and is not exercised here; the `boot_entries` example
//! is what checks it against the live machine.
//!
//! The partition is scanned either as ours ([`scan_own`]) or as another
//! system's ([`scan_other`]), since that is what decides the whole policy.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

/// Disk the fixture partitions are pretended to live on, as
/// `super::super::disk_hint` would spell it.
const HINT: &str = "nvme0n1";

/// A fixture EFI system partition.
struct Esp {
    dir: TempDir,
}

impl Esp {
    /// Creates a partition holding an `EFI` directory and nothing else.
    fn new() -> Self {
        let esp = Self {
            dir: TempDir::new().expect("tempdir"),
        };
        fs::create_dir_all(esp.root().join("EFI")).expect("mkdir");
        esp
    }

    /// Creates a partition with no `EFI` directory at all, as a data partition
    /// mistyped as an ESP, or a freshly formatted one, presents.
    fn without_efi_directory() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    /// Creates a partition whose boot directory is spelled `efi`, as some
    /// firmware and installers write it - FAT does not preserve case.
    fn with_lowercase_efi_directory() -> Self {
        let esp = Self {
            dir: TempDir::new().expect("tempdir"),
        };
        fs::create_dir_all(esp.root().join("efi")).expect("mkdir");
        esp
    }

    /// The partition's mount point, to be passed where an ESP would be
    /// mounted.
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Creates an empty file at `relative`, creating its parent directories.
    ///
    /// # Parameters
    /// * `relative` - path inside the partition, e.g.
    ///   `"EFI/Microsoft/Boot/bootmgfw.efi"`.
    fn add_file(&self, relative: &str) -> &Self {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&path, b"").expect("write");
        self
    }

    /// Creates an empty directory at `relative`.
    fn add_dir(&self, relative: &str) -> &Self {
        fs::create_dir_all(self.root().join(relative)).expect("mkdir");
        self
    }
}

/// Scans `esp` as the partition this installation boots from.
fn scan_own(esp: &Esp) -> Vec<(String, String)> {
    scan_esp(esp.root(), true, HINT)
}

/// Scans `esp` as a partition belonging to another system.
fn scan_other(esp: &Esp) -> Vec<(String, String)> {
    scan_esp(esp.root(), false, HINT)
}

mod unit {
    use super::*;

    #[test]
    fn known_vendor_gets_its_product_name() {
        assert_eq!(display_name("Microsoft"), "Windows Boot Manager");
        assert_eq!(display_name("MICROSOFT"), "Windows Boot Manager");
        assert_eq!(display_name("pop"), "Pop!_OS");
    }

    #[test]
    fn unknown_vendor_is_named_after_itself_capitalised() {
        assert_eq!(display_name("frobnix"), "Frobnix");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn name_loses_the_characters_limine_reads_as_syntax() {
        assert_eq!(sanitise_name("Ubuntu/22.04"), "Ubuntu22.04");
        assert_eq!(sanitise_name(" Fedora\n"), "Fedora");
    }

    #[test]
    fn a_generic_label_is_qualified_with_the_disk() {
        assert_eq!(with_disk_hint("NixOS", "nvme0n1"), "NixOS (nvme0n1)");
        assert_eq!(with_disk_hint("NixOS", ""), "NixOS");
    }

    #[test]
    fn our_own_vendor_directories_are_the_generic_ones() {
        assert_eq!(generic_vendor_name("NixOS"), Some("NixOS"));
        assert_eq!(generic_vendor_name("BOOT"), Some("Other system"));
        assert_eq!(generic_vendor_name("systemd"), Some("Linux"));
        assert_eq!(generic_vendor_name("Microsoft"), None);
    }
}

mod own_esp {
    use super::*;

    #[test]
    fn no_efi_directory_yields_nothing() {
        assert!(scan_own(&Esp::without_efi_directory()).is_empty());
    }

    #[test]
    fn empty_efi_directory_yields_nothing() {
        assert!(scan_own(&Esp::new()).is_empty());
    }

    #[test]
    fn windows_boot_manager_is_found_one_level_down() {
        let esp = Esp::new();
        esp.add_file("EFI/Microsoft/Boot/bootmgfw.efi");

        assert_eq!(
            scan_own(&esp),
            vec![(
                "Windows Boot Manager".to_string(),
                "/EFI/Microsoft/Boot/bootmgfw.efi".to_string()
            )]
        );
    }

    #[test]
    fn loader_case_on_disk_is_preserved() {
        let esp = Esp::new();
        esp.add_file("EFI/Microsoft/Boot/BOOTMGFW.EFI");

        assert_eq!(
            scan_own(&esp),
            vec![(
                "Windows Boot Manager".to_string(),
                "/EFI/Microsoft/Boot/BOOTMGFW.EFI".to_string()
            )]
        );
    }

    #[test]
    fn lowercase_efi_directory_is_found_and_kept_as_spelled() {
        let esp = Esp::with_lowercase_efi_directory();
        esp.add_file("efi/Microsoft/Boot/bootmgfw.efi");

        assert_eq!(
            scan_own(&esp),
            vec![(
                "Windows Boot Manager".to_string(),
                "/efi/Microsoft/Boot/bootmgfw.efi".to_string()
            )]
        );
    }

    #[test]
    fn our_own_loaders_are_never_entries() {
        let esp = Esp::new();
        esp.add_file("EFI/limine/limine.conf")
            .add_file("EFI/BOOT/BOOTX64.EFI")
            .add_file("EFI/nixos/kernel.efi")
            .add_file("EFI/systemd/systemd-bootx64.efi")
            .add_file("EFI/Linux/modulixos-6.12.efi")
            .add_file("EFI/tools/shellx64.efi")
            .add_file("EFI/memtest86/memtest86.efi")
            .add_file("EFI/Microsoft/Boot/bootmgfw.efi");

        assert_eq!(
            scan_own(&esp),
            vec![(
                "Windows Boot Manager".to_string(),
                "/EFI/Microsoft/Boot/bootmgfw.efi".to_string()
            )]
        );
    }

    #[test]
    fn shim_wins_over_grub_to_keep_the_secure_boot_chain() {
        let esp = Esp::new();
        esp.add_file("EFI/ubuntu/grubx64.efi")
            .add_file("EFI/ubuntu/shimx64.efi")
            .add_file("EFI/ubuntu/mmx64.efi");

        assert_eq!(
            scan_own(&esp),
            vec![("Ubuntu".to_string(), "/EFI/ubuntu/shimx64.efi".to_string())]
        );
    }

    #[test]
    fn a_vendor_without_shim_falls_back_to_grub() {
        let esp = Esp::new();
        esp.add_file("EFI/gentoo/grubx64.efi");

        assert_eq!(
            scan_own(&esp),
            vec![("Gentoo".to_string(), "/EFI/gentoo/grubx64.efi".to_string())]
        );
    }

    #[test]
    fn an_unknown_loader_name_is_still_an_entry() {
        let esp = Esp::new();
        esp.add_file("EFI/frobnix/frobload.efi");

        assert_eq!(
            scan_own(&esp),
            vec![(
                "Frobnix".to_string(),
                "/EFI/frobnix/frobload.efi".to_string()
            )]
        );
    }

    #[test]
    fn a_vendor_directory_without_any_efi_is_skipped() {
        let esp = Esp::new();
        esp.add_dir("EFI/fedora").add_file("EFI/fedora/BOOTX64.CSV");

        assert!(scan_own(&esp).is_empty());
    }

    #[test]
    fn several_systems_come_back_sorted_by_label() {
        let esp = Esp::new();
        esp.add_file("EFI/Microsoft/Boot/bootmgfw.efi")
            .add_file("EFI/fedora/shimx64.efi")
            .add_file("EFI/debian/shimx64.efi");

        assert_eq!(
            scan_own(&esp),
            vec![
                ("Debian".to_string(), "/EFI/debian/shimx64.efi".to_string()),
                ("Fedora".to_string(), "/EFI/fedora/shimx64.efi".to_string()),
                (
                    "Windows Boot Manager".to_string(),
                    "/EFI/Microsoft/Boot/bootmgfw.efi".to_string()
                ),
            ]
        );
    }
}

mod other_esp {
    use super::*;

    /// The case this whole split exists for: a system booted by systemd-boot
    /// from unified kernel images has nothing under `EFI/` but the directories
    /// that would be ours on our own partition, and used to come back empty.
    #[test]
    fn a_systemd_boot_installation_yields_its_own_boot_loader() {
        let esp = Esp::new();
        esp.add_file("EFI/systemd/systemd-bootx64.efi")
            .add_file("EFI/BOOT/BOOTX64.EFI")
            .add_file("EFI/Linux/linux-6.12.efi");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "Linux (nvme0n1)".to_string(),
                "/EFI/BOOT/BOOTX64.EFI".to_string()
            )]
        );
    }

    /// The same tree on our own partition is ours, and Limine already lists
    /// what it boots.
    #[test]
    fn the_same_tree_on_our_own_partition_yields_nothing() {
        let esp = Esp::new();
        esp.add_file("EFI/systemd/systemd-bootx64.efi")
            .add_file("EFI/BOOT/BOOTX64.EFI")
            .add_file("EFI/Linux/linux-6.12.efi");

        assert!(scan_own(&esp).is_empty());
    }

    #[test]
    fn another_nixos_is_named_after_its_vendor_directory() {
        let esp = Esp::new();
        esp.add_file("EFI/nixos/kernel.efi")
            .add_file("EFI/BOOT/BOOTX64.EFI");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "NixOS (nvme0n1)".to_string(),
                "/EFI/BOOT/BOOTX64.EFI".to_string()
            )]
        );
    }

    #[test]
    fn a_windows_only_partition_falls_back_to_its_vendor_directory() {
        let esp = Esp::new();
        esp.add_file("EFI/Microsoft/Boot/bootmgfw.efi");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "Windows Boot Manager".to_string(),
                "/EFI/Microsoft/Boot/bootmgfw.efi".to_string()
            )]
        );
    }

    #[test]
    fn an_unknown_vendor_is_named_after_itself() {
        let esp = Esp::new();
        esp.add_file("EFI/frobnix/frobload.efi");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "Frobnix".to_string(),
                "/EFI/frobnix/frobload.efi".to_string()
            )]
        );
    }

    /// A partition shared by several systems still yields one entry, pointing
    /// at the loader the firmware itself would run: which of them installed
    /// `EFI/BOOT/BOOTX64.EFI` cannot be told from the tree, so the label says
    /// neither.
    #[test]
    fn a_shared_partition_yields_one_entry_on_its_default_loader() {
        let esp = Esp::new();
        esp.add_file("EFI/Microsoft/Boot/bootmgfw.efi")
            .add_file("EFI/fedora/shimx64.efi")
            .add_file("EFI/BOOT/BOOTX64.EFI");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "Other system (nvme0n1)".to_string(),
                "/EFI/BOOT/BOOTX64.EFI".to_string()
            )]
        );
    }

    #[test]
    fn firmware_utilities_are_not_a_system() {
        let esp = Esp::new();
        esp.add_file("EFI/tools/shellx64.efi")
            .add_file("EFI/memtest86/memtest86.efi");

        assert!(scan_other(&esp).is_empty());
    }

    #[test]
    fn without_a_disk_to_name_the_generic_label_stands_alone() {
        let esp = Esp::new();
        esp.add_file("EFI/BOOT/BOOTX64.EFI");

        assert_eq!(
            scan_esp(esp.root(), false, ""),
            vec![(
                "Other system".to_string(),
                "/EFI/BOOT/BOOTX64.EFI".to_string()
            )]
        );
    }

    #[test]
    fn a_partition_with_nothing_bootable_yields_nothing() {
        let esp = Esp::new();
        esp.add_dir("EFI/BOOT").add_file("EFI/fedora/BOOTX64.CSV");

        assert!(scan_other(&esp).is_empty());
    }
}

mod no_regression {
    use super::*;

    /// A sub-directory bearing a preferred name must not be taken for the
    /// loader itself: `EFI/<vendor>/grubx64.efi/` as a directory is nonsense a
    /// botched copy can leave behind, and chainloading it would fail at boot
    /// rather than at detection.
    #[test]
    fn a_directory_named_like_a_loader_is_not_picked() {
        let esp = Esp::new();
        esp.add_dir("EFI/ubuntu/shimx64.efi")
            .add_file("EFI/ubuntu/grubx64.efi");

        assert_eq!(
            scan_own(&esp),
            vec![("Ubuntu".to_string(), "/EFI/ubuntu/grubx64.efi".to_string())]
        );
    }

    /// Two scans of one partition must agree, including on which loader was
    /// picked: `read_dir` hands entries back in filesystem order, so the
    /// fallback path has to sort before choosing.
    #[test]
    fn the_fallback_loader_is_deterministic() {
        let esp = Esp::new();
        esp.add_file("EFI/frobnix/zzz.efi")
            .add_file("EFI/frobnix/aaa.efi")
            .add_file("EFI/frobnix/mmm.efi");

        let first = scan_own(&esp);
        assert_eq!(
            first,
            vec![("Frobnix".to_string(), "/EFI/frobnix/aaa.efi".to_string())]
        );
        assert_eq!(scan_own(&esp), first);
    }

    /// `EFI/BOOT` holds the fallback copy next to the Secure Boot shims that
    /// came with it; the entry must name the copy the firmware runs, not
    /// whichever file happens to sort first.
    #[test]
    fn the_fallback_directory_is_read_by_name_not_by_sort_order() {
        let esp = Esp::new();
        esp.add_file("EFI/BOOT/BOOTX64.EFI")
            .add_file("EFI/BOOT/fbx64.efi")
            .add_file("EFI/BOOT/mmx64.efi");

        assert_eq!(
            scan_other(&esp),
            vec![(
                "Other system (nvme0n1)".to_string(),
                "/EFI/BOOT/BOOTX64.EFI".to_string()
            )]
        );
    }
}
