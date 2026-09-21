//! Matching the detected hardware against the `nixos-hardware` catalogue: what
//! the generated `hardware-configuration.nix` ends up importing.
//!
//! Module names are dash-separated and sorted (`framework-13inch-7040-amd`,
//! `common-gpu-nvidia-turing`), so the search walks the sorted list segment by
//! segment: vendor, then family, then whatever narrows it further. When no
//! machine-specific module matches, the `common-*` ones are picked from the
//! detected CPU, GPU and disks instead.

use serde::{Deserialize, Serialize};

use crate::mx;

use super::hardware_driver::HardwareModule;
use super::system_info::computer_info::ComputerInfo;
use super::system_info::cpu_info::CpuInfo;
use super::system_info::vga_info::VgaInfo;

/// The hardware-driven part of a configuration: which modules to import and
/// which optional services the machine calls for.
///
/// # Fields
/// * `impoted_module` - `nixos-hardware` module names to import.
/// * `fingerprint` - whether a fingerprint reader was detected.
/// * `iio_sensor` - whether industrial-I/O sensors were detected.
/// * `cpu_info` - the detected CPU, kept for serialisation and inspection.
#[derive(Serialize, Deserialize, Debug)]
pub struct DriverConfig {
    impoted_module: Vec<String>,
    fingerprint: bool,
    iio_sensor: bool,
    cpu_info: CpuInfo,
}

impl DriverConfig {
    /// Narrows a range of candidate module names one dash-separated segment at a
    /// time, recursing until a single name is left.
    ///
    /// # Parameters
    /// * `hardware_module` - candidates still in the running, sorted, all
    ///   sharing the first `depth` segments.
    /// * `computer_info` - the detected machine, matched against the segments.
    /// * `vga_info` - the detected GPUs, used for the `nvidia`/`amdgpu` segments.
    /// * `depth` - index of the segment to discriminate on.
    ///
    /// # Returns
    /// The chosen module name, borrowed from `hardware_module`; `None` when the
    /// range is empty or no segment matches. A single candidate is accepted as-is
    /// without checking the remaining segments. A candidate whose path has no
    /// further segment past `depth` has no next segment to discriminate on, so
    /// it is treated as a leaf/default candidate for its range.
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

    /// Restricts a vendor's modules to the ones whose family segment matches the
    /// machine, then narrows further.
    ///
    /// # Parameters
    /// * `hardware_module` - the vendor's modules, sorted.
    /// * `computer_info` - the detected machine, whose product family must
    ///   contain the family segment for it to match.
    /// * `vga_info` - the detected GPUs, forwarded to the recursion.
    ///
    /// # Returns
    /// The chosen module name, or `None` when no module's family matches.
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

    /// Looks for the machine-specific `nixos-hardware` module of this machine.
    ///
    /// # Parameters
    /// * `hardware_module` - the catalogue, whose machine modules are searched.
    /// * `computer_info` - the detected machine; its vendor must contain a
    ///   module's vendor segment for that vendor's range to be considered.
    /// * `vga_info` - the detected GPUs, forwarded to the narrowing.
    ///
    /// # Returns
    /// The chosen module name, borrowed from the catalogue, or `None` when the
    /// machine has no module - the caller then falls back to the `common-*` ones.
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

    /// Restricts a sorted range to the entries sharing a given prefix.
    ///
    /// # Parameters
    /// * `range` - sorted module names.
    /// * `prefix` - prefix the sub-range must share (e.g. `common-gpu-nvidia`).
    ///
    /// # Returns
    /// The matching contiguous sub-range. A prefix that matches nothing yields a
    /// range starting at index 0, since the search falls back to the start rather
    /// than to an empty slice.
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

    /// Picks the `common-*` modules a machine without a dedicated module needs.
    ///
    /// # Parameters
    /// * `vga_info` - the detected GPUs, which decide the `common-gpu-*` modules.
    /// * `computer_info` - the detected machine, which decides the `common-pc-*`
    ///   ones.
    ///
    /// # Returns
    /// The module names, possibly several: one per GPU vendor present, plus
    /// `common-gpu-nvidia-prime` on a hybrid laptop, `common-pc-laptop` or
    /// `common-pc`, and `common-pc-ssd` when the machine has a solid-state disk.
    /// The names are built by hand rather than looked up, so one that
    /// `nixos-hardware` does not (or no longer) provide surfaces later, as an
    /// evaluation failure of the rebuilt configuration. GPU vendors other than
    /// NVIDIA are matched on the description text, so an NVIDIA card in an Intel
    /// machine can pull `common-gpu-intel` in through the integrated GPU.
    fn get_common_hardware_module(vga_info: &VgaInfo, computer_info: &ComputerInfo) -> Vec<String> {
        let mut all_module: Vec<String> = vec![];

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

        if vga_info.match_archtecture_codename("intel") {
            all_module.push(String::from("common-gpu-intel"));
        }

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

    /// Probes the machine and derives the hardware configuration it needs.
    ///
    /// # Returns
    /// The modules to import plus the fingerprint/sensor flags. The machine's
    /// own `nixos-hardware` module is preferred and used alone when it exists;
    /// otherwise the `common-*` set is used.
    ///
    /// # Pre-conditions
    /// Needs `cpuid`, `lspci` and `lsusb` on `PATH`, DMI under `sysfs`, and
    /// network access for `nix` to evaluate `nixos-hardware` from GitHub.
    ///
    /// # Errors
    /// Any error from the probes: notably
    /// [`mx::ErrorKind::GetVGAInfoError`] (GPUs),
    /// [`mx::ErrorKind::CPUInfoNofFound`] or
    /// [`mx::ErrorKind::UnknowCPUConstructor`] (CPU),
    /// [`mx::ErrorKind::IOError`] (DMI, disks) and
    /// [`mx::ErrorKind::RequestSenderError`] (module catalogue).
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

    /// The `nixos-hardware` modules to import.
    ///
    /// # Returns
    /// The module names, borrowed from `self`; they still have to be spelled
    /// `nixos-hardware.nixosModules.<name>` in the configuration, which
    /// [`crate::hardware_config`] does.
    pub fn get_module(&self) -> &Vec<String> {
        &self.impoted_module
    }

    /// Whether a fingerprint reader was detected.
    ///
    /// # Returns
    /// `true` when the machine has one, so the configuration can enable the
    /// matching service.
    pub fn get_fingerprint(&self) -> bool {
        self.fingerprint
    }

    /// Whether industrial-I/O sensors were detected.
    ///
    /// # Returns
    /// `true` when the machine has them, i.e. a convertible whose screen
    /// rotation and light sensor need a service.
    pub fn get_iio_sensor(&self) -> bool {
        self.iio_sensor
    }
}
