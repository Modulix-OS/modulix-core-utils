use serde::{Deserialize, Serialize};

use crate::mx;

use super::hardware_driver::HardwareModule;
use super::system_info::computer_info::ComputerInfo;
use super::system_info::cpu_info::CpuInfo;
use super::system_info::vga_info::VgaInfo;

#[derive(Serialize, Deserialize, Debug)]
pub struct DriverConfig {
    impoted_module: Vec<String>,
    fingerprint: bool,
    iio_sensor: bool,
    cpu_info: CpuInfo,
}

impl DriverConfig {
    fn get_computer_hardware_module_rec<'a>(
        hardware_module: &'a [String],
        computer_info: &ComputerInfo,
        vga_info: &VgaInfo,
        depth: usize,
    ) -> Option<&'a str> {
        if hardware_module.len() == 1 {
            return Some(&hardware_module[0]);
        } else if hardware_module.is_empty() {
            return None;
        }

        let mut match_module: Option<&str> = None;
        let mut begin: Option<usize> = None;
        let mut end: Option<usize> = None;
        let mut common_b: Option<usize> = None;
        let mut common_e: Option<usize> = None;
        let mut def: Option<usize> = None;
        let mut nvidia: Option<usize> = None;
        let mut amdgpu: Option<usize> = None;

        for i in 0..hardware_module.len() {
            let parts: Vec<&str> = hardware_module[i].split('-').collect();
            let Some(&segment) = parts.get(depth) else {
                continue;
            };

            match segment {
                "common" => {
                    common_b.get_or_insert(i);
                    common_e = Some(i + 1);
                    if begin.is_some() {
                        end = Some(i);
                    }
                }
                "nvidia" => {
                    nvidia = Some(i);
                    if begin.is_some() {
                        end = Some(i);
                    }
                }
                "amdgpu" => {
                    amdgpu = Some(i);
                    if begin.is_some() {
                        end = Some(i);
                    }
                }
                _ if parts.len() == depth + 1 => {
                    // Pas de segment suivant : c'est un module "feuille" (défaut)
                    def = Some(i);
                    if begin.is_some() {
                        end = Some(i);
                    }
                }
                _ => match match_module {
                    None if segment.split('-').all(|s| {
                        computer_info.get_product_name().contains(s)
                            || computer_info.get_product_family().contains(s)
                    }) =>
                    {
                        match_module = Some(segment);
                        begin = Some(i);
                    }
                    Some(m) if m != segment => {
                        end = Some(i);
                        break;
                    }
                    _ => continue,
                },
            }
        }

        if begin.is_none() {
            if let Some(c) = common_b {
                return Self::get_computer_hardware_module_rec(
                    &hardware_module[c..common_e.unwrap()],
                    computer_info,
                    vga_info,
                    depth + 1,
                );
            }
            if let Some(n) = nvidia {
                if vga_info.has_nvidia_device() {
                    return Some(&hardware_module[n]);
                }
            }
            if let Some(a) = amdgpu {
                if vga_info.match_archtecture_codename("amd") {
                    return Some(&hardware_module[a]);
                }
            }
            return def.map(|d| hardware_module[d].as_str());
        }

        let range = &hardware_module[begin.unwrap()..end.unwrap_or(hardware_module.len())];
        Self::get_computer_hardware_module_rec(range, computer_info, vga_info, depth + 1)
    }

    fn get_computer_hardware_module_family<'a>(
        hardware_module: &'a [String],
        computer_info: &ComputerInfo,
        vga_info: &VgaInfo,
    ) -> Option<&'a str> {
        let mut match_family: Option<&str> = None;
        let mut begin: Option<usize> = None;
        let mut end: Option<usize> = None;

        for i in 0..hardware_module.len() {
            let parts: Vec<&str> = hardware_module[i].split('-').collect();
            let Some(&family) = parts.get(1) else {
                continue;
            };

            match match_family {
                None if computer_info.get_product_family().contains(family) => {
                    match_family = Some(family);
                    begin = Some(i);
                }
                Some(m) if m != family => {
                    end = Some(i);
                    break;
                }
                _ => continue,
            }
        }

        begin?;

        let range = &hardware_module[begin.unwrap()..end.unwrap_or(hardware_module.len())];
        Self::get_computer_hardware_module_rec(range, computer_info, vga_info, 2)
    }

    fn get_computer_hardware_module<'a>(
        hardware_module: &'a HardwareModule,
        computer_info: &ComputerInfo,
        vga_info: &VgaInfo,
    ) -> Option<&'a str> {
        let modules = hardware_module.get_computer_module();

        let vendor = modules
            .iter()
            .map(|s| s.split('-').next().unwrap_or(""))
            .find(|&v| computer_info.get_vendor().contains(v))?;

        let begin = modules.iter().position(|s| s.starts_with(vendor))?;
        let end = modules[begin..]
            .iter()
            .position(|s| !s.starts_with(vendor))
            .map(|p| p + begin)
            .unwrap_or(modules.len());

        Self::get_computer_hardware_module_family(&modules[begin..end], computer_info, vga_info)
    }

    #[cfg(feature = "match-exact-gpu-gen")]
    fn restrict_range<'a>(range: &'a [String], prefix: &str) -> &'a [String] {
        let b = range
            .iter()
            .position(|s| s.starts_with(prefix))
            .unwrap_or(0);
        let e = range[b..]
            .iter()
            .position(|s| !s.starts_with(prefix))
            .map(|p| p + b)
            .unwrap_or(range.len());
        &range[b..e]
    }

    fn get_common_hardware_module(vga_info: &VgaInfo, computer_info: &ComputerInfo) -> Vec<String> {
        let mut all_module: Vec<String> = vec![];

        // GPU Nvidia — ex: `common-gpu-nvidia-turing`
        if vga_info.has_nvidia_device() {
            #[cfg(feature = "match-exact-gpu-gen")]
            {
                let nvidia_modules = Self::restrict_range(common, "common-gpu-nvidia");
                match vga_info.get_nvidia_generation() {
                    Ok(arch) => all_module.push(
                        nvidia_modules
                            .iter()
                            .find(|s| s.split('-').nth(3).map_or(false, |seg| seg == arch))
                            .cloned()
                            .unwrap_or_else(|| String::from("common-gpu-nvidia")),
                    ),
                    Err(_) => all_module.push(String::from("common-gpu-nvidia")),
                }
            }
            #[cfg(not(feature = "match-exact-gpu-gen"))]
            {
                all_module.push(String::from("common-gpu-nvidia"));
            }
            if vga_info.has_nvidia_laptop() {
                all_module.push(String::from("common-gpu-nvidia-prime"));
            }
        }

        if vga_info.match_archtecture_codename("amd") {
            #[cfg(feature = "match-exact-gpu-gen")]
            {
                let amd_modules = Self::restrict_range(common, "common-gpu-amd");
                if let Some(s) = amd_modules.iter().find(|s| {
                    s.split('-')
                        .skip(3)
                        .all(|p| vga_info.match_archtecture_codename(p))
                }) {
                    all_module.push(s.clone());
                }
            }
            #[cfg(not(feature = "match-exact-gpu-gen"))]
            {
                all_module.push(String::from("common-gpu-amd"));
            }
        }

        // GPU Intel
        if vga_info.match_archtecture_codename("intel") {
            all_module.push(String::from("common-gpu-intel"));
        }

        // PC
        if ComputerInfo::is_laptop() {
            all_module.push(String::from("common-pc-laptop"));
        } else {
            all_module.push(String::from("common-pc"));
        }

        if computer_info.has_ssd() {
            all_module.push(String::from("common-pc-ssd"));
        }

        all_module
    }

    pub fn new() -> mx::Result<DriverConfig> {
        let vga_info = VgaInfo::new()?;
        let hardware_module = HardwareModule::new()?;
        let computer_info = ComputerInfo::new()?;
        let cpu_info = CpuInfo::new()?;

        Ok(DriverConfig {
            impoted_module: match Self::get_computer_hardware_module(
                &hardware_module,
                &computer_info,
                &vga_info,
            ) {
                Some(s) => vec![s.to_string()],
                None => Self::get_common_hardware_module(&vga_info, &computer_info),
            },
            fingerprint: ComputerInfo::has_fingerprint_device(),
            iio_sensor: ComputerInfo::has_iio_device(),
            cpu_info: cpu_info,
        })
    }

    pub fn get_module(&self) -> &Vec<String> {
        &self.impoted_module
    }

    pub fn get_fingerprint(&self) -> bool {
        self.fingerprint
    }

    pub fn get_iio_sensor(&self) -> bool {
        self.iio_sensor
    }
}
