//! Machine identity and peripherals, read from DMI and `sysfs`: what the
//! `nixos-hardware` machine modules are matched against.

use std::fs;
use std::path::Path;

use crate::error::io_error_at;
use crate::mx;

/// The `sysfs` mount point the probes read.
const SYSFS_ROOT: &str = "/sys";

/// The machine's identity plus the devices worth knowing about when picking
/// modules.
///
/// # Fields
/// * `vendor` - DMI system vendor, lowercased and normalised.
/// * `product_family` - DMI product family, lowercased and normalised.
/// * `product_name` - DMI product name, raw (trailing newline included).
/// * `disk` - names of the `sd*`/`nvme*` block devices found in `/sys/block`.
#[derive(Debug)]
pub struct ComputerInfo {
    vendor: String,
    product_family: String,
    product_name: String,
    disk: Vec<String>,
}
impl ComputerInfo {
    /// Builds a [`ComputerInfo`] from canned DMI values, for tests.
    ///
    /// # Parameters
    /// * `vendor` - the DMI system vendor, already lowercased and normalised.
    /// * `product_family` - the DMI product family, already lowercased and
    ///   normalised.
    /// * `product_name` - the DMI product name.
    /// * `disk` - the block-device names.
    ///
    /// # Returns
    /// A [`ComputerInfo`] holding exactly those values, without reading DMI.
    #[cfg(test)]
    pub(crate) fn from_parts(
        vendor: &str,
        product_family: &str,
        product_name: &str,
        disk: Vec<String>,
    ) -> ComputerInfo {
        ComputerInfo {
            vendor: vendor.to_string(),
            product_family: product_family.to_string(),
            product_name: product_name.to_string(),
            disk,
        }
    }

    /// Vendor strings rewritten to the spelling `nixos-hardware` uses.
    const HARDWARE_VENDOR_REPLACMENT: [(&'static str, &'static str); 2] =
        [("Hewlett-Packard", "hp"), ("Hewlett Packard", "hp")];

    /// USB vendor ids whose USB devices are fingerprint readers, lowercase and
    /// without the `0x` prefix, as `sysfs` spells `idVendor`.
    ///
    /// Only vendors that ship no other kind of USB device belong here, since a
    /// match on the id alone is enough to answer `true`. Elan (`04f3`) and
    /// Synaptics (`06cb`) are deliberately absent: both also ship USB touchpads
    /// and keyboards, so their readers are recognised through the product
    /// string instead.
    const FINGERPRINT_USB_VENDORS: [&'static str; 7] = [
        "27c6", // Goodix
        "138a", // Validity Sensors
        "08ff", // AuthenTec
        "147e", // Upek
        "1c7a", // LighTuning
        "298d", // Next Biometrics
        "16d1", // Suprema
    ];

    /// Per-vendor product-family rewrites, for the families whose DMI spelling
    /// does not match the module name.
    const FAMILY_EXCEPTION_RULES: [(&'static str, &[(&'static str, &'static str)]); 1] = [(
        "framework",
        &[("13in laptop", "13inch"), ("16in laptop", "16inch")],
    )];

    /// Reads the machine's vendor from DMI.
    ///
    /// # Returns
    /// The normalised vendor: one of [`ComputerInfo::HARDWARE_VENDOR_REPLACMENT`]'s
    /// replacements when it applies, else the raw value lowercased - which keeps
    /// the trailing newline `sysfs` emits.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if `/sys/devices/virtual/dmi/id/sys_vendor`
    /// cannot be read, as happens on a machine without DMI.
    fn grep_vendor() -> mx::Result<String> {
        let vendor = fs::read_to_string("/sys/devices/virtual/dmi/id/sys_vendor")
            .map_err(|e| io_error_at("/sys/devices/virtual/dmi/id/sys_vendor", e))?;
        match Self::HARDWARE_VENDOR_REPLACMENT
            .iter()
            .position(|(s, _)| s.contains(&vendor))
        {
            Some(i) => Ok(Self::HARDWARE_VENDOR_REPLACMENT[i].1.to_string()),
            None => Ok(vendor.to_lowercase()),
        }
    }

    /// Reads the machine's product family from DMI.
    ///
    /// # Parameters
    /// * `vendor` - the normalised vendor, used to pick the rewrite rules.
    ///
    /// # Returns
    /// The family lowercased, or its rewrite when
    /// [`ComputerInfo::FAMILY_EXCEPTION_RULES`] has one for this vendor.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if
    /// `/sys/devices/virtual/dmi/id/product_family` cannot be read.
    fn grep_product_family(vendor: &str) -> mx::Result<String> {
        let family = fs::read_to_string("/sys/devices/virtual/dmi/id/product_family")
            .map_err(|e| io_error_at("/sys/devices/virtual/dmi/id/product_family", e))?
            .to_lowercase();
        let pos_vendor = Self::FAMILY_EXCEPTION_RULES
            .iter()
            .position(|(s, _)| s.eq(&vendor));
        if let Some(pos) = pos_vendor {
            let pos_rule = Self::FAMILY_EXCEPTION_RULES[pos]
                .1
                .iter()
                .position(|(s, _)| s.eq(&family));
            if let Some(posr) = pos_rule {
                return Ok(Self::FAMILY_EXCEPTION_RULES[pos].1[posr].1.to_string());
            }
        }
        return Ok(family);
    }

    /// Reads the machine's product name from DMI.
    ///
    /// # Returns
    /// The raw value, neither lowercased nor trimmed.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if `/sys/devices/virtual/dmi/id/product_name`
    /// cannot be read.
    fn grep_product_name() -> mx::Result<String> {
        fs::read_to_string("/sys/devices/virtual/dmi/id/product_name")
            .map_err(|e| io_error_at("/sys/devices/virtual/dmi/id/product_name", e))
    }

    /// Identifies the running machine.
    ///
    /// # Returns
    /// Its vendor, family and product name, plus the list of its disks.
    ///
    /// # Pre-conditions
    /// Requires DMI under `/sys/devices/virtual/dmi/id` and a readable
    /// `/sys/block`; a virtual machine without DMI makes this fail.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if any of the DMI attributes or the block
    /// device listing cannot be read.
    pub fn new() -> mx::Result<ComputerInfo> {
        let n = Self::grep_product_name();
        let v = Self::grep_vendor()?;
        let f = Self::grep_product_family(&v);
        Ok(ComputerInfo {
            product_family: f?,
            product_name: n?,
            vendor: v,
            disk: Self::list_block_device()?,
        })
    }

    /// The machine's vendor.
    ///
    /// # Returns
    /// The normalised vendor, borrowed from `self`.
    pub fn get_vendor(&self) -> &str {
        return &self.vendor;
    }

    /// The machine's product family.
    ///
    /// # Returns
    /// The normalised family, borrowed from `self`.
    pub fn get_product_family(&self) -> &str {
        return &self.product_family;
    }

    /// The machine's product name.
    ///
    /// # Returns
    /// The raw DMI value, borrowed from `self`.
    pub fn get_product_name(&self) -> &str {
        return &self.product_name;
    }

    /// Whether the machine has an industrial-I/O device, i.e. the sensors
    /// (accelerometer, ambient light) a convertible exposes.
    ///
    /// # Returns
    /// `true` when `/sys/bus/iio/devices` holds at least one entry; `false` when
    /// it is empty, absent or unreadable - an unreadable `sysfs` is reported as
    /// "no device", not as an error.
    pub fn has_iio_device() -> bool {
        let path = Path::new("/sys/bus/iio/devices");
        if path.exists()
            && path.is_dir()
            && match path.read_dir() {
                Ok(read_dir) => read_dir,
                Err(_) => return false,
            }
            .next()
            .is_some()
        {
            return true;
        }
        return false;
    }

    /// Whether the machine has a fingerprint reader, read from the live
    /// `sysfs`.
    ///
    /// # Returns
    /// As in [`ComputerInfo::has_fingerprint_device_in`], over `/sys`.
    pub fn has_fingerprint_device() -> bool {
        Self::has_fingerprint_device_in(Path::new(SYSFS_ROOT))
    }

    /// Whether a USB fingerprint reader is enumerated under a `sysfs` tree.
    ///
    /// Reads `sysfs` rather than running `lsusb`, so the verdict does not
    /// depend on `usbutils` being on `PATH`: a missing `lsusb` used to read as
    /// "no reader" on every caller that was not wrapped with it. The device
    /// descriptor strings `sysfs` exposes are the ones `lsusb` prints.
    ///
    /// # Parameters
    /// * `sysfs` - the `sysfs` mount point, so a test can hand this a fixture
    ///   tree instead of the running kernel.
    ///
    /// # Returns
    /// `true` when an entry of `{sysfs}/bus/usb/devices` either names a
    /// fingerprint reader in its `product`/`manufacturer` string, or carries an
    /// `idVendor` from [`ComputerInfo::FINGERPRINT_USB_VENDORS`]. `false`
    /// otherwise, and also when that directory is absent or unreadable - an
    /// unreadable `sysfs` is reported as "no device", not as an error. Only USB
    /// readers are seen; a reader on another bus (SPI, I2C) is missed.
    pub(crate) fn has_fingerprint_device_in(sysfs: &Path) -> bool {
        let devices = match sysfs.join("bus/usb/devices").read_dir() {
            Ok(read_dir) => read_dir,
            Err(_) => return false,
        };

        for entry in devices.flatten() {
            let path = entry.path();

            if ["product", "manufacturer"].iter().any(|attr| {
                Self::read_sysfs_attr(&path.join(attr))
                    .is_some_and(|value| value.to_lowercase().contains("fingerprint"))
            }) {
                return true;
            }

            if Self::read_sysfs_attr(&path.join("idVendor")).is_some_and(|vendor| {
                Self::FINGERPRINT_USB_VENDORS.contains(&vendor.to_lowercase().as_str())
            }) {
                return true;
            }
        }

        false
    }

    /// Reads one `sysfs` attribute file.
    ///
    /// # Parameters
    /// * `path` - the attribute file.
    ///
    /// # Returns
    /// Its contents, trimmed of the trailing newline `sysfs` emits; `None` when
    /// the file is absent, unreadable or empty once trimmed.
    fn read_sysfs_attr(path: &Path) -> Option<String> {
        let value = fs::read_to_string(path).ok()?.trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    }

    /// Whether the machine is a laptop.
    ///
    /// # Returns
    /// `true` when `/sys/class/power_supply` holds at least one entry, taking a
    /// battery as the laptop signal; `false` otherwise. A desktop with a UPS the
    /// kernel exposes there reads as a laptop.
    pub fn is_laptop() -> bool {
        let path = Path::new("/sys/class/power_supply");
        if path.exists()
            && path.is_dir()
            && match path.read_dir() {
                Ok(read_dir) => read_dir,
                Err(_) => return false,
            }
            .next()
            .is_some()
        {
            return true;
        }
        return false;
    }

    /// Lists the machine's disks.
    ///
    /// # Returns
    /// The `/sys/block` entries whose name starts with `sd` or `nvme`, in
    /// directory order; other kinds of block device (`mmcblk`, loop, device
    /// mapper) are ignored.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if `/sys/block` cannot be listed.
    fn list_block_device() -> mx::Result<Vec<String>> {
        let mut devices = Vec::new();
        for entry in fs::read_dir("/sys/block").map_err(|e| io_error_at("/sys/block", e))? {
            let entry = entry.map_err(|e| io_error_at("/sys/block", e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("sd") || name.starts_with("nvme") {
                devices.push(name);
            }
        }
        Ok(devices)
    }

    /// Whether a block device is a spinning disk.
    ///
    /// # Parameters
    /// * `device` - device name as listed in `/sys/block` (e.g. `sda`).
    ///
    /// # Returns
    /// `true` when the kernel reports the device as rotational.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the device's `queue/rotational` attribute
    /// cannot be read.
    #[allow(dead_code)]
    fn is_hdd(device: &str) -> mx::Result<bool> {
        let path = format!("/sys/block/{}/queue/rotational", device);
        let contents = fs::read_to_string(&path).map_err(|e| io_error_at(&path, e))?;
        Ok(contents.trim() == "1")
    }

    /// Whether a block device is solid-state.
    ///
    /// # Parameters
    /// * `device` - device name as listed in `/sys/block`.
    ///
    /// # Returns
    /// `true` when the kernel reports the device as non-rotational.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the device's `queue/rotational` attribute
    /// cannot be read.
    fn is_ssd(device: &str) -> mx::Result<bool> {
        let path = format!("/sys/block/{}/queue/rotational", device);
        let contents = fs::read_to_string(&path).map_err(|e| io_error_at(&path, e))?;
        Ok(contents.trim() == "0")
    }

    /// Whether the machine has at least one spinning disk.
    ///
    /// # Returns
    /// `true` at the first rotational disk found. `false` when none is, and also
    /// as soon as one device cannot be probed - an unreadable device stops the
    /// scan and reads as "no HDD", even if a later one is a spinning disk.
    #[allow(dead_code)]
    pub fn has_hdd(&self) -> bool {
        for device in &self.disk {
            match Self::is_hdd(&device) {
                Ok(true) => return true,
                Ok(false) => continue,
                Err(_) => return false,
            }
        }
        return false;
    }

    /// Whether the machine has at least one solid-state disk, which is what
    /// decides the `common-pc-ssd` module.
    ///
    /// # Returns
    /// `true` at the first non-rotational disk found; `false` when none is or a
    /// device cannot be probed, with the same early stop as
    /// [`ComputerInfo::has_hdd`].
    pub fn has_ssd(&self) -> bool {
        for device in &self.disk {
            match Self::is_ssd(&device) {
                Ok(true) => return true,
                Ok(false) => continue,
                Err(_) => return false,
            }
        }
        return false;
    }
}

#[cfg(test)]
#[path = "computer_info_tests.rs"]
mod tests;
