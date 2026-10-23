//! The boot entries the firmware itself keeps, read from `efivarfs`.
//!
//! An operating system that installs a boot loader also registers it in the
//! UEFI boot manager, with the label *it* chose: `Windows Boot Manager`,
//! `Fedora`, `Linux Boot Manager`, `NixOS Boot Manager`. That label is strictly
//! better than anything guessable from a directory name under `EFI/`, and each
//! entry carries the GPT GUID of the partition and the path of the executable
//! it points at - so it can be matched against a loader [`super::scan`] found,
//! rather than merely trusted.
//!
//! Reading `/sys/firmware/efi/efivars` needs root and a machine booted in UEFI
//! mode; everything here degrades to "no label" otherwise, since a boot menu
//! entry is still correct with a less pretty name. The NVRAM can also be stale
//! or wiped (reinstallation, flat CMOS battery, cloned disk), which is why it
//! renames entries instead of producing them.
//!
//! The parsing is `EFI_LOAD_OPTION` and `EFI_DEVICE_PATH` from the UEFI
//! specification, and it is pure byte handling - that is the tested half.

use std::fs;
use std::path::Path;

/// Directory `efivarfs` is mounted at.
const EFIVARS_DIRECTORY: &str = "/sys/firmware/efi/efivars";

/// Vendor GUID of the UEFI global variables, the suffix of every variable name
/// in [`EFIVARS_DIRECTORY`] that this module reads.
const EFI_GLOBAL_VARIABLE: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// Size of the attribute word every `efivarfs` file starts with, before the
/// variable's own data.
const EFIVARFS_ATTRIBUTES_SIZE: usize = 4;

/// Device path node type of a media device path.
const DEVICE_PATH_TYPE_MEDIA: u8 = 4;

/// Media device path sub-type of a hard drive node, which carries the
/// partition's GPT GUID.
const DEVICE_PATH_SUBTYPE_HARD_DRIVE: u8 = 1;

/// Media device path sub-type of a file path node.
const DEVICE_PATH_SUBTYPE_FILE_PATH: u8 = 4;

/// Device path node type marking the end of a device path.
const DEVICE_PATH_TYPE_END: u8 = 0x7f;

/// Signature type of a hard drive node whose signature is a GPT partition
/// GUID; `1` would be an MBR disk signature, which is not addressable.
const SIGNATURE_TYPE_GUID: u8 = 2;

/// Length of a hard drive device path node, header included.
const HARD_DRIVE_NODE_SIZE: usize = 42;

/// Length of a device path node header: type, sub-type, length.
const DEVICE_PATH_HEADER_SIZE: usize = 4;

/// One entry of the UEFI boot manager.
///
/// # Fields
/// * `description` - the label the operating system registered, e.g.
///   `"Windows Boot Manager"`.
/// * `partuuid` - GPT GUID of the partition the entry points at, lowercase,
///   absent when the entry names no GPT partition (a PXE or MBR entry).
/// * `efi_path` - path of the executable inside that partition, `/`-separated
///   and lowercase for comparison, absent when the entry carries no file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BootOption {
    pub(super) description: String,
    pub(super) partuuid: Option<String>,
    pub(super) efi_path: Option<String>,
}

/// Reads a little-endian `u16` at `offset`.
///
/// # Parameters
/// * `bytes` - the buffer to read from.
/// * `offset` - byte offset to read at.
///
/// # Returns
/// The value, or `None` when the buffer is too short.
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Decodes a UTF-16LE string up to its first NUL.
///
/// # Parameters
/// * `bytes` - the buffer holding the string; an odd trailing byte is ignored.
///
/// # Returns
/// The decoded text, with unpaired surrogates replaced, stopping at the NUL
/// terminator. Empty when the first unit is already NUL.
fn utf16_until_nul(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Number of bytes a UTF-16LE NUL-terminated string occupies at the start of
/// `bytes`, terminator included.
///
/// # Parameters
/// * `bytes` - the buffer the string starts at.
///
/// # Returns
/// The length, or `None` when no terminator is present.
fn utf16_size_with_nul(bytes: &[u8]) -> Option<usize> {
    bytes
        .chunks_exact(2)
        .position(|pair| pair == [0, 0])
        .map(|units| (units + 1) * 2)
}

/// Formats the 16 bytes of a GPT partition GUID the way the partition table
/// spells it.
///
/// # Parameters
/// * `bytes` - the signature field of a hard drive device path node, which must
///   be 16 bytes long.
///
/// # Returns
/// The GUID in lowercase canonical form, e.g.
/// `"49eaf743-20fe-44fa-b9fe-b0944153ac72"`, or `None` on a short slice. Its
/// first three fields are stored little-endian and the rest big-endian, exactly
/// as in the GPT itself, so the result compares directly with `lsblk`'s
/// `PARTUUID`.
fn format_guid(bytes: &[u8]) -> Option<String> {
    let b: [u8; 16] = bytes.try_into().ok()?;

    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    ))
}

/// Normalises an EFI file path for comparison.
///
/// # Parameters
/// * `path` - the path as a file path device path node spells it, e.g.
///   `"\\EFI\\Microsoft\\Boot\\bootmgfw.efi"`.
///
/// # Returns
/// The same path with `\` turned into `/`, a leading `/` guaranteed and the
/// whole thing lowercased - FAT does not preserve case, and the firmware does
/// not spell it like the directory entry does.
fn normalise_efi_path(path: &str) -> String {
    let slashed = path.replace('\\', "/").to_lowercase();
    if slashed.starts_with('/') {
        slashed
    } else {
        format!("/{slashed}")
    }
}

/// Walks an `EFI_DEVICE_PATH` list and keeps what identifies a loader.
///
/// # Parameters
/// * `bytes` - the device path list, which ends at an end-of-path node or at
///   the end of the slice.
///
/// # Returns
/// `(partuuid, efi_path)`: the GPT GUID of the hard drive node, if any, and the
/// concatenation of the file path nodes, normalised by [`normalise_efi_path`].
/// Either may be absent; a malformed node stops the walk rather than failing
/// the whole variable.
fn parse_device_path(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let mut partuuid = None;
    let mut path = String::new();
    let mut offset = 0;

    while offset + DEVICE_PATH_HEADER_SIZE <= bytes.len() {
        let node_type = bytes[offset];
        let subtype = bytes[offset + 1];
        let Some(length) = read_u16(bytes, offset + 2).map(usize::from) else {
            break;
        };

        if node_type == DEVICE_PATH_TYPE_END || length < DEVICE_PATH_HEADER_SIZE {
            break;
        }
        let Some(node) = bytes.get(offset..offset + length) else {
            break;
        };

        if node_type == DEVICE_PATH_TYPE_MEDIA {
            match subtype {
                DEVICE_PATH_SUBTYPE_HARD_DRIVE
                    if length >= HARD_DRIVE_NODE_SIZE && node[41] == SIGNATURE_TYPE_GUID =>
                {
                    partuuid = format_guid(&node[24..40]);
                }
                DEVICE_PATH_SUBTYPE_FILE_PATH => {
                    path.push_str(&utf16_until_nul(&node[DEVICE_PATH_HEADER_SIZE..]));
                }
                _ => {}
            }
        }

        offset += length;
    }

    let efi_path = if path.is_empty() {
        None
    } else {
        Some(normalise_efi_path(&path))
    };
    (partuuid, efi_path)
}

/// Parses an `EFI_LOAD_OPTION`.
///
/// # Parameters
/// * `bytes` - the variable's data, *without* the `efivarfs` attribute word:
///   `u32` attributes, `u16` file path list length, NUL-terminated UTF-16
///   description, then the device path list.
///
/// # Returns
/// The entry, or `None` when the buffer is too short or its description is not
/// terminated.
fn parse_load_option(bytes: &[u8]) -> Option<BootOption> {
    let file_path_length = usize::from(read_u16(bytes, 4)?);

    let description_offset = 6;
    let description_bytes = bytes.get(description_offset..)?;
    let description_size = utf16_size_with_nul(description_bytes)?;
    let description = utf16_until_nul(description_bytes);

    let device_path_offset = description_offset + description_size;
    let device_path = bytes
        .get(device_path_offset..device_path_offset + file_path_length)
        .or_else(|| bytes.get(device_path_offset..))
        .unwrap_or_default();
    let (partuuid, efi_path) = parse_device_path(device_path);

    Some(BootOption {
        description,
        partuuid,
        efi_path,
    })
}

/// Parses one `efivarfs` file holding an `EFI_LOAD_OPTION`.
///
/// # Parameters
/// * `raw` - the whole file content, attribute word included.
///
/// # Returns
/// The entry, or `None` when the file is too short or malformed.
fn parse_boot_variable(raw: &[u8]) -> Option<BootOption> {
    parse_load_option(raw.get(EFIVARFS_ATTRIBUTES_SIZE..)?)
}

/// Parses the `BootOrder` variable.
///
/// # Parameters
/// * `raw` - the whole file content, attribute word included.
///
/// # Returns
/// The boot option numbers, in the order the firmware tries them; empty when
/// the variable is missing or malformed.
fn parse_boot_order(raw: &[u8]) -> Vec<u16> {
    raw.get(EFIVARFS_ATTRIBUTES_SIZE..)
        .unwrap_or_default()
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// Boot option number a variable name encodes.
///
/// # Parameters
/// * `name` - an `efivarfs` file name, e.g.
///   `"Boot0002-8be4df61-93ca-11d2-aa0d-00e098032b8c"`.
///
/// # Returns
/// The number, or `None` when the name is not a global `Boot####` variable.
fn boot_option_number(name: &str) -> Option<u16> {
    let rest = name.strip_prefix("Boot")?;
    let (digits, guid) = rest.split_once('-')?;

    if digits.len() != 4 || !guid.eq_ignore_ascii_case(EFI_GLOBAL_VARIABLE) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

/// Reads every `Boot####` variable of the firmware's boot manager.
///
/// # Pre-conditions
/// The machine booted in UEFI mode and the caller is root; otherwise
/// `/sys/firmware/efi/efivars` is unreadable.
///
/// # Post-conditions
/// Nothing is written: the variables are only read.
///
/// # Returns
/// The entries, ordered by `BootOrder` first and then by option number, so that
/// the firmware's own preference decides when two entries point at the same
/// loader. Empty when the directory cannot be read - a missing label is not an
/// error.
pub(super) fn boot_options() -> Vec<BootOption> {
    let directory = Path::new(EFIVARS_DIRECTORY);

    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut numbered: Vec<(u16, BootOption)> = entries
        .flatten()
        .filter_map(|entry| {
            let number = boot_option_number(&entry.file_name().to_string_lossy())?;
            let raw = fs::read(entry.path()).ok()?;
            Some((number, parse_boot_variable(&raw)?))
        })
        .collect();
    numbered.sort_by_key(|(number, _)| *number);

    let order = fs::read(directory.join(format!("BootOrder-{EFI_GLOBAL_VARIABLE}")))
        .map(|raw| parse_boot_order(&raw))
        .unwrap_or_default();

    let mut options = Vec::with_capacity(numbered.len());
    for wanted in &order {
        if let Some((_, option)) = numbered.iter().find(|(number, _)| number == wanted) {
            options.push(option.clone());
        }
    }
    for (number, option) in numbered {
        if !order.contains(&number) {
            options.push(option);
        }
    }
    options
}

/// Label the firmware holds for one loader.
///
/// # Parameters
/// * `options` - the entries [`boot_options`] read.
/// * `partuuid` - GPT GUID of the partition the loader lives on.
/// * `efi_path` - its path inside that partition, as [`super::scan`] spells it.
///
/// # Returns
/// The description of the first entry pointing at that very executable on that
/// very partition, trimmed; `None` when no entry does or its description is
/// empty. Comparison is case-insensitive on both the GUID and the path, since
/// neither the firmware nor FAT preserves case.
pub(super) fn label_for(options: &[BootOption], partuuid: &str, efi_path: &str) -> Option<String> {
    let wanted_uuid = partuuid.to_lowercase();
    let wanted_path = normalise_efi_path(efi_path);

    options
        .iter()
        .find(|option| {
            option
                .partuuid
                .as_deref()
                .is_some_and(|uuid| uuid == wanted_uuid)
                && option
                    .efi_path
                    .as_deref()
                    .is_some_and(|path| path == wanted_path)
        })
        .map(|option| option.description.trim().to_string())
        .filter(|description| !description.is_empty())
}

#[cfg(test)]
#[path = "efivars_tests.rs"]
mod tests;
