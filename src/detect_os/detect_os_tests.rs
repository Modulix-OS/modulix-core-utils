//! Tests for [`super`].
//!
//! Only the parts of the module that do not need a machine: how two spellings
//! of a mount point compare, and the order [`super::detect`] hands its entries
//! back in. The partition reading is tested in `super::scan`, the privileged
//! enumeration and mounting is not tested at all - see `super::esp`.

use super::*;

mod unit {
    use super::*;

    #[test]
    fn mount_point_comparison_ignores_a_trailing_slash() {
        assert_eq!(
            normalise_mount_point("/nonexistent/boot/"),
            normalise_mount_point("/nonexistent/boot")
        );
    }
}

mod no_regression {
    use super::*;

    /// The entries are ordered, so a caller - and the `boot.nix` it writes -
    /// does not churn between two identical detections.
    #[test]
    fn entries_sort_on_label_then_path_then_partition() {
        let mut entries = [
            OtherOs {
                name: "Windows Boot Manager".to_string(),
                efi_path: "/EFI/Microsoft/Boot/bootmgfw.efi".to_string(),
                esp: Some("b".to_string()),
            },
            OtherOs {
                name: "Windows Boot Manager".to_string(),
                efi_path: "/EFI/Microsoft/Boot/bootmgfw.efi".to_string(),
                esp: None,
            },
            OtherOs {
                name: "Debian".to_string(),
                efi_path: "/EFI/debian/shimx64.efi".to_string(),
                esp: None,
            },
        ];
        entries.sort();

        assert_eq!(
            entries
                .iter()
                .map(|e| (e.name.as_str(), e.esp.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("Debian", None),
                ("Windows Boot Manager", None),
                ("Windows Boot Manager", Some("b")),
            ]
        );
    }
}

mod disk {
    use super::*;

    #[test]
    fn a_partition_is_named_after_the_disk_it_lies_on() {
        assert_eq!(disk_hint("/dev/nvme0n1p1"), "nvme0n1");
        assert_eq!(disk_hint("/dev/mmcblk0p1"), "mmcblk0");
        assert_eq!(disk_hint("/dev/sda1"), "sda");
    }

    #[test]
    fn a_device_without_a_partition_suffix_is_left_alone() {
        assert_eq!(disk_hint("/dev/sda"), "sda");
        assert_eq!(disk_hint("/dev/nvme0n1"), "nvme0n");
        assert_eq!(disk_hint(""), "");
    }
}
