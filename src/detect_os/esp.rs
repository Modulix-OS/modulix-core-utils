//! Finding the machine's EFI system partitions, and getting one readable.
//!
//! Everything privileged lives here: `lsblk` to enumerate the partitions, and
//! `mount(2)` to make a partition that nobody mounted readable for the length
//! of a scan. Nothing in this file is unit-tested on purpose - it needs a real
//! block device and `CAP_SYS_ADMIN`; what *is* testable, the reading of a
//! mounted partition, lives in [`super::scan`].

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::mount::{MsFlags, mount, umount};
use serde::Deserialize;

use crate::error::{io_error_at, io_message};
use crate::mx;

/// GPT partition type GUID of an EFI system partition.
///
/// Matched instead of `lsblk`'s `PARTTYPENAME` ("EFI System"), which is a
/// human-readable label from libblkid's table rather than a stable key.
const ESP_PARTITION_TYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// Directory under [`crate::cache_dir`] where an ESP that is not already
/// mounted is mounted read-only for the duration of a scan.
const PROBE_DIRECTORY: &str = "esp-probe";

/// Filesystem type an EFI system partition is mounted as.
const ESP_FILESYSTEM: &str = "vfat";

/// An EFI system partition, as `lsblk` reports it.
///
/// # Fields
/// * `device` - the block device path, e.g. `/dev/nvme0n1p1`.
/// * `partuuid` - its GPT partition GUID, absent on a partition table that has
///   none (an MBR disk `lsblk` still typed as an ESP).
/// * `mountpoint` - where it is already mounted, if it is.
pub(super) struct Esp {
    pub(super) device: String,
    pub(super) partuuid: Option<String>,
    pub(super) mountpoint: Option<String>,
}

/// `lsblk --json` top level.
#[derive(Deserialize)]
struct LsblkOutput {
    #[serde(default)]
    blockdevices: Vec<LsblkDevice>,
}

/// One node of the `lsblk --json` device tree.
///
/// Every field is optional because `lsblk` emits `null` for a column that does
/// not apply to the node (a whole disk has no `parttype`, an unmounted
/// partition no `mountpoint`).
#[derive(Deserialize)]
struct LsblkDevice {
    path: Option<String>,
    parttype: Option<String>,
    partuuid: Option<String>,
    mountpoint: Option<String>,
    #[serde(default)]
    children: Vec<LsblkDevice>,
}

/// Read-only mount of an EFI system partition, unmounted on drop.
///
/// The scan returns early on several paths, so the unmount cannot be a
/// statement at the end of a function: `Drop` is what guarantees that a probe
/// never leaks a mount or an empty directory under [`crate::cache_dir`].
pub(super) struct MountedEsp {
    path: PathBuf,
}

impl MountedEsp {
    /// Mounts `device` read-only under `cache_dir()/esp-probe/<key>`.
    ///
    /// # Parameters
    /// * `device` - block device of the partition to mount.
    /// * `key` - directory name to mount it under; must not contain a path
    ///   separator.
    ///
    /// # Pre-conditions
    /// The caller has `CAP_SYS_ADMIN`, and `device` is not already mounted
    /// (an already-mounted ESP is read in place instead).
    ///
    /// # Post-conditions
    /// On success the partition is mounted at [`Self::path`] with `MS_RDONLY`,
    /// `MS_NOSUID`, `MS_NODEV` and `MS_NOEXEC`, and is unmounted when the
    /// returned value is dropped. On failure nothing is mounted and no
    /// directory is left behind.
    ///
    /// # Returns
    /// The guard owning the mount.
    ///
    /// # Errors
    /// * `mx::ErrorKind::IOError` - the mount point could not be created.
    /// * `mx::ErrorKind::UnixError` - `mount(2)` failed (not privileged, not a
    ///   FAT filesystem, device gone).
    pub(super) fn mount_read_only(device: &str, key: &str) -> mx::Result<Self> {
        let path = crate::cache_dir().join(PROBE_DIRECTORY).join(key);
        fs::create_dir_all(&path).map_err(|e| io_error_at(&path.to_string_lossy(), e))?;

        let flags =
            MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC;
        match mount(
            Some(device),
            &path,
            Some(ESP_FILESYSTEM),
            flags,
            None::<&str>,
        ) {
            Ok(()) => Ok(Self { path }),
            Err(e) => {
                let _ = fs::remove_dir(&path);
                Err(mx::ErrorKind::UnixError(e))
            }
        }
    }

    /// Where the partition is mounted.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MountedEsp {
    fn drop(&mut self) {
        let _ = umount(&self.path);
        let _ = fs::remove_dir(&self.path);
    }
}

/// Removes the probe directory, now that every mount under it is gone.
///
/// # Post-conditions
/// `cache_dir()/esp-probe` no longer exists when it was empty; a failure is
/// ignored, since a leftover empty directory is of no consequence.
pub(super) fn clean_probe_directory() {
    let _ = fs::remove_dir(crate::cache_dir().join(PROBE_DIRECTORY));
}

/// Lists the EFI system partitions of the machine.
///
/// # Pre-conditions
/// `lsblk` is on `PATH` (it is wrapped into `mx-init`'s via `util-linux`).
///
/// # Returns
/// One [`Esp`] per partition whose GPT type is [`ESP_PARTITION_TYPE`], in the
/// order `lsblk` walks the device tree. Empty when the machine has none.
///
/// # Errors
/// * `mx::ErrorKind::CommandError` - `lsblk` could not be run or exited
///   non-zero; payload carries its stderr.
/// * `mx::ErrorKind::ParseError` - its JSON could not be deserialised.
pub(super) fn list_esp() -> mx::Result<Vec<Esp>> {
    let output = Command::new("lsblk")
        .args([
            "--json",
            "--paths",
            "--output",
            "PATH,PARTTYPE,PARTUUID,MOUNTPOINT",
        ])
        .output()
        .map_err(|e| mx::ErrorKind::CommandError(format!("lsblk: {}", io_message(&e))))?;

    if !output.status.success() {
        return Err(mx::ErrorKind::CommandError(format!(
            "lsblk exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let parsed: LsblkOutput =
        serde_json::from_slice(&output.stdout).map_err(mx::ErrorKind::ParseError)?;

    let mut esps = Vec::new();
    collect_esp(&parsed.blockdevices, &mut esps);
    Ok(esps)
}

/// Walks an `lsblk` device tree and keeps the EFI system partitions.
///
/// # Parameters
/// * `devices` - the nodes to walk, children included.
/// * `out` - receives one entry per EFI system partition found.
///
/// # Post-conditions
/// `out` is appended to; nothing already in it is touched.
fn collect_esp(devices: &[LsblkDevice], out: &mut Vec<Esp>) {
    for device in devices {
        let is_esp = device
            .parttype
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(ESP_PARTITION_TYPE));

        if is_esp && let Some(path) = &device.path {
            out.push(Esp {
                device: path.clone(),
                partuuid: device.partuuid.clone(),
                mountpoint: device.mountpoint.clone(),
            });
        }

        collect_esp(&device.children, out);
    }
}
