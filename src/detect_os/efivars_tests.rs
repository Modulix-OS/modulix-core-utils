//! Tests for [`super`].
//!
//! The seam is the bytes, not the filesystem: [`boot_variable`] builds an
//! `EFI_LOAD_OPTION` the way a firmware stores one, so the whole parsing path -
//! description, hard drive node, file path node, boot order - is exercised
//! without `/sys/firmware/efi/efivars`, which needs root and a UEFI machine.
//! Reading that directory is the untested half, like `super::super::esp`.

use super::*;

/// PARTUUID the fixture entries point at, as `lsblk` would report it.
const PARTUUID: &str = "49eaf743-20fe-44fa-b9fe-b0944153ac72";

/// The same GUID as a hard drive node stores it: first three fields
/// little-endian, the rest big-endian.
const PARTUUID_BYTES: [u8; 16] = [
    0x43, 0xf7, 0xea, 0x49, 0xfe, 0x20, 0xfa, 0x44, 0xb9, 0xfe, 0xb0, 0x94, 0x41, 0x53, 0xac, 0x72,
];

/// Encodes `text` as a NUL-terminated UTF-16LE string.
fn utf16(text: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// Builds a hard drive device path node for [`PARTUUID_BYTES`].
///
/// # Parameters
/// * `signature_type` - the node's signature type: `2` for a GPT GUID, `1` for
///   an MBR disk signature.
fn hard_drive_node(signature_type: u8) -> Vec<u8> {
    let mut node = vec![
        DEVICE_PATH_TYPE_MEDIA,
        DEVICE_PATH_SUBTYPE_HARD_DRIVE,
        HARD_DRIVE_NODE_SIZE as u8,
        0,
    ];
    node.extend_from_slice(&1u32.to_le_bytes());
    node.extend_from_slice(&2048u64.to_le_bytes());
    node.extend_from_slice(&2_097_152u64.to_le_bytes());
    node.extend_from_slice(&PARTUUID_BYTES);
    node.push(2);
    node.push(signature_type);
    node
}

/// Builds a file path device path node holding `path`.
fn file_path_node(path: &str) -> Vec<u8> {
    let text = utf16(path);
    let length = (DEVICE_PATH_HEADER_SIZE + text.len()) as u16;

    let mut node = vec![DEVICE_PATH_TYPE_MEDIA, DEVICE_PATH_SUBTYPE_FILE_PATH];
    node.extend_from_slice(&length.to_le_bytes());
    node.extend_from_slice(&text);
    node
}

/// Builds the end-of-path node every device path list is terminated by.
fn end_node() -> Vec<u8> {
    vec![DEVICE_PATH_TYPE_END, 0xff, 4, 0]
}

/// Builds the content of a `Boot####` `efivarfs` file.
///
/// # Parameters
/// * `description` - the label the operating system registered.
/// * `device_path` - the device path list the entry points at.
fn boot_variable(description: &str, device_path: &[u8]) -> Vec<u8> {
    let mut raw = vec![0x07, 0x00, 0x00, 0x00];
    raw.extend_from_slice(&1u32.to_le_bytes());
    raw.extend_from_slice(&(device_path.len() as u16).to_le_bytes());
    raw.extend_from_slice(&utf16(description));
    raw.extend_from_slice(device_path);
    raw
}

/// Builds a whole entry pointing at `path` on [`PARTUUID`].
fn entry(description: &str, path: &str) -> BootOption {
    let mut device_path = hard_drive_node(SIGNATURE_TYPE_GUID);
    device_path.extend_from_slice(&file_path_node(path));
    device_path.extend_from_slice(&end_node());

    parse_boot_variable(&boot_variable(description, &device_path)).expect("parsed")
}

mod unit {
    use super::*;

    #[test]
    fn a_guid_is_spelled_the_way_the_partition_table_does() {
        assert_eq!(format_guid(&PARTUUID_BYTES).as_deref(), Some(PARTUUID));
        assert_eq!(format_guid(&PARTUUID_BYTES[..8]), None);
    }

    #[test]
    fn a_description_stops_at_its_terminator() {
        let mut bytes = utf16("Fedora");
        bytes.extend_from_slice(&utf16("ignored"));

        assert_eq!(utf16_until_nul(&bytes), "Fedora");
        assert_eq!(utf16_size_with_nul(&bytes), Some(14));
        assert_eq!(utf16_size_with_nul(&[0x41, 0x00]), None);
    }

    #[test]
    fn a_firmware_path_compares_like_a_scanned_one() {
        assert_eq!(
            normalise_efi_path("\\EFI\\Microsoft\\Boot\\bootmgfw.efi"),
            "/efi/microsoft/boot/bootmgfw.efi"
        );
        assert_eq!(
            normalise_efi_path("EFI/BOOT/BOOTX64.EFI"),
            "/efi/boot/bootx64.efi"
        );
    }

    #[test]
    fn a_variable_name_must_be_a_global_boot_entry() {
        let guid = EFI_GLOBAL_VARIABLE;
        assert_eq!(boot_option_number(&format!("Boot0002-{guid}")), Some(2));
        assert_eq!(boot_option_number(&format!("Boot000A-{guid}")), Some(10));
        assert_eq!(boot_option_number(&format!("BootOrder-{guid}")), None);
        assert_eq!(boot_option_number(&format!("Boot02-{guid}")), None);
        assert_eq!(
            boot_option_number("Boot0002-8be4df61-0000-0000-0000-000000000000"),
            None
        );
    }

    #[test]
    fn the_boot_order_is_a_list_of_option_numbers() {
        let raw = [0x07, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00];
        assert_eq!(parse_boot_order(&raw), vec![2, 0, 1]);
        assert_eq!(parse_boot_order(&[0x07, 0x00]), Vec::<u16>::new());
    }
}

mod load_option {
    use super::*;

    #[test]
    fn an_entry_carries_its_label_partition_and_loader() {
        assert_eq!(
            entry(
                "Windows Boot Manager",
                "\\EFI\\Microsoft\\Boot\\bootmgfw.efi"
            ),
            BootOption {
                description: "Windows Boot Manager".to_string(),
                partuuid: Some(PARTUUID.to_string()),
                efi_path: Some("/efi/microsoft/boot/bootmgfw.efi".to_string()),
            }
        );
    }

    /// An MBR disk signature is not a PARTUUID, and Limine cannot address it,
    /// so such an entry must not be matched against a partition.
    #[test]
    fn a_non_guid_signature_is_not_a_partition_identifier() {
        let mut device_path = hard_drive_node(1);
        device_path.extend_from_slice(&file_path_node("\\EFI\\BOOT\\BOOTX64.EFI"));
        device_path.extend_from_slice(&end_node());

        let option = parse_boot_variable(&boot_variable("Legacy", &device_path)).expect("parsed");
        assert_eq!(option.partuuid, None);
        assert_eq!(option.efi_path.as_deref(), Some("/efi/boot/bootx64.efi"));
    }

    /// A network entry has neither partition nor file, and the firmware ships
    /// several of them by default.
    #[test]
    fn an_entry_without_a_device_path_is_still_read() {
        let option = parse_boot_variable(&boot_variable("UEFI PXEv4", &[])).expect("parsed");
        assert_eq!(option.description, "UEFI PXEv4");
        assert_eq!(option.partuuid, None);
        assert_eq!(option.efi_path, None);
    }

    #[test]
    fn a_truncated_variable_is_refused() {
        assert_eq!(parse_boot_variable(&[0x07, 0x00, 0x00]), None);
        assert_eq!(parse_boot_variable(&[0x07, 0x00, 0x00, 0x00]), None);
        assert_eq!(
            parse_boot_variable(&[0x07, 0x00, 0x00, 0x00, 1, 0, 0, 0, 0, 0, 0x41, 0x00]),
            None
        );
    }
}

mod label {
    use super::*;

    #[test]
    fn a_loader_is_named_after_the_entry_pointing_at_it() {
        let options = [entry("Fedora", "\\EFI\\fedora\\shimx64.efi")];

        assert_eq!(
            label_for(&options, PARTUUID, "/EFI/fedora/shimx64.efi").as_deref(),
            Some("Fedora")
        );
        assert_eq!(
            label_for(
                &options,
                &PARTUUID.to_uppercase(),
                "/efi/FEDORA/SHIMX64.EFI"
            )
            .as_deref(),
            Some("Fedora")
        );
    }

    #[test]
    fn an_entry_on_another_partition_or_loader_does_not_match() {
        let options = [entry("Fedora", "\\EFI\\fedora\\shimx64.efi")];

        assert_eq!(label_for(&options, PARTUUID, "/EFI/BOOT/BOOTX64.EFI"), None);
        assert_eq!(
            label_for(
                &options,
                "00000000-0000-0000-0000-000000000000",
                "/EFI/fedora/shimx64.efi"
            ),
            None
        );
    }

    #[test]
    fn an_empty_description_is_no_label() {
        let options = [entry("   ", "\\EFI\\BOOT\\BOOTX64.EFI")];

        assert_eq!(label_for(&options, PARTUUID, "/EFI/BOOT/BOOTX64.EFI"), None);
    }
}
