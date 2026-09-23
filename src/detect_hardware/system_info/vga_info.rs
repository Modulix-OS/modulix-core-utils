//! GPU detection from `lspci`: which vendors are present, whether the NVIDIA
//! card is a mobile one, and which generation each vendor's most recent card
//! belongs to.

use regex::Regex;
use std::process::Command;

use crate::mx;

/// The graphics devices `lspci` reported, as `(PCI address, description)` pairs.
type VgaDevices = Vec<(String, String)>;

/// The machine's graphics devices.
///
/// # Fields
/// * `vga_device` - one entry per VGA or 3D controller, in `lspci` order.
#[derive(Debug)]
pub struct VgaInfo {
    vga_device: VgaDevices,
}

impl VgaInfo {
    /// NVIDIA chipset prefixes mapped to their architecture name, oldest first -
    /// the order is what makes "most recent card wins" work.
    const NVIDIA_GEN_CHIPSET: [(&'static str, &'static str); 8] = [
        ("GF", "fermi"),
        ("GK", "kepler"),
        ("GM", "maxwell"),
        ("GP", "pascal"),
        ("TU", "turing"),
        ("GA", "ampere"),
        ("AD", "ada-lovelace"),
        ("GB", "blackwell"),
    ];

    /// AMD chipset family names mapped to their architecture name, oldest
    /// first, same ordering convention as [`VgaInfo::NVIDIA_GEN_CHIPSET`].
    const AMD_GEN_CHIPSET: [(&'static str, &'static str); 10] = [
        ("Southern Islands", "gcn-1-gen"),
        ("Sea Islands", "gcn-2-gen"),
        ("Volcanic Islands", "gcn-3-gen"),
        ("Arctic Island", "gcn-4-gen"),
        ("Polaris", "gcn-4-gen"),
        ("Vega", "gcn-5-gen"),
        ("Navi 1", "rdna"),
        ("Navi 2", "rdna2"),
        ("Navi 3", "rdna3"),
        ("Navi 4", "rdna4"),
    ];

    /// Rewrites an `lspci` address into the `PCI:bus:device:function` form the
    /// X and NVIDIA configurations expect.
    ///
    /// # Parameters
    /// * `address` - the address as `lspci` prints it (`00:02.0`), possibly with
    ///   a domain prefix, which is dropped.
    ///
    /// # Returns
    /// `PCI:<bus>:<device>:<function>`, with bus and device converted from
    /// hexadecimal to decimal.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GetVGAInfoError`] when the address has fewer than three
    /// components.
    ///
    /// # Panics
    /// If a component is not the number base expected - bus and device
    /// hexadecimal, function decimal.
    fn convert_to_pci_format(address: &str) -> mx::Result<String> {
        let re = Regex::new(r"[:\\.]").map_err(|_| {
            mx::ErrorKind::GetVGAInfoError("An error has occurred while convert to pci format")
        })?;

        let device_id: Vec<&str> = re.split(address).collect();
        if device_id.len() < 3 {
            return Err(mx::ErrorKind::GetVGAInfoError("Invalide device id"));
        }
        let bus: u32 = u32::from_str_radix(device_id[device_id.len() - 3], 16).unwrap();
        let device: u32 = u32::from_str_radix(device_id[device_id.len() - 2], 16).unwrap();
        let function: u32 = u32::from_str_radix(device_id[device_id.len() - 1], 10).unwrap();
        Ok(format!("PCI:{}:{}:{}", bus, device, function))
    }

    /// Lists the machine's graphics devices through `lspci`.
    ///
    /// # Returns
    /// One `(PCI address, description)` pair per line describing a
    /// `VGA compatible controller` or a `3D controller` - the latter being how a
    /// laptop's discrete GPU usually shows up.
    ///
    /// # Pre-conditions
    /// `lspci` must be on `PATH`.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GetVGAInfoError`] if `lspci` cannot be run or one of its
    /// addresses does not parse. Its exit status is not checked: a failed run
    /// yields an empty list.
    fn get_vga_devices() -> mx::Result<VgaDevices> {
        let output = Command::new("lspci")
            .output()
            .map_err(|_| mx::ErrorKind::GetVGAInfoError("Failed to execute lspci command"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().split('\n').collect();

        let keywords: [&str; 2] = [" VGA compatible controller: ", " 3D controller: "];

        let mut vga_devices: VgaDevices = Vec::new();

        for line in lines {
            for keyword in &keywords {
                if let Some(index) = line.find(keyword) {
                    let (address, description) = line.split_at(index);
                    let pci_address = Self::convert_to_pci_format(address.trim())?;
                    if !pci_address.is_empty() {
                        vga_devices.push((pci_address, description.trim().to_string()));
                    }
                    break;
                }
            }
        }
        Ok(vga_devices)
    }

    /// Detects the machine's graphics devices.
    ///
    /// # Returns
    /// The device list, which can legitimately be empty on a headless machine.
    ///
    /// # Errors
    /// As in [`VgaInfo::get_vga_devices`].
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
        for (_, description) in &self.vga_device {
            if description.to_lowercase().contains("nvidia") {
                return true;
            }
        }
        return false;
    }

    /// Whether the NVIDIA GPU is a mobile one, which is what decides the
    /// hybrid-graphics (Optimus) modules.
    ///
    /// # Returns
    /// `true` when an NVIDIA description says `laptop` or `mobile`, or carries a
    /// three-digit model number suffixed with `M` (the older mobile naming).
    /// `false` otherwise, including when the detection regex fails to build - in
    /// which case the reason is printed on stdout.
    pub fn has_nvidia_laptop(&self) -> bool {
        for (_, description) in &self.vga_device {
            let desc_lower = description.to_lowercase();
            if desc_lower.contains("nvidia") {
                const KEYWORD: [&str; 2] = ["laptop", "mobile"];
                for keyw in KEYWORD {
                    if desc_lower.contains(keyw) {
                        return true;
                    }
                }
                let pattern = match Regex::new(r"\b\d{3}M\b") {
                    Ok(reg) => reg,
                    Err(err) => {
                        println!(
                            "An error has occurred while detecting an nvidia laptop : {}",
                            err
                        );
                        return false;
                    }
                };
                if pattern.is_match(description) {
                    return true;
                }
            }
        }
        return false;
    }

    /// Architecture of the most recent NVIDIA card in the machine.
    ///
    /// # Returns
    /// `Ok` with the architecture name (`turing`, `ampere`, …) taken from the
    /// chipset prefix of the newest card, per
    /// [`VgaInfo::NVIDIA_GEN_CHIPSET`]'s ordering. `Err` with a static message
    /// when no NVIDIA card is found, when its description carries no recognisable
    /// chipset code, or when the matching regex cannot be built.
    ///
    /// # Panics
    /// If `lspci` reports a chipset prefix made of two letters that the table
    /// does not list.
    pub fn get_nvidia_generation(&self) -> Result<&'static str, &'static str> {
        let list_codename = Self::NVIDIA_GEN_CHIPSET
            .map(|(code, _)| code.to_string())
            .join("|");
        let reg_chipset =
            match Regex::new(format!(r"\b[{}]{{2}}\d{{3}}[M]{{0,1}}\b", list_codename).as_str()) {
                Ok(reg) => reg,
                Err(_) => return Err("Error to create patern for chipset"),
            };
        let mut arch: &str = "";
        for (_, description) in &self.vga_device {
            if description.to_ascii_lowercase().contains("nvidia") {
                let match_chipset = match reg_chipset.find(description) {
                    Some(m) => m.as_str(),
                    None => continue,
                };
                if arch.is_empty() {
                    arch = &match_chipset[0..2];
                } else if Self::NVIDIA_GEN_CHIPSET
                    .iter()
                    .position(|(code, _)| code.eq(&arch))
                    .unwrap()
                    < Self::NVIDIA_GEN_CHIPSET
                        .iter()
                        .position(|(code, _)| code.eq(&&match_chipset[0..2]))
                        .unwrap()
                {
                    arch = &match_chipset[0..2];
                }
            }
        }
        if arch.is_empty() {
            Err("No nvidia card")
        } else {
            Ok(Self::NVIDIA_GEN_CHIPSET[Self::NVIDIA_GEN_CHIPSET
                .iter()
                .position(|(code, _)| code.eq(&arch))
                .unwrap()]
            .1)
        }
    }

    /// Architecture of the most recent AMD card in the machine.
    ///
    /// # Returns
    /// `Ok` with the architecture name (`rdna3`, `gcn-5-gen`, …) of the newest
    /// card, matched by family name against [`VgaInfo::AMD_GEN_CHIPSET`] and
    /// compared case-sensitively. `Err` with a static message when no AMD or
    /// Radeon device is found, or when none of their descriptions names a known
    /// family.
    pub fn get_amd_generation(&self) -> Result<&'static str, &'static str> {
        let mut best_gen: Option<usize> = None;

        for (_, description) in &self.vga_device {
            if !description.to_lowercase().contains("amd")
                && !description.to_lowercase().contains("radeon")
            {
                continue;
            }
            for (i, (codename, _)) in Self::AMD_GEN_CHIPSET.iter().enumerate() {
                if description.contains(codename) {
                    match best_gen {
                        None => best_gen = Some(i),
                        Some(current) if i > current => best_gen = Some(i),
                        _ => {}
                    }
                }
            }
        }

        match best_gen {
            Some(i) => Ok(Self::AMD_GEN_CHIPSET[i].1),
            None => Err("No AMD card"),
        }
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
        for device in &self.vga_device {
            if device.1.to_lowercase().contains(&codename.to_lowercase()) {
                return true;
            }
        }
        return false;
    }
}
