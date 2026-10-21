//! Tests for [`super`].
//!
//! [`super::ComputerInfo::has_fingerprint_device_in`] reads `sysfs` text files,
//! so the seam is a fixture tree rather than a mocked `lsusb`: [`UsbSysfs`]
//! builds the `bus/usb/devices` entries and the descriptor attributes a real
//! `/sys` exposes.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

/// A fixture `sysfs` tree holding USB devices.
struct UsbSysfs {
    dir: TempDir,
}

impl UsbSysfs {
    /// Creates an empty tree with the `bus/usb/devices` directory present and
    /// holding no device.
    fn new() -> Self {
        let sysfs = Self {
            dir: TempDir::new().expect("tempdir"),
        };
        fs::create_dir_all(sysfs.root().join("bus/usb/devices")).expect("mkdir");
        sysfs
    }

    /// Creates a tree with no `bus/usb/devices` directory at all, as a kernel
    /// without USB support - or a container - presents.
    fn without_usb_bus() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    /// The tree's root, to be passed where `/sys` would be.
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Adds one USB device.
    ///
    /// # Parameters
    /// * `name` - the device directory's name, as `sysfs` spells it.
    /// * `vendor` - `idVendor`, written whatever its value.
    /// * `attrs` - the descriptor strings, each written only when it is
    ///   `Some`, so a device whose kernel exposed none can be described.
    fn device(&self, name: &str, vendor: &str, attrs: &[(&str, Option<&str>)]) -> &Self {
        let path = self.root().join("bus/usb/devices").join(name);
        fs::create_dir_all(&path).expect("mkdir");
        fs::write(path.join("idVendor"), format!("{vendor}\n")).expect("write");
        for (attr, value) in attrs {
            if let Some(value) = value {
                fs::write(path.join(attr), format!("{value}\n")).expect("write");
            }
        }
        self
    }

    /// Adds the USB hub every machine enumerates, as a device that must not be
    /// taken for a reader.
    fn hub(&self, name: &str) -> &Self {
        self.device(
            name,
            "05e3",
            &[
                ("product", Some("USB2.0 Hub")),
                ("manufacturer", Some("GenesysLogic")),
            ],
        )
    }
}

#[test]
fn product_string_naming_a_reader_is_detected() {
    let sysfs = UsbSysfs::new();
    sysfs.hub("1-0:1.0").device(
        "1-10",
        "27c6",
        &[
            ("product", Some("Goodix Fingerprint USB Device")),
            ("manufacturer", Some("Goodix Technology Co., Ltd.")),
        ],
    );

    assert!(ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn product_string_match_is_case_insensitive() {
    let sysfs = UsbSysfs::new();
    sysfs.device(
        "1-3",
        "138a",
        &[("product", Some("Validity FINGERPRINT Sensor"))],
    );

    assert!(ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn known_vendor_id_is_detected_without_any_product_string() {
    let sysfs = UsbSysfs::new();
    sysfs.device("1-10", "27c6", &[]);

    assert!(ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn manufacturer_string_alone_is_enough() {
    let sysfs = UsbSysfs::new();
    sysfs.device(
        "1-4",
        "04f3",
        &[("manufacturer", Some("Elan Fingerprint Sensor"))],
    );

    assert!(ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn machine_without_a_reader_is_reported_as_none() {
    let sysfs = UsbSysfs::new();
    sysfs.hub("1-0:1.0").device(
        "1-5",
        "046d",
        &[("product", Some("PRO X Wireless Gaming Headset"))],
    );

    assert!(!ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn ambiguous_vendor_without_the_product_string_is_not_a_reader() {
    let sysfs = UsbSysfs::new();
    sysfs
        .device("1-6", "06cb", &[("product", Some("Synaptics TouchPad"))])
        .device("1-7", "04f3", &[("product", Some("ELAN USB Keyboard"))]);

    assert!(!ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn an_empty_usb_bus_is_reported_as_none() {
    let sysfs = UsbSysfs::new();

    assert!(!ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn a_missing_usb_bus_is_reported_as_none() {
    let sysfs = UsbSysfs::without_usb_bus();

    assert!(!ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}

#[test]
fn a_device_exposing_no_attribute_is_reported_as_none() {
    let sysfs = UsbSysfs::new();
    fs::create_dir_all(sysfs.root().join("bus/usb/devices/1-8")).expect("mkdir");

    assert!(!ComputerInfo::has_fingerprint_device_in(sysfs.root()));
}
