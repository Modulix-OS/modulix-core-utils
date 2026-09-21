//! Machine identity and peripherals, read from DMI and `sysfs`: what the
//! `nixos-hardware` machine modules are matched against.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::mx;

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
    /// Vendor strings rewritten to the spelling `nixos-hardware` uses.
    const HARDWARE_VENDOR_REPLACMENT: [(&'static str, &'static str); 2] =
        [("Hewlett-Packard", "hp"), ("Hewlett Packard", "hp")];

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
            .map_err(mx::ErrorKind::IOError)?;
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
            .map_err(mx::ErrorKind::IOError)?
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
            .map_err(mx::ErrorKind::IOError)
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

    /// Whether the machine has a fingerprint reader.
    ///
    /// # Returns
    /// `true` when a `lsusb` line mentions `fingerprint`, case-insensitively.
    /// `false` when none does, and also when `lsusb` cannot be run - so a
    /// missing `usbutils` silently reads as "no reader". Only USB readers are
    /// seen; a reader on another bus is missed.
    pub fn has_fingerprint_device() -> bool {
        let output = match Command::new("lsusb").output() {
            Ok(out) => out,
            Err(_) => return false,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().split('\n').collect();

        for line in lines {
            if line.to_lowercase().contains("fingerprint") {
                return true;
            }
        }
        return false;
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
        for entry in fs::read_dir("/sys/block").map_err(mx::ErrorKind::IOError)? {
            let entry = entry.map_err(mx::ErrorKind::IOError)?;
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
        let contents = fs::read_to_string(path).map_err(mx::ErrorKind::IOError)?;
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
        let contents = fs::read_to_string(path).map_err(mx::ErrorKind::IOError)?;
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
