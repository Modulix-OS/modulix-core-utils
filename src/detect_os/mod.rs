//! Detects the operating systems already installed on the machine, so the
//! Modulix bootloader can offer them instead of hiding them.
//!
//! The question answered here is deliberately narrow: *which EFI executables,
//! other than ours, can Limine chainload, and on which EFI system partition do
//! they live?* That is exactly the shape of a Limine `efi_chainload` entry, and
//! [`crate::boot_config`] turns the answer into `mx.bootloader.extraEntries`
//! without having to know anything more.
//!
//! Only the EFI system partitions are looked at. An ESP is self-describing -
//! `EFI/Microsoft/Boot/bootmgfw.efi` *is* Windows' boot manager, no matter what
//! the partitions around it hold - whereas `/etc/os-release` on a neighbouring
//! root says what a system is but not how to boot it, and the UEFI boot
//! variables routinely outlive the disk they point at.
//!
//! Detection is best effort: one unreadable partition is skipped, not fatal.
//! An installation must not fail over a boot menu entry. Reading an ESP needs
//! root (the mount point is root-owned, and mounting one at all is privileged),
//! so an unprivileged run simply finds nothing.
//!
//! The split between the two private sub-modules is the privilege boundary:
//! [`esp`] enumerates and mounts partitions, which needs a real machine, while
//! [`scan`] only reads below a directory and carries the whole naming and
//! exclusion policy, which is why that half is the tested one.

mod efivars;
mod esp;
mod scan;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::mx;

/// One boot entry for an operating system other than this installation.
///
/// # Fields
/// * `name` - label to show in the boot menu, e.g. `"Windows Boot Manager"`.
///   Free of `/`, `\r` and `\n`: Limine reads `/` as its entry separator.
/// * `efi_path` - absolute path of the EFI executable *inside its own ESP*,
///   with the case it carries on disk, e.g.
///   `"/EFI/Microsoft/Boot/bootmgfw.efi"`.
/// * `esp` - GPT partition GUID (`PARTUUID`) of the ESP holding `efi_path`, or
///   `None` when that ESP is the one this installation boots from. `None`
///   renders as Limine's `boot()`, which stays correct even if the partition is
///   recreated with a new GUID.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct OtherOs {
    pub name: String,
    pub efi_path: String,
    pub esp: Option<String>,
}

impl PartialOrd for OtherOs {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OtherOs {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.name, &self.efi_path, &self.esp).cmp(&(&other.name, &other.efi_path, &other.esp))
    }
}

/// Canonical form of a mount point, for comparing two spellings of one path.
///
/// # Parameters
/// * `path` - the mount point to normalise.
///
/// # Returns
/// `path` resolved through the filesystem when that succeeds, else `path` with
/// its trailing slashes removed. The fallback matters: during an installation
/// the ESP of the target system is given as `/mnt/boot`, which exists, but a
/// caller may also name a path that does not.
fn normalise_mount_point(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path.trim_end_matches('/')))
}

/// Name of the disk a partition lies on, for labelling a boot entry.
///
/// # Parameters
/// * `device` - the partition's block device path, e.g. `/dev/nvme0n1p1`.
///
/// # Returns
/// The device's base name without its partition suffix: `p` followed by digits
/// when the name ends that way (`nvme0n1p1` -> `nvme0n1`, `mmcblk0p1` ->
/// `mmcblk0`), else the trailing digits (`sda1` -> `sda`). A name carrying no
/// digit at all comes back unchanged.
fn disk_hint(device: &str) -> String {
    let name = Path::new(device)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.len() == name.len() {
        return name;
    }

    trimmed
        .strip_suffix('p')
        .filter(|stem| !stem.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Label the firmware's own boot manager holds for a loader, if it holds one.
///
/// Preferred over the name [`scan`] derives from the directory tree: it is the
/// label the other operating system registered for itself, so it says `Fedora`
/// or `Windows Boot Manager` where a directory can only say `Other system`.
///
/// # Parameters
/// * `boot_options` - the firmware's entries, as [`efivars::boot_options`]
///   read them.
/// * `partition` - the EFI system partition the loader was found on; an entry
///   matches only through its GPT GUID, so a partition without one is never
///   renamed.
/// * `efi_path` - the loader's path inside that partition.
///
/// # Returns
/// The label, stripped of the characters Limine reads as syntax, or `None`
/// when no entry points at that loader, the firmware's variables could not be
/// read, or the label would be empty.
fn firmware_label(
    boot_options: &[efivars::BootOption],
    partition: &esp::Esp,
    efi_path: &str,
) -> Option<String> {
    let partuuid = partition.partuuid.as_deref()?;

    let label = scan::sanitise_name(&efivars::label_for(boot_options, partuuid, efi_path)?);
    if label.is_empty() {
        return None;
    }
    Some(label)
}

/// Scans every EFI system partition of the machine and builds the boot entries
/// of the other operating systems installed on it.
///
/// A partition already mounted is read where it is; any other is mounted
/// read-only under [`crate::cache_dir`] for the duration of the scan and
/// unmounted straight after, whatever the outcome.
///
/// # Parameters
/// * `own_esp_mount` - where *this* installation's ESP is mounted: `"/boot"` on
///   a running system, `"<root>/boot"` during an installation. Entries found
///   there get `esp: None`, i.e. Limine's `boot()`. A path that matches nothing
///   is not an error, but it is reported on stderr and every partition is then
///   scanned as if it were ours: taking our own ESP for somebody else's would
///   add an entry chainloading our own Limine.
///
/// # Pre-conditions
/// `lsblk` is on `PATH`. Reading an ESP needs root; without it every partition
/// is skipped and the result is empty.
///
/// # Post-conditions
/// No partition is left mounted and no probe directory is left behind. Nothing
/// is written to any ESP.
///
/// # Returns
/// One [`OtherOs`] per operating system found, deduplicated and sorted by
/// label then path. Each is named after the firmware's own boot entry when one
/// points at that very loader ([`firmware_label`]), else after the directory it
/// was found in. A partition that cannot be mounted or read is reported on
/// stderr and skipped: a missing boot entry must never fail an installation.
///
/// # Errors
/// * `mx::ErrorKind::CommandError` - `lsblk` could not be run or failed.
/// * `mx::ErrorKind::ParseError` - its output could not be deserialised.
pub fn detect(own_esp_mount: &str) -> mx::Result<Vec<OtherOs>> {
    let own = normalise_mount_point(own_esp_mount);
    let partitions = esp::list_esp()?;

    let is_own_mount = |partition: &esp::Esp| {
        partition
            .mountpoint
            .as_deref()
            .is_some_and(|m| normalise_mount_point(m) == own)
    };
    let own_found = partitions.iter().any(is_own_mount);
    if !own_found {
        eprintln!(
            "detect_os: no EFI system partition is mounted at {own_esp_mount}, \
             scanning every one of them as our own"
        );
    }

    let boot_options = efivars::boot_options();
    let mut entries = BTreeSet::new();

    for partition in &partitions {
        let is_own = !own_found || is_own_mount(partition);

        if !is_own && partition.partuuid.is_none() {
            eprintln!(
                "detect_os: {} has no partition GUID, skipping it",
                partition.device
            );
            continue;
        }

        let mounted;
        let root: &Path = match &partition.mountpoint {
            Some(mountpoint) => Path::new(mountpoint),
            None => {
                let key = partition
                    .partuuid
                    .clone()
                    .unwrap_or_else(|| partition.device.replace('/', "_"));
                match esp::MountedEsp::mount_read_only(&partition.device, &key) {
                    Ok(guard) => {
                        mounted = guard;
                        mounted.path()
                    }
                    Err(e) => {
                        eprintln!("detect_os: cannot read {}: {}", partition.device, e);
                        continue;
                    }
                }
            }
        };

        for (name, efi_path) in scan::scan_esp(root, is_own, &disk_hint(&partition.device)) {
            entries.insert(OtherOs {
                name: firmware_label(&boot_options, partition, &efi_path).unwrap_or(name),
                efi_path,
                esp: if is_own {
                    None
                } else {
                    partition.partuuid.clone()
                },
            });
        }
    }

    esp::clean_probe_directory();

    Ok(entries.into_iter().collect())
}

#[cfg(test)]
#[path = "detect_os_tests.rs"]
mod tests;
