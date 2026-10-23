//! Reading a mounted EFI system partition: which boot loader belongs to which
//! operating system, and what to call it.
//!
//! Nothing here touches a block device or a subprocess - it only reads below a
//! directory, which is what makes the whole naming and exclusion policy
//! testable against a fixture tree.
//!
//! The policy is not the same on both sides of the machine. On *our* ESP the
//! loaders under `EFI/limine`, `EFI/systemd`, `EFI/nixos`, `EFI/Linux` and
//! `EFI/BOOT` are ours, and Limine already lists the generations they boot, so
//! they are excluded and every other vendor directory becomes an entry. On a
//! *foreign* ESP those very directories are the other system's own loaders, so
//! the partition yields exactly one entry pointing at its boot loader - which
//! is then the one listing its own kernels.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Name of the directory an ESP keeps its boot loaders in.
const EFI_DIRECTORY: &str = "EFI";

/// Vendor directories under `EFI/` that never describe an operating system, on
/// any partition: firmware utilities.
///
/// Lowercase; compared case-insensitively, because FAT does not preserve case
/// reliably.
const FIRMWARE_VENDORS: &[&str] = &["tools", "memtest86", "memtest86+"];

/// Vendor directories that hold a boot loader without naming a distribution,
/// most informative first.
///
/// On our own ESP they are ours and are skipped. On a foreign ESP they are what
/// the other system boots with, and the display name here is the best label
/// available - qualified with the disk it was found on, since two foreign ESPs
/// would otherwise land in the menu under one name.
///
/// * `nixos`, `limine` - another Modulix or NixOS installation.
/// * `systemd`, `linux` - systemd-boot and the unified kernel images it loads.
/// * `boot` - `EFI/BOOT/BOOTX64.EFI`, the removable-media fallback, which is
///   also what the firmware launches by default on that disk.
const GENERIC_VENDOR_NAMES: &[(&str, &str)] = &[
    ("nixos", "NixOS"),
    ("limine", "Modulix OS"),
    ("systemd", "Linux"),
    ("linux", "Linux"),
    ("boot", "Other system"),
];

/// Directories to look for a foreign ESP's own boot loader in, most preferred
/// first; anything else it holds is tried afterwards.
///
/// `BOOT` comes first because `EFI/BOOT/BOOTX64.EFI` is the copy the firmware
/// launches when it boots that disk, so it is literally the other system's boot
/// loader. The vendor directories are the fallback for an ESP that has no such
/// copy, which is the usual shape of a Windows-only partition.
const OTHER_BOOTLOADER_DIRECTORIES: &[&str] = &["BOOT", "systemd", "limine", "nixos", "Linux"];

/// Boot loader file names to chainload, most preferred first.
///
/// Lowercase; compared case-insensitively. `shim` comes before `grub` on
/// purpose: it is the Secure Boot entry point of every distribution that ships
/// one, and chainloading `grubx64.efi` directly breaks the signature chain on a
/// machine with Secure Boot enabled. The `boot*.efi` names come last so that a
/// vendor directory holding both its real loader and a copy of the fallback
/// still yields the real one; they are listed at all so that `EFI/BOOT` is
/// picked by name rather than by the sorted-first-`*.efi` fallback, which would
/// hand back `fbx64.efi` on a partition that sorts it first.
const LOADER_PREFERENCE: &[&str] = &[
    "bootmgfw.efi",
    "shimx64.efi",
    "shimaa64.efi",
    "shimia32.efi",
    "grubx64.efi",
    "grubaa64.efi",
    "grub.efi",
    "systemd-bootx64.efi",
    "bootx64.efi",
    "bootaa64.efi",
    "bootia32.efi",
    "boot.efi",
];

/// Display names for the vendor directories commonly found on an ESP.
///
/// Keys are lowercase and matched case-insensitively against the directory
/// name. A directory absent from this table is named after itself, with its
/// first character upper-cased.
const VENDOR_NAMES: &[(&str, &str)] = &[
    ("microsoft", "Windows Boot Manager"),
    ("apple", "macOS"),
    ("ubuntu", "Ubuntu"),
    ("debian", "Debian"),
    ("fedora", "Fedora"),
    ("arch", "Arch Linux"),
    ("opensuse", "openSUSE"),
    ("suse", "openSUSE"),
    ("manjaro", "Manjaro"),
    ("pop", "Pop!_OS"),
    ("linuxmint", "Linux Mint"),
    ("mint", "Linux Mint"),
    ("zorin", "Zorin OS"),
    ("endeavouros", "EndeavourOS"),
    ("gentoo", "Gentoo"),
    ("centos", "CentOS"),
    ("rhel", "Red Hat Enterprise Linux"),
    ("redhat", "Red Hat Enterprise Linux"),
    ("void", "Void Linux"),
    ("kali", "Kali Linux"),
    ("elementary", "elementary OS"),
    ("garuda", "Garuda Linux"),
    ("nobara", "Nobara"),
    ("cachyos", "CachyOS"),
    ("devuan", "Devuan"),
    ("slackware", "Slackware"),
    ("alpine", "Alpine Linux"),
    ("solus", "Solus"),
    ("steamos", "SteamOS"),
];

/// Child of `dir` whose name equals `name`, ignoring case.
///
/// FAT does not preserve case reliably, so `EFI`, `efi` and `Efi` all have to
/// be found - while the name actually stored on disk is what must end up in the
/// boot entry, since Limine reads the path back from the same filesystem.
///
/// # Parameters
/// * `dir` - directory to look in.
/// * `name` - name to look for, in any case.
///
/// # Returns
/// The matching entry's path, with its on-disk spelling, or `None` when `dir`
/// cannot be read or holds no such child.
fn child_ignore_case(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            return Some(entry.path());
        }
    }
    None
}

/// Sub-directories of `dir`, sorted by name.
///
/// Sorted so that a scan of the same ESP always yields the same entries in the
/// same order - `read_dir` order is whatever the filesystem hands back.
///
/// # Parameters
/// * `dir` - directory to list.
///
/// # Returns
/// The paths of its sub-directories, or an empty vector when `dir` cannot be
/// read.
fn sorted_subdirectories(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// EFI executables directly inside `dir`, sorted by name.
///
/// # Parameters
/// * `dir` - directory to list.
///
/// # Returns
/// The paths of its `*.efi` files, in any case, or an empty vector when `dir`
/// cannot be read.
fn sorted_efi_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("efi"))
        })
        .collect();
    files.sort();
    files
}

/// Picks the EFI executable to chainload for one vendor directory.
///
/// Looks for each name of [`LOADER_PREFERENCE`] in turn, first in `vendor_dir`
/// itself and then in its sub-directories - Windows keeps its boot manager one
/// level down, in `EFI/Microsoft/Boot/`. When no preferred name is found, the
/// first `*.efi` encountered in that same order is taken, so an unknown
/// distribution still gets an entry.
///
/// # Parameters
/// * `vendor_dir` - a directory under `EFI/`, e.g. `<esp>/EFI/Microsoft`.
///
/// # Returns
/// The path of the chosen executable, or `None` when the directory holds no
/// `*.efi` at either depth.
fn pick_loader(vendor_dir: &Path) -> Option<PathBuf> {
    let subdirectories = sorted_subdirectories(vendor_dir);

    for wanted in LOADER_PREFERENCE {
        if let Some(found) = child_ignore_case(vendor_dir, wanted)
            && found.is_file()
        {
            return Some(found);
        }
        for subdirectory in &subdirectories {
            if let Some(found) = child_ignore_case(subdirectory, wanted)
                && found.is_file()
            {
                return Some(found);
            }
        }
    }

    if let Some(first) = sorted_efi_files(vendor_dir).into_iter().next() {
        return Some(first);
    }
    subdirectories
        .iter()
        .find_map(|d| sorted_efi_files(d).into_iter().next())
}

/// Whether a vendor directory holds a firmware utility rather than a system.
///
/// # Parameters
/// * `vendor` - the directory name as it is spelled on disk.
///
/// # Returns
/// `true` when it is listed in [`FIRMWARE_VENDORS`], ignoring case.
fn is_firmware_vendor(vendor: &str) -> bool {
    FIRMWARE_VENDORS
        .iter()
        .any(|own| vendor.eq_ignore_ascii_case(own))
}

/// Display name of a vendor directory that names no distribution.
///
/// # Parameters
/// * `vendor` - the directory name as it is spelled on disk.
///
/// # Returns
/// Its entry in [`GENERIC_VENDOR_NAMES`], or `None` when the directory does
/// name a distribution - which also means it is one of the directories excluded
/// on our own ESP.
fn generic_vendor_name(vendor: &str) -> Option<&'static str> {
    GENERIC_VENDOR_NAMES
        .iter()
        .find(|(key, _)| vendor.eq_ignore_ascii_case(key))
        .map(|(_, name)| *name)
}

/// Display name for a vendor directory.
///
/// # Parameters
/// * `vendor` - the directory name as it is spelled on disk.
///
/// # Returns
/// Its entry in [`VENDOR_NAMES`] when it has one, else `vendor` with its first
/// character upper-cased.
fn display_name(vendor: &str) -> String {
    if let Some((_, name)) = VENDOR_NAMES
        .iter()
        .find(|(key, _)| vendor.eq_ignore_ascii_case(key))
    {
        return (*name).to_string();
    }

    let mut chars = vendor.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Qualifies a label that names no distribution with the disk it was found on.
///
/// # Parameters
/// * `name` - the generic label, e.g. `"Other system"`.
/// * `hint` - the disk the ESP belongs to, e.g. `"nvme0n1"`; may be empty.
///
/// # Returns
/// `"<name> (<hint>)"`, or `name` alone when `hint` is empty. Two foreign ESPs
/// that both fall back to a generic label would otherwise be indistinguishable
/// in the boot menu.
fn with_disk_hint(name: &str, hint: &str) -> String {
    if hint.is_empty() {
        return name.to_string();
    }
    format!("{name} ({hint})")
}

/// Strips from a menu label the characters Limine reads as syntax.
///
/// Limine's configuration introduces an entry with `/` and a sub-entry with
/// `//`, and the label runs to the end of the line. A directory name carrying
/// either would silently restructure the menu.
///
/// # Parameters
/// * `name` - the candidate label.
///
/// # Returns
/// `name` without `/`, `\r` and `\n`, trimmed.
pub(super) fn sanitise_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\r' | '\n'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Name of a directory, as it is spelled on disk.
///
/// # Parameters
/// * `dir` - the path to name.
///
/// # Returns
/// Its last component, or `None` when it has none.
fn directory_name(dir: &Path) -> Option<String> {
    dir.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Renders the path of a loader as it must be written in a boot entry.
///
/// # Parameters
/// * `efi_name` - the `EFI` directory's name with its on-disk case.
/// * `efi_dir` - that directory's path, which `loader` lies below.
/// * `loader` - the chosen executable.
///
/// # Returns
/// The path of `loader` inside its partition, absolute, `/`-separated and with
/// the case stored on disk, or `None` when `loader` is not below `efi_dir`.
fn entry_path(efi_name: &str, efi_dir: &Path, loader: &Path) -> Option<String> {
    let relative = loader.strip_prefix(efi_dir).ok()?;

    let mut path = format!("/{efi_name}");
    for component in relative.components() {
        path.push('/');
        path.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(path)
}

/// Scans the `EFI` directory of *our own* ESP.
///
/// # Parameters
/// * `efi_dir` - the partition's `EFI` directory.
/// * `efi_name` - that directory's name with its on-disk case.
///
/// # Returns
/// One `(label, efi_path)` per vendor directory that is neither one of ours
/// ([`GENERIC_VENDOR_NAMES`]) nor a firmware utility
/// ([`FIRMWARE_VENDORS`]), sorted by label. A Windows installed on the same
/// partition as Modulix is found here.
fn scan_own_esp(efi_dir: &Path, efi_name: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();

    for vendor_dir in sorted_subdirectories(efi_dir) {
        let Some(vendor) = directory_name(&vendor_dir) else {
            continue;
        };

        if is_firmware_vendor(&vendor) || generic_vendor_name(&vendor).is_some() {
            continue;
        }

        let Some(loader) = pick_loader(&vendor_dir) else {
            continue;
        };
        let Some(efi_path) = entry_path(efi_name, efi_dir, &loader) else {
            continue;
        };

        let name = sanitise_name(&display_name(&vendor));
        if name.is_empty() {
            continue;
        }

        found.push((name, efi_path));
    }

    found.sort();
    found
}

/// Picks the boot loader of a foreign ESP, and the vendor directory it came
/// from.
///
/// # Parameters
/// * `efi_dir` - the partition's `EFI` directory.
///
/// # Returns
/// `(vendor directory, loader)` for the executable to chainload: the first
/// directory of [`OTHER_BOOTLOADER_DIRECTORIES`] that holds one, else the first
/// of the remaining sub-directories in sorted order that does. `None` when the
/// partition holds no `*.efi` outside a firmware utility.
fn pick_other_bootloader(efi_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    for wanted in OTHER_BOOTLOADER_DIRECTORIES {
        if let Some(vendor_dir) = child_ignore_case(efi_dir, wanted)
            && let Some(loader) = pick_loader(&vendor_dir)
        {
            return Some((vendor_dir, loader));
        }
    }

    sorted_subdirectories(efi_dir).into_iter().find_map(|dir| {
        let vendor = directory_name(&dir)?;
        if is_firmware_vendor(&vendor) {
            return None;
        }
        let loader = pick_loader(&dir)?;
        Some((dir, loader))
    })
}

/// Label for the single entry of a foreign ESP.
///
/// A loader taken from a distribution's own directory is named after it. One
/// taken from a directory that names no distribution - `EFI/BOOT` above all -
/// is named after the most informative such directory on the partition, so an
/// ESP holding `EFI/nixos` next to `EFI/BOOT` says `NixOS` rather than
/// `Other system`. The distribution directories are deliberately *not*
/// consulted in that case: on a partition shared by Windows and a distribution,
/// `EFI/BOOT/BOOTX64.EFI` is whichever of the two installed it last, so naming
/// it after either one would be a guess.
///
/// # Parameters
/// * `efi_dir` - the partition's `EFI` directory.
/// * `vendor_dir` - the directory the chosen loader came from.
/// * `hint` - the disk the ESP belongs to, e.g. `"nvme0n1"`; may be empty.
///
/// # Returns
/// The menu label, or `None` when it would be empty.
fn other_bootloader_label(efi_dir: &Path, vendor_dir: &Path, hint: &str) -> Option<String> {
    let vendor = directory_name(vendor_dir)?;

    let label = if generic_vendor_name(&vendor).is_some() {
        let vendors: Vec<String> = sorted_subdirectories(efi_dir)
            .iter()
            .filter_map(|d| directory_name(d))
            .collect();

        let (_, name) = GENERIC_VENDOR_NAMES
            .iter()
            .find(|(key, _)| vendors.iter().any(|v| v.eq_ignore_ascii_case(key)))?;
        with_disk_hint(name, hint)
    } else {
        display_name(&vendor)
    };

    let label = sanitise_name(&label);
    if label.is_empty() {
        return None;
    }
    Some(label)
}

/// Scans the `EFI` directory of an ESP belonging to another system.
///
/// # Parameters
/// * `efi_dir` - the partition's `EFI` directory.
/// * `efi_name` - that directory's name with its on-disk case.
/// * `hint` - the disk the ESP belongs to, e.g. `"nvme0n1"`; may be empty.
///
/// # Returns
/// At most one `(label, efi_path)`: the partition's own boot loader, which is
/// then the one listing that system's kernels. Empty when the partition holds
/// nothing bootable.
fn scan_other_esp(efi_dir: &Path, efi_name: &str, hint: &str) -> Vec<(String, String)> {
    let Some((vendor_dir, loader)) = pick_other_bootloader(efi_dir) else {
        return Vec::new();
    };
    let Some(efi_path) = entry_path(efi_name, efi_dir, &loader) else {
        return Vec::new();
    };
    let Some(name) = other_bootloader_label(efi_dir, &vendor_dir, hint) else {
        return Vec::new();
    };

    vec![(name, efi_path)]
}

/// Scans one mounted EFI system partition for the boot loaders of other
/// operating systems.
///
/// # Parameters
/// * `root` - the mount point of the partition to scan.
/// * `is_own` - whether this is the ESP this installation boots from. On ours
///   the loaders listed in [`GENERIC_VENDOR_NAMES`] are skipped - they are ours,
///   and Limine already lists what they boot - and every other vendor directory
///   becomes an entry. On a foreign one those same directories are the other
///   system's own loaders, and the partition yields a single entry pointing at
///   its boot loader.
/// * `hint` - the disk the partition belongs to, e.g. `"nvme0n1"`, used to
///   qualify a label that names no distribution; may be empty.
///
/// # Post-conditions
/// Nothing under `root` is written or modified.
///
/// # Returns
/// `(label, efi_path)` per operating system found, sorted by label.
/// `efi_path` is absolute inside the partition, uses `/` separators and keeps
/// the case stored on disk. Firmware utilities ([`FIRMWARE_VENDORS`]) are never
/// entries, on either side, and a partition with no `EFI/` directory at all
/// yields nothing.
pub(super) fn scan_esp(root: &Path, is_own: bool, hint: &str) -> Vec<(String, String)> {
    let Some(efi_dir) = child_ignore_case(root, EFI_DIRECTORY) else {
        return Vec::new();
    };

    let efi_name = directory_name(&efi_dir).unwrap_or_else(|| EFI_DIRECTORY.to_string());

    if is_own {
        scan_own_esp(&efi_dir, &efi_name)
    } else {
        scan_other_esp(&efi_dir, &efi_name, hint)
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
