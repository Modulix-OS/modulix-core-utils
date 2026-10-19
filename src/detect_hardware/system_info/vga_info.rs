//! Which GPU vendors `lspci` reports, and whether the NVIDIA card is a mobile
//! one: the two questions [`super::super::driver_config`] asks to pick the
//! `nixos-hardware` modules the configuration imports.
//!
//! Deliberately nothing more. The per-GPU facts the `mx.hardware.gpu` options
//! need - vendor, PCI id, PCI address, DRM index, generation - come from
//! [`super::gpu_info`], which reads `sysfs` and therefore does not depend on
//! the `pci.ids` database a description is spelled from.

use regex::Regex;
use std::process::Command;

use crate::mx;

/// The graphics devices `lspci` reported, as the descriptions it printed.
type VgaDevices = Vec<String>;

/// The machine's graphics devices.
///
/// # Fields
/// * `vga_device` - one description per VGA or 3D controller, in `lspci`
///   order.
#[derive(Debug)]
pub struct VgaInfo {
    vga_device: VgaDevices,
}

impl VgaInfo {
    /// Lists the machine's graphics devices through `lspci`.
    ///
    /// # Returns
    /// One description per line describing a `VGA compatible controller` or a
    /// `3D controller` - the latter being how a laptop's discrete GPU usually
    /// shows up, having no display output of its own.
    ///
    /// # Pre-conditions
    /// `lspci` must be on `PATH`.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GetVGAInfoError`] if `lspci` cannot be run or exits
    /// non-zero. A non-zero exit is reported rather than read as "no GPU",
    /// which would silently cost the machine its GPU modules.
    fn get_vga_devices() -> mx::Result<VgaDevices> {
        let output = Command::new("lspci")
            .output()
            .map_err(|_| mx::ErrorKind::GetVGAInfoError("Failed to execute lspci command"))?;

        if !output.status.success() {
            return Err(mx::ErrorKind::GetVGAInfoError("lspci exited with an error"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let keywords: [&str; 2] = [" VGA compatible controller: ", " 3D controller: "];

        Ok(stdout
            .lines()
            .filter_map(|line| {
                keywords
                    .iter()
                    .find_map(|keyword| line.find(keyword))
                    .map(|index| line[index..].trim().to_string())
            })
            .collect())
    }

    /// Detects the machine's graphics devices.
    ///
    /// # Returns
    /// The device list, which can legitimately be empty on a headless machine.
    ///
    /// # Errors
    /// As in the `lspci` invocation above.
    pub fn new() -> mx::Result<VgaInfo> {
        Ok(VgaInfo {
            vga_device: Self::get_vga_devices()?,
        })
    }

    /// Whether the machine has an NVIDIA GPU.
    ///
    /// # Returns
    /// `true` when a device description mentions `nvidia`, case-insensitively.
    pub fn has_nvidia_device(&self) -> bool {
        self.vga_device
            .iter()
            .any(|description| description.to_lowercase().contains("nvidia"))
    }

    /// Whether the NVIDIA GPU is a mobile one, which is what decides the
    /// hybrid-graphics (Optimus) modules.
    ///
    /// # Returns
    /// `true` when an NVIDIA description says `laptop` or `mobile`, or carries
    /// a three-digit model number suffixed with `M` (the older mobile
    /// naming). `false` otherwise, including when the detection regex fails to
    /// build.
    pub fn has_nvidia_laptop(&self) -> bool {
        const KEYWORD: [&str; 2] = ["laptop", "mobile"];
        let Ok(pattern) = Regex::new(r"\b\d{3}M\b") else {
            return false;
        };
        self.vga_device.iter().any(|description| {
            let lowercase = description.to_lowercase();
            lowercase.contains("nvidia")
                && (KEYWORD.iter().any(|keyword| lowercase.contains(keyword))
                    || pattern.is_match(description))
        })
    }

    /// Whether any graphics device's description mentions a given codename.
    ///
    /// # Parameters
    /// * `codename` - the text to look for; the comparison is
    ///   case-insensitive and on substrings, so a short codename can match by
    ///   accident.
    ///
    /// # Returns
    /// `true` at the first device whose description contains it.
    pub fn match_archtecture_codename(&self, codename: &str) -> bool {
        let needle = codename.to_lowercase();
        self.vga_device
            .iter()
            .any(|description| description.to_lowercase().contains(&needle))
    }
}
