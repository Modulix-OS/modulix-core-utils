//! The machine's graphics topology, as the `mx.hardware.gpu` options describe
//! it: which GPU sits in the dedicated slot, which one in the integrated slot,
//! and the identifiers each slot needs.
//!
//! Everything but the generation codename comes from `sysfs`, which is why
//! this module exists next to [`super::vga_info`] rather than inside it:
//! `/sys/bus/pci/devices` reports a vendor id, a device id, a PCI address and
//! a `drm/cardN` node for every graphics controller, driver-bound or not.
//! `lspci` is consulted for one thing only - the human-readable chipset name a
//! generation codename is read off - so a machine without `pciutils` still
//! detects every other field.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::io_error_at;
use crate::mx;

/// `sysfs` mount point the probes read.
const SYSFS_ROOT: &str = "/sys";

/// File the CPU vendor is read from.
const CPUINFO_PATH: &str = "/proc/cpuinfo";

/// Chassis types DMI uses for a portable machine: Portable, Laptop, Notebook,
/// Hand Held, Sub Notebook, Tablet, Convertible, Detachable.
const PORTABLE_CHASSIS: [u32; 8] = [8, 9, 10, 11, 14, 30, 31, 32];

/// Chassis types that carry no information, so the battery fallback has to be
/// consulted anyway: Other, Unknown, Unspecified.
const UNINFORMATIVE_CHASSIS: [u32; 3] = [0, 1, 2];

/// Factor by which a GPU's video memory must exceed another's for the
/// difference to be read as "dedicated memory versus an APU carve-out". An
/// APU is routinely given 512 MB and can be given 2 GB, so a smaller ratio
/// would call a generously configured APU a discrete card.
const VRAM_DEDICATED_RATIO: u64 = 4;

/// NVIDIA chipset prefixes mapped to their architecture name.
const NVIDIA_GENERATIONS: [(&str, &str); 9] = [
    ("GF", "fermi"),
    ("GK", "kepler"),
    ("GM", "maxwell"),
    ("GP", "pascal"),
    ("GV", "volta"),
    ("TU", "turing"),
    ("GA", "ampere"),
    ("AD", "ada-lovelace"),
    ("GB", "blackwell"),
];

/// AMD family and APU names mapped to their architecture name. Matched against
/// a lowercased, whitespace-free description, so the keys carry neither case
/// nor spaces.
///
/// The APU names are there because an APU's description names the APU, never
/// the Navi die it descends from: a Phoenix iGPU is reported as `Phoenix1`,
/// not as `Navi 3x`.
///
/// Order matters, as [`table_generation`] takes the first match: a key that is
/// a prefix of another must come after it.
const AMD_GENERATIONS: [(&str, &str); 22] = [
    ("southernislands", "gcn-1-gen"),
    ("seaislands", "gcn-2-gen"),
    ("volcanicislands", "gcn-3-gen"),
    ("arcticisland", "gcn-4-gen"),
    ("polaris", "gcn-4-gen"),
    ("ravenridge", "gcn-5-gen"),
    ("picasso", "gcn-5-gen"),
    ("renoir", "gcn-5-gen"),
    ("cezanne", "gcn-5-gen"),
    ("lucienne", "gcn-5-gen"),
    ("barcelo", "gcn-5-gen"),
    ("mendocino", "gcn-5-gen"),
    ("vega", "gcn-5-gen"),
    ("navi1", "rdna"),
    ("navi2", "rdna2"),
    ("rembrandt", "rdna2"),
    ("vangogh", "rdna2"),
    ("navi3", "rdna3"),
    ("phoenix", "rdna3"),
    ("hawkpoint", "rdna3"),
    ("strix", "rdna3"),
    ("navi4", "rdna4"),
];

/// Intel family names mapped to their architecture name, same normalisation
/// and same ordering rule as [`AMD_GENERATIONS`].
///
/// Only the five legacy codenames matter to the configuration: mxpkgs matches
/// `ironlake`, `sandybridge`, `ivybridge`, `haswell` and `westmere` to pick
/// `intel-vaapi-driver`/`i965`, and treats every other value - `None`
/// included - as the modern `intel-media-driver`/`iHD`. The newer entries are
/// there for legibility of the generated file, not for behaviour, which is why
/// an unrecognised modern chip can safely stay `None`.
///
/// `coreprocessorintegratedgraphics` is the Arrandale/Ironlake description and
/// is a **substring** of the Haswell one (`4th Gen Core Processor Integrated
/// Graphics Controller`), so it must stay last among the legacy keys.
const INTEL_GENERATIONS: [(&str, &str); 23] = [
    ("sandybridge", "sandybridge"),
    ("2ndgenerationcore", "sandybridge"),
    ("ivybridge", "ivybridge"),
    ("3rdgencore", "ivybridge"),
    ("haswell", "haswell"),
    ("4thgencore", "haswell"),
    ("arrandale", "westmere"),
    ("westmere", "westmere"),
    ("ironlake", "ironlake"),
    ("broadwell", "broadwell"),
    ("skylake", "skylake"),
    ("kabylake", "kabylake"),
    ("coffeelake", "coffeelake"),
    ("cometlake", "cometlake"),
    ("icelake", "icelake"),
    ("tigerlake", "tigerlake"),
    ("alderlake", "alderlake"),
    ("raptorlake", "raptorlake"),
    ("meteorlake", "meteorlake"),
    ("arrowlake", "arrowlake"),
    ("lunarlake", "lunarlake"),
    ("dg2", "arc"),
    ("coreprocessorintegratedgraphics", "ironlake"),
];

/// A GPU vendor, in the spelling `mx.hardware.gpu.<slot>.vendor` accepts.
///
/// # Variants
/// * `Amd` - PCI vendor `0x1002`.
/// * `Nvidia` - PCI vendor `0x10de`.
/// * `Intel` - PCI vendor `0x8086`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Amd,
    Nvidia,
    Intel,
}

impl GpuVendor {
    /// Maps a PCI vendor id to a vendor.
    ///
    /// # Parameters
    /// * `id` - the PCI vendor id, as `/sys/bus/pci/devices/<addr>/vendor`
    ///   reports it.
    ///
    /// # Returns
    /// The vendor, or `None` for any id the `vendor` option cannot express - a
    /// virtio or QXL display adapter, or a server BMC - which must leave both
    /// slots empty rather than be written as a value the Nix enum rejects.
    fn from_pci_vendor(id: u32) -> Option<Self> {
        match id {
            0x1002 => Some(Self::Amd),
            0x10de => Some(Self::Nvidia),
            0x8086 => Some(Self::Intel),
            _ => None,
        }
    }

    /// The vendor's Nix spelling.
    ///
    /// # Returns
    /// `"amd"`, `"nvidia"` or `"intel"` - the three values the
    /// `mx.hardware.gpu.<slot>.vendor` enum accepts.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Amd => "amd",
            Self::Nvidia => "nvidia",
            Self::Intel => "intel",
        }
    }
}

/// One graphics controller as `sysfs` describes it, before it is assigned to a
/// slot.
///
/// # Fields
/// * `vendor` - the vendor its PCI vendor id names.
/// * `pci_id` - `vendor:device`, lowercase hexadecimal.
/// * `pci_addr` - PCI address without the domain (`c5:00.0`).
/// * `drm_index` - the `N` of the `drm/cardN` the device owns, `None` when no
///   driver claimed it.
/// * `boot_vga` - whether the firmware booted on this device.
/// * `vram_total` - `mem_info_vram_total` in bytes, `None` when the attribute
///   does not exist (non-`amdgpu` drivers, or no driver at all).
/// * `dedicated_vram` - whether `mem_info_vram_vendor` exists and names a
///   memory vendor, which `amdgpu` only does for real dedicated memory.
#[derive(Debug, Clone)]
struct PciGpu {
    vendor: GpuVendor,
    pci_id: String,
    pci_addr: String,
    drm_index: Option<u32>,
    boot_vga: bool,
    vram_total: Option<u64>,
    dedicated_vram: bool,
}

/// One filled `mx.hardware.gpu` slot.
///
/// # Fields
/// * `vendor` - value of `.vendor`. Not optional: an absent slot is a `None`
///   slot, so a half-filled slot cannot be represented.
/// * `generation` - value of `.generation`, `None` when the chipset name is
///   unavailable or unrecognised. `None` is the option's own default and is
///   always safe.
/// * `pci_id` - value of `.pciId`, as `vendor:device` in lowercase
///   hexadecimal.
/// * `pci_addr` - value of `.pciAddr`, without the PCI domain: mxpkgs builds
///   `DRI_PRIME=pci-0000_<addr>` out of it, so a domain here would be doubled.
/// * `drm_index` - value of `.drmIndex`, the card's own number rather than its
///   rank among the cards.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GpuSlot {
    pub vendor: GpuVendor,
    pub generation: Option<String>,
    pub pci_id: String,
    pub pci_addr: String,
    pub drm_index: Option<u32>,
}

/// The PRIME bus ids `hardware.nvidia.prime` needs, in NixOS spelling.
///
/// # Fields
/// * `nvidia` - `mx.hardware.gpu.nvidia.nvidiaBusId`.
/// * `intel` - `mx.hardware.gpu.nvidia.intelBusId`.
/// * `amd` - `mx.hardware.gpu.nvidia.amdBusId`.
///
/// # Post-conditions
/// At most one of `intel`/`amd` is ever `Some`, and all three are `None`
/// together whenever `nvidia` is - which is what makes the assertions
/// `mx.hardware.gpu.nvidia` declares hold by construction.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PrimeBusIds {
    pub nvidia: Option<String>,
    pub intel: Option<String>,
    pub amd: Option<String>,
}

/// Everything about this machine the `mx.hardware.*` options describe.
///
/// # Fields
/// * `dedicated` - the discrete GPU, or the sole GPU of a single-GPU machine.
/// * `integrated` - the iGPU, only ever filled when the machine has at least
///   two GPUs.
/// * `laptop` - value of `mx.hardware.laptop`.
/// * `has_fingerprint` - value of `mx.hardware.has_fingerprint`.
/// * `chipset_names` - whether `lspci` could be run. When `false` every
///   `generation` is `None`, which is harmless for AMD and Intel but gives a
///   Fermi-to-Volta NVIDIA card the current driver branch instead of its
///   legacy one; a caller can warn on it.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HardwareTopology {
    pub dedicated: Option<GpuSlot>,
    pub integrated: Option<GpuSlot>,
    pub laptop: bool,
    pub has_fingerprint: bool,
    pub chipset_names: bool,
}

/// Reads a `sysfs` attribute, trimming the newline it ends with.
///
/// # Parameters
/// * `path` - the attribute file.
///
/// # Returns
/// The trimmed contents, or `None` when the file does not exist, cannot be
/// read, or is empty once trimmed - the normal way an optional attribute
/// reports its own absence.
fn read_attr(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Parses a `sysfs` hexadecimal attribute such as `0x030000`.
///
/// # Parameters
/// * `value` - the attribute's trimmed contents, with or without the `0x`
///   prefix.
///
/// # Returns
/// The parsed value, or `None` when it is not hexadecimal.
fn parse_hex(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

/// Tells whether a PCI class is a graphics controller.
///
/// # Parameters
/// * `class` - the full class/subclass/prog-if triple, as
///   `/sys/bus/pci/devices/<addr>/class` reports it (`0x030000`).
///
/// # Returns
/// `true` for `0300` (VGA compatible controller), `0302` (3D controller - how
/// a laptop's discrete GPU usually appears, having no display output of its
/// own) and `0380` (Display controller). The prog-if byte is ignored.
fn is_gpu_class(class: u32) -> bool {
    matches!(class >> 8, 0x0300 | 0x0302 | 0x0380)
}

/// Drops the domain from a PCI address.
///
/// # Parameters
/// * `addr` - a PCI address, with (`0000:c5:00.0`) or without (`c5:00.0`) its
///   domain.
///
/// # Returns
/// The `bus:device.function` part. mxpkgs renders `.pciAddr` into
/// `DRI_PRIME=pci-0000_<addr>`, so keeping the domain would yield
/// `pci-0000_0000_c5_00_0`.
fn strip_pci_domain(addr: &str) -> &str {
    match addr.matches(':').count() {
        2 => addr.split_once(':').map(|(_, rest)| rest).unwrap_or(addr),
        _ => addr,
    }
}

/// Reads the card number out of a DRM entry name.
///
/// # Parameters
/// * `name` - a directory name found in a device's `drm/`.
///
/// # Returns
/// The `N` of `cardN`, or `None` for anything else - which is what rejects the
/// `controlD65` and `renderD128` nodes sitting next to it.
fn drm_index_from_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("card")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Reads the DRM card number a PCI device owns.
///
/// # Parameters
/// * `device` - the device's directory under `/sys/bus/pci/devices`.
///
/// # Returns
/// The `N` of its `drm/cardN`, or `None` when the device has no `drm/`
/// directory at all - which is the case for a card whose kernel module is not
/// loaded, and is a legitimate `drmIndex = null`.
///
/// # Post-conditions
/// The number is the card's own, not its rank among the machine's cards: a
/// machine can perfectly well have `card1` and `card2` and no `card0`.
fn read_drm_index(device: &Path) -> Option<u32> {
    fs::read_dir(device.join("drm"))
        .ok()?
        .flatten()
        .filter_map(|entry| drm_index_from_name(&entry.file_name().to_string_lossy()))
        .min()
}

/// Tells whether a DMI chassis type denotes a portable machine.
///
/// # Parameters
/// * `value` - the trimmed contents of `chassis_type`.
///
/// # Returns
/// `Some(true)` for the types in [`PORTABLE_CHASSIS`], `Some(false)` for any
/// other known type, and `None` for the types that carry no information
/// ([`UNINFORMATIVE_CHASSIS`]) and for a value that is not a number - in both
/// of which cases the caller has to look elsewhere.
fn chassis_is_portable(value: &str) -> Option<bool> {
    let chassis = value.parse::<u32>().ok()?;
    if UNINFORMATIVE_CHASSIS.contains(&chassis) {
        return None;
    }
    Some(PORTABLE_CHASSIS.contains(&chassis))
}

/// Reads the CPU vendor from a `/proc/cpuinfo` dump.
///
/// Used to tell an iGPU from a discrete card: the integrated GPU is the one
/// whose vendor is the CPU's. Deliberately not [`super::cpu_info::CpuInfo`],
/// which needs the `cpuid` binary and fails outright on any vendor other than
/// Intel or AMD - that would make a missing `cpuid` lose the whole GPU
/// detection.
///
/// # Parameters
/// * `content` - the contents of `/proc/cpuinfo`.
///
/// # Returns
/// [`GpuVendor::Amd`] for `AuthenticAMD`, [`GpuVendor::Intel`] for
/// `GenuineIntel`, `None` for any other vendor and for a `cpuinfo` without a
/// `vendor_id` line, as every non-x86 architecture has.
fn cpu_vendor(content: &str) -> Option<GpuVendor> {
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "vendor_id" {
            continue;
        }
        return match value.trim() {
            "AuthenticAMD" => Some(GpuVendor::Amd),
            "GenuineIntel" => Some(GpuVendor::Intel),
            _ => None,
        };
    }
    None
}

/// Normalises a device description for substring matching against the
/// generation tables.
///
/// # Parameters
/// * `description` - the description `lspci` printed.
///
/// # Returns
/// The description lowercased with every ASCII whitespace removed, so that one
/// table key matches both spellings `lspci` uses - `TigerLake-LP` and `Tiger
/// Lake-P`, `Navi 31` and `Navi31`.
fn normalise_description(description: &str) -> String {
    description
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Reads an NVIDIA architecture off a device description.
///
/// # Parameters
/// * `description` - the description `lspci` printed for that one device.
///
/// # Returns
/// The architecture name, or `None` when the description carries no
/// recognisable chipset code - a pre-Fermi card, for instance, which no
/// current driver branch covers. The pattern is a real alternation over
/// [`NVIDIA_GENERATIONS`], so an unlisted two-letter prefix simply fails to
/// match instead of being looked up.
fn nvidia_generation(description: &str) -> Option<String> {
    let prefixes = NVIDIA_GENERATIONS
        .iter()
        .map(|(code, _)| *code)
        .collect::<Vec<_>>()
        .join("|");
    let pattern = Regex::new(&format!(r"\b({})\d{{2,3}}[A-Z]{{0,2}}\b", prefixes)).ok()?;
    let code = pattern.captures(description)?.get(1)?.as_str();
    NVIDIA_GENERATIONS
        .iter()
        .find(|(prefix, _)| *prefix == code)
        .map(|(_, arch)| (*arch).to_string())
}

/// Reads an architecture off a device description using a normalised table.
///
/// # Parameters
/// * `description` - the description `lspci` printed for that one device.
/// * `table` - family-name/architecture pairs whose keys are already
///   lowercased and whitespace-free.
///
/// # Returns
/// The architecture of the first table entry whose key the description
/// contains, or `None`.
///
/// # Pre-conditions
/// `table` is ordered so that a key which is a substring of another comes
/// after it.
fn table_generation(description: &str, table: &[(&str, &str)]) -> Option<String> {
    let haystack = normalise_description(description);
    table
        .iter()
        .find(|(family, _)| haystack.contains(family))
        .map(|(_, arch)| (*arch).to_string())
}

/// Reads a GPU's generation codename off its description.
///
/// # Parameters
/// * `vendor` - the device's vendor, which selects the table.
/// * `description` - the description `lspci` printed for that one device, or
///   `None` when `lspci` is unavailable.
///
/// # Returns
/// The codename, or `None` when there is no description or none of the
/// vendor's families matches. `None` never misconfigures the system: it is the
/// option's default, and it selects the modern VAAPI driver for Intel, the
/// non-legacy branch for NVIDIA and no Proton upscaler upgrade for AMD.
fn generation_for(vendor: GpuVendor, description: Option<&str>) -> Option<String> {
    let description = description?;
    match vendor {
        GpuVendor::Nvidia => nvidia_generation(description),
        GpuVendor::Amd => table_generation(description, &AMD_GENERATIONS),
        GpuVendor::Intel => table_generation(description, &INTEL_GENERATIONS),
    }
}

/// Parses `lspci -nn` output into a description per PCI address.
///
/// # Parameters
/// * `output` - the command's stdout.
///
/// # Returns
/// A map from the address `lspci` printed - domain-less, as `lspci` prints it
/// without `-D` - to the text after the class, which is the vendor and chipset
/// name. Lines carrying no `": "` separator are skipped, so non-device noise
/// costs nothing.
pub(crate) fn parse_lspci(output: &str) -> HashMap<String, String> {
    let mut devices = HashMap::new();
    for line in output.lines() {
        let Some((head, description)) = line.split_once(": ") else {
            continue;
        };
        let Some(addr) = head.split_whitespace().next() else {
            continue;
        };
        devices.insert(
            strip_pci_domain(addr).to_string(),
            description.trim().to_string(),
        );
    }
    devices
}

/// Runs `lspci -nn` to collect the chipset names.
///
/// # Returns
/// A description per PCI address, and `None` when `lspci` cannot be run or
/// exits non-zero. Never an error: `pciutils` is only needed to name a
/// generation, every other field comes from `sysfs`, so its absence must cost
/// one option and not the whole detection.
fn lspci_descriptions() -> Option<HashMap<String, String>> {
    let output = Command::new("lspci").arg("-nn").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_lspci(&String::from_utf8_lossy(&output.stdout)))
}

/// Lists the machine's graphics controllers from `sysfs`.
///
/// # Parameters
/// * `sysfs` - the `sysfs` mount point.
///
/// # Returns
/// One entry per graphics controller whose PCI vendor id is one the `vendor`
/// option can express, sorted by PCI address so the result is stable across
/// runs.
///
/// # Post-conditions
/// Enumeration goes through `/sys/bus/pci/devices` rather than
/// `/sys/class/drm` because a card whose kernel module is not loaded has no
/// DRM node at all - exactly the installer-on-a-live-medium case for an
/// NVIDIA card. A machine with no graphics controller yields an empty list,
/// and so does a `sysfs` exposing no PCI bus at all, as a container's does:
/// both mean "no GPU to configure", which is a legitimate answer and not a
/// failure.
///
/// # Errors
/// [`mx::ErrorKind::IOError`], naming the path, when
/// `<sysfs>/bus/pci/devices` exists but cannot be listed.
fn list_gpus(sysfs: &Path) -> mx::Result<Vec<PciGpu>> {
    let devices_dir = sysfs.join("bus/pci/devices");
    let entries = match fs::read_dir(&devices_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error_at(&devices_dir.to_string_lossy(), e)),
    };

    let mut gpus: Vec<PciGpu> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let addr = entry.file_name().to_string_lossy().into_owned();

        let Some(class) = read_attr(&path.join("class"))
            .as_deref()
            .and_then(parse_hex)
        else {
            continue;
        };
        if !is_gpu_class(class) {
            continue;
        }
        let Some(vendor_id) = read_attr(&path.join("vendor"))
            .as_deref()
            .and_then(parse_hex)
        else {
            continue;
        };
        let Some(vendor) = GpuVendor::from_pci_vendor(vendor_id) else {
            continue;
        };
        let Some(device_id) = read_attr(&path.join("device"))
            .as_deref()
            .and_then(parse_hex)
        else {
            continue;
        };

        gpus.push(PciGpu {
            vendor,
            pci_id: format!("{:04x}:{:04x}", vendor_id, device_id),
            pci_addr: strip_pci_domain(&addr).to_string(),
            drm_index: read_drm_index(&path),
            boot_vga: read_attr(&path.join("boot_vga")).as_deref() == Some("1"),
            vram_total: read_attr(&path.join("mem_info_vram_total"))
                .and_then(|v| v.parse::<u64>().ok()),
            dedicated_vram: read_attr(&path.join("mem_info_vram_vendor")).is_some(),
        });
    }

    gpus.sort_by(|a, b| a.pci_addr.cmp(&b.pci_addr));
    Ok(gpus)
}

/// Picks the integrated GPU among same-vendor candidates.
///
/// # Parameters
/// * `gpus` - every graphics controller found.
/// * `candidates` - indices of the candidates, all of the same vendor, at
///   least two of them.
///
/// # Returns
/// The index of the iGPU, or `None` when no signal distinguishes one.
///
/// # Post-conditions
/// For AMD the decision rests on dedicated-memory evidence: `amdgpu` exposes
/// `mem_info_vram_vendor` only for real video memory, so the card without it
/// is the APU's carve-out; failing that, a card whose `mem_info_vram_total` is
/// at least [`VRAM_DEDICATED_RATIO`] times another's is the discrete one. For
/// Intel it rests on the root bus, an iGPU always sitting at `00:02.0` while
/// an Arc card never does. The PCI bus number is deliberately not used as a
/// general signal: on a Framework 16 the iGPU is at `c5:00.0` and the discrete
/// card at `03:00.0`.
fn same_vendor_integrated(gpus: &[PciGpu], candidates: &[usize]) -> Option<usize> {
    match gpus[candidates[0]].vendor {
        GpuVendor::Amd => {
            let without: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&index| !gpus[index].dedicated_vram)
                .collect();
            if !without.is_empty() && without.len() < candidates.len() {
                return without.first().copied();
            }
            let smallest = candidates
                .iter()
                .copied()
                .min_by_key(|&index| gpus[index].vram_total.unwrap_or(u64::MAX))?;
            let largest = candidates
                .iter()
                .copied()
                .max_by_key(|&index| gpus[index].vram_total.unwrap_or(0))?;
            let small = gpus[smallest].vram_total?;
            let large = gpus[largest].vram_total?;
            (small != 0 && large >= small.saturating_mul(VRAM_DEDICATED_RATIO)).then_some(smallest)
        }
        GpuVendor::Intel => candidates
            .iter()
            .copied()
            .find(|&index| gpus[index].pci_addr.starts_with("00:")),
        GpuVendor::Nvidia => None,
    }
}

/// Picks the integrated GPU out of a multi-GPU machine.
///
/// # Parameters
/// * `gpus` - every graphics controller found, at least two of them, sorted by
///   PCI address.
/// * `cpu_vendor` - the CPU's vendor, `None` when it could not be read.
///
/// # Returns
/// The index of the iGPU in `gpus`, or `None` when nothing identifies one - in
/// which case the caller leaves the integrated slot empty rather than guess.
///
/// # Post-conditions
/// Never returns an NVIDIA card: NVIDIA ships no integrated graphics, so a
/// slot filled with one would break the PRIME assertions. Never returns a card
/// that shows dedicated-memory evidence, which is what keeps a second discrete
/// card out of the integrated slot on a two-dGPU machine. When the CPU vendor
/// is known, only a card of that vendor is eligible, which keeps a discrete
/// AMD card in an Intel machine out of the slot. As a last resort the
/// firmware's boot device is taken, which is driver-independent and therefore
/// still right when no kernel module is loaded - though it is deliberately
/// last, since a MUX-equipped laptop and most desktops boot on the discrete
/// card.
fn find_integrated(gpus: &[PciGpu], cpu_vendor: Option<GpuVendor>) -> Option<usize> {
    let candidates: Vec<usize> = gpus
        .iter()
        .enumerate()
        .filter(|(_, gpu)| gpu.vendor != GpuVendor::Nvidia)
        .filter(|(_, gpu)| cpu_vendor.is_none_or(|vendor| vendor == gpu.vendor))
        .map(|(index, _)| index)
        .collect();

    if candidates.iter().all(|&index| gpus[index].dedicated_vram) {
        return None;
    }

    let picked = match candidates.as_slice() {
        [] => None,
        [only] => Some(*only),
        many => {
            let vendor = gpus[many[0]].vendor;
            if many.iter().any(|&index| gpus[index].vendor != vendor) {
                None
            } else {
                same_vendor_integrated(gpus, many).or_else(|| {
                    let booting: Vec<usize> = many
                        .iter()
                        .copied()
                        .filter(|&index| gpus[index].boot_vga)
                        .collect();
                    match booting.as_slice() {
                        [only] => Some(*only),
                        _ => None,
                    }
                })
            }
        }
    };

    picked.filter(|&index| !gpus[index].dedicated_vram)
}

/// Picks the dedicated GPU out of the cards the integrated slot did not take.
///
/// # Parameters
/// * `gpus` - the remaining graphics controllers.
///
/// # Returns
/// An NVIDIA card if there is one - it is never anything but discrete -
/// otherwise the card with the most video memory, which falls back to the
/// first by PCI address when no card reports any. `None` for an empty list.
fn pick_dedicated(gpus: Vec<PciGpu>) -> Option<PciGpu> {
    if let Some(index) = gpus.iter().position(|gpu| gpu.vendor == GpuVendor::Nvidia) {
        return gpus.into_iter().nth(index);
    }
    gpus.into_iter()
        .enumerate()
        .max_by_key(|(index, gpu)| (gpu.vram_total.unwrap_or(0), usize::MAX - index))
        .map(|(_, gpu)| gpu)
}

/// Assigns the detected graphics controllers to the two option slots.
///
/// # Parameters
/// * `gpus` - every graphics controller found, sorted by PCI address.
/// * `cpu_vendor` - the CPU's vendor, `None` when it could not be read.
///
/// # Returns
/// `(dedicated, integrated)`.
///
/// # Post-conditions
/// A machine with a single GPU puts it in the dedicated slot and leaves the
/// integrated one empty, which is what the option documentation's own
/// single-card example does; it also keeps mxpkgs from building an
/// `igpu-launch` wrapper, which it does as soon as `integrated.pciId` is set
/// and which has nothing to switch to on a single-GPU machine.
fn split_slots(
    gpus: Vec<PciGpu>,
    cpu_vendor: Option<GpuVendor>,
) -> (Option<PciGpu>, Option<PciGpu>) {
    if gpus.len() <= 1 {
        return (gpus.into_iter().next(), None);
    }
    match find_integrated(&gpus, cpu_vendor) {
        Some(index) => {
            let mut rest = gpus;
            let integrated = rest.remove(index);
            (pick_dedicated(rest), Some(integrated))
        }
        None => (pick_dedicated(gpus), None),
    }
}

/// Turns a detected controller into a slot, naming its generation.
///
/// # Parameters
/// * `gpu` - the controller.
/// * `descriptions` - chipset names per PCI address, `None` when `lspci` could
///   not be run.
///
/// # Returns
/// The slot, with `generation` set when a description exists and names a
/// family the vendor's table lists.
fn build_slot(gpu: PciGpu, descriptions: Option<&HashMap<String, String>>) -> GpuSlot {
    let description = descriptions.and_then(|map| map.get(&gpu.pci_addr));
    GpuSlot {
        vendor: gpu.vendor,
        generation: generation_for(gpu.vendor, description.map(String::as_str)),
        pci_id: gpu.pci_id,
        pci_addr: gpu.pci_addr,
        drm_index: gpu.drm_index,
    }
}

/// Tells whether the machine is a laptop.
///
/// # Parameters
/// * `sysfs` - the `sysfs` mount point.
///
/// # Returns
/// `true` when DMI reports a portable chassis. When `chassis_type` is
/// unreadable or uninformative - a container, or a virtual machine without
/// DMI - falls back to the presence of a system battery, which is weaker: it
/// only excludes the peripheral batteries (a wireless mouse) by their
/// `scope`, so a desktop whose UPS the kernel exposes as a system battery
/// still reads as a laptop. DMI is tried first precisely to keep that
/// fallback off the common path.
fn detect_laptop(sysfs: &Path) -> bool {
    if let Some(verdict) = read_attr(&sysfs.join("devices/virtual/dmi/id/chassis_type"))
        .as_deref()
        .and_then(chassis_is_portable)
    {
        return verdict;
    }
    has_system_battery(sysfs)
}

/// Tells whether the machine carries a battery of its own.
///
/// # Parameters
/// * `sysfs` - the `sysfs` mount point.
///
/// # Returns
/// `true` when `<sysfs>/class/power_supply` holds an entry whose `type` is
/// `Battery` and whose `scope` is not `Device`. The `scope` test is what
/// rejects a wireless mouse's or keyboard's battery, which the kernel exposes
/// in the same directory. An unreadable directory reads as "no battery", not
/// as an error.
fn has_system_battery(sysfs: &Path) -> bool {
    let Ok(entries) = fs::read_dir(sysfs.join("class/power_supply")) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if read_attr(&path.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        if read_attr(&path.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        return true;
    }
    false
}

/// Tells whether the running machine is a laptop.
///
/// # Returns
/// [`detect_laptop`] applied to the live `sysfs`, i.e. the DMI chassis type
/// when it is conclusive and the presence of a system battery otherwise. This
/// is the same verdict [`HardwareTopology::detect`] stores in its `laptop`
/// field, exposed on its own for the callers that need the fact without a
/// full GPU probe.
///
/// # Pre-conditions
/// Reads the live machine under `/sys`, never an installation root.
pub fn machine_is_laptop() -> bool {
    detect_laptop(Path::new(SYSFS_ROOT))
}

impl HardwareTopology {
    /// Probes the running machine.
    ///
    /// # Returns
    /// The two GPU slots plus the `laptop` and `has_fingerprint` facts.
    ///
    /// # Pre-conditions
    /// Reads the **live** machine, never an installation root: an installer
    /// running from a live medium is on the hardware it is installing onto, so
    /// the live machine is the target.
    ///
    /// # Post-conditions
    /// `lspci`, a DRM node, DMI, the USB bus under `sysfs` and the `amdgpu`
    /// memory attributes are all optional: each one missing costs exactly the field it feeds and
    /// leaves the rest detected. Only an unlistable `/sys/bus/pci/devices` is
    /// fatal.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] when `/sys/bus/pci/devices` cannot be
    /// listed, as opposed to being absent.
    pub fn detect() -> mx::Result<Self> {
        Self::detect_from(
            Path::new(SYSFS_ROOT),
            Path::new(CPUINFO_PATH),
            lspci_descriptions().as_ref(),
            super::computer_info::ComputerInfo::has_fingerprint_device(),
        )
    }

    /// Assembles a topology from sources the caller has already collected.
    ///
    /// Every impure input is a parameter, so a test can hand this a fixture
    /// tree and a canned `lspci` output instead of the running kernel.
    ///
    /// # Parameters
    /// * `sysfs` - the `sysfs` mount point.
    /// * `cpuinfo` - the `/proc/cpuinfo` file, read for the CPU vendor only.
    /// * `descriptions` - chipset names per PCI address, as [`parse_lspci`]
    ///   produces them; `None` when `lspci` could not be run, which leaves
    ///   every `generation` unset.
    /// * `has_fingerprint` - whether a fingerprint reader was detected.
    ///
    /// # Returns
    /// The topology, as in [`HardwareTopology::detect`].
    ///
    /// # Errors
    /// As in [`HardwareTopology::detect`].
    pub(crate) fn detect_from(
        sysfs: &Path,
        cpuinfo: &Path,
        descriptions: Option<&HashMap<String, String>>,
        has_fingerprint: bool,
    ) -> mx::Result<Self> {
        let gpus = list_gpus(sysfs)?;
        let cpu = fs::read_to_string(cpuinfo)
            .ok()
            .as_deref()
            .and_then(cpu_vendor);

        let (dedicated, integrated) = split_slots(gpus, cpu);

        Ok(Self {
            dedicated: dedicated.map(|gpu| build_slot(gpu, descriptions)),
            integrated: integrated.map(|gpu| build_slot(gpu, descriptions)),
            laptop: detect_laptop(sysfs),
            has_fingerprint,
            chipset_names: descriptions.is_some(),
        })
    }

    /// The PRIME bus ids this topology calls for.
    ///
    /// # Returns
    /// All three `None` unless the machine is a laptop whose dedicated GPU is
    /// NVIDIA and whose integrated GPU is Intel or AMD; otherwise `nvidia`
    /// plus exactly one of `intel`/`amd`.
    ///
    /// # Post-conditions
    /// Satisfies every assertion `mx.hardware.gpu.nvidia` makes, by
    /// construction: `intelBusId` and `amdBusId` are never both set, each is
    /// set only alongside the matching `integrated.vendor`, and `nvidiaBusId`
    /// is never set on its own. PRIME is a laptop feature and is pointless on
    /// a desktop with two independent heads, so the laptop condition also
    /// keeps the ids off a machine that has no use for them.
    pub fn prime_bus_ids(&self) -> PrimeBusIds {
        let empty = PrimeBusIds::default();
        if !self.laptop {
            return empty;
        }
        let (Some(dedicated), Some(integrated)) = (&self.dedicated, &self.integrated) else {
            return empty;
        };
        if dedicated.vendor != GpuVendor::Nvidia {
            return empty;
        }
        let (Some(nvidia), Some(igpu)) = (
            bus_id_nix(&dedicated.pci_addr),
            bus_id_nix(&integrated.pci_addr),
        ) else {
            return empty;
        };
        match integrated.vendor {
            GpuVendor::Intel => PrimeBusIds {
                nvidia: Some(nvidia),
                intel: Some(igpu),
                amd: None,
            },
            GpuVendor::Amd => PrimeBusIds {
                nvidia: Some(nvidia),
                intel: None,
                amd: Some(igpu),
            },
            GpuVendor::Nvidia => empty,
        }
    }
}

/// Rewrites a PCI address into the `PCI:bus:device:function` form NixOS wants.
///
/// # Parameters
/// * `addr` - a PCI address as `lspci` prints it (`c5:00.0`), with or without
///   a domain prefix, which is dropped.
///
/// # Returns
/// `PCI:<bus>:<device>:<function>` with bus and device converted from
/// hexadecimal to **decimal** and no leading zeros, so bus `c5` becomes `197`
/// and not `c5`. `None` when the address has fewer than three components or
/// one of them does not parse.
fn bus_id_nix(addr: &str) -> Option<String> {
    let parts: Vec<&str> = addr.split([':', '.']).collect();
    if parts.len() < 3 {
        return None;
    }
    let bus = u32::from_str_radix(parts[parts.len() - 3], 16).ok()?;
    let device = u32::from_str_radix(parts[parts.len() - 2], 16).ok()?;
    let function = parts[parts.len() - 1].parse::<u32>().ok()?;
    Some(format!("PCI:{}:{}:{}", bus, device, function))
}

#[cfg(test)]
#[path = "gpu_info_tests.rs"]
mod tests;
