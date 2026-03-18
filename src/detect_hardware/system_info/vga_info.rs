use regex::Regex;
use std::process::Command;

use crate::mx;

type VgaDevices = Vec<(String, String)>;

#[derive(Debug)]
pub struct VgaInfo {
    vga_device: VgaDevices,
}

impl VgaInfo {
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

    pub fn new() -> mx::Result<VgaInfo> {
        Ok(VgaInfo {
            vga_device: Self::get_vga_devices()?,
        })
    }

    pub fn has_nvidia_device(&self) -> bool {
        for (_, description) in &self.vga_device {
            if description.to_lowercase().contains("nvidia") {
                return true;
            }
        }
        return false;
    }

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

    /// Get generation codename for most recent card
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
                }
                // Prefere most recent card
                else if Self::NVIDIA_GEN_CHIPSET
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

    pub fn match_archtecture_codename(&self, codename: &str) -> bool {
        for device in &self.vga_device {
            if device.1.to_lowercase().contains(&codename.to_lowercase()) {
                return true;
            }
        }
        return false;
    }
}
