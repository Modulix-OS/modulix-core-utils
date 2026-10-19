//! Tests for [`super`].
//!
//! The probes read `sysfs` text files, so the seam is a fixture tree rather
//! than a mocked subprocess: [`Sysfs`] builds the directories and attribute
//! files a real `/sys` has, and the `lspci` output and the fingerprint verdict
//! arrive as plain parameters of
//! [`super::HardwareTopology::detect_from`].

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

/// A fixture `sysfs` tree.
struct Sysfs {
    dir: TempDir,
}

/// One PCI device being described inside a [`Sysfs`].
struct Dev {
    path: PathBuf,
}

impl Sysfs {
    /// Creates an empty tree holding no device at all.
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    /// The tree's root, to be passed where `/sys` would be.
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Writes one attribute file, creating its parents.
    fn write(&self, rel: &str, content: &str) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    /// Adds a graphics controller, with the four attributes every PCI device
    /// has.
    ///
    /// # Parameters
    /// * `addr` - domain-qualified address, as `sysfs` names the directory.
    /// * `pci_id` - `vendor:device` in hexadecimal.
    /// * `class` - the `class` attribute's contents.
    fn gpu(&self, addr: &str, pci_id: &str, class: &str) -> Dev {
        let (vendor, device) = pci_id.split_once(':').expect("vendor:device");
        let rel = format!("bus/pci/devices/{addr}");
        self.write(&format!("{rel}/class"), &format!("{class}\n"));
        self.write(&format!("{rel}/vendor"), &format!("0x{vendor}\n"));
        self.write(&format!("{rel}/device"), &format!("0x{device}\n"));
        self.write(&format!("{rel}/boot_vga"), "0\n");
        Dev {
            path: self.root().join(rel),
        }
    }

    /// Writes the DMI chassis type.
    fn chassis(&self, value: &str) -> &Self {
        self.write("devices/virtual/dmi/id/chassis_type", &format!("{value}\n"));
        self
    }

    /// Adds a power supply entry.
    ///
    /// # Parameters
    /// * `name` - the entry's name.
    /// * `kind` - its `type` attribute.
    /// * `scope` - its `scope` attribute, omitted when `None` as it is for a
    ///   system battery.
    fn power_supply(&self, name: &str, kind: &str, scope: Option<&str>) -> &Self {
        self.write(
            &format!("class/power_supply/{name}/type"),
            &format!("{kind}\n"),
        );
        if let Some(scope) = scope {
            self.write(
                &format!("class/power_supply/{name}/scope"),
                &format!("{scope}\n"),
            );
        }
        self
    }

    /// Writes a `cpuinfo` naming a vendor, and returns its path.
    fn cpuinfo(&self, vendor_id: &str) -> PathBuf {
        self.write(
            "proc_cpuinfo",
            &format!("processor\t: 0\nvendor_id\t: {vendor_id}\nmodel name\t: whatever\n"),
        );
        self.root().join("proc_cpuinfo")
    }

    /// A path that does not exist, for the "CPU vendor unknown" case.
    fn no_cpuinfo(&self) -> PathBuf {
        self.root().join("absent_cpuinfo")
    }
}

impl Dev {
    /// Gives the device a DRM node, plus the control and render nodes that sit
    /// next to it and must be ignored.
    fn drm_card(self, number: u32) -> Self {
        for name in [
            format!("card{number}"),
            format!("controlD{}", 64 + number),
            format!("renderD{}", 128 + number),
        ] {
            fs::create_dir_all(self.path.join("drm").join(name)).expect("mkdir drm");
        }
        self
    }

    /// Marks the device as the one the firmware booted on.
    fn boot_vga(self) -> Self {
        fs::write(self.path.join("boot_vga"), "1\n").expect("write");
        self
    }

    /// Sets `mem_info_vram_total`.
    fn vram_total(self, bytes: u64) -> Self {
        fs::write(self.path.join("mem_info_vram_total"), format!("{bytes}\n")).expect("write");
        self
    }

    /// Sets `mem_info_vram_vendor`, which `amdgpu` only exposes for real
    /// dedicated memory.
    fn vram_vendor(self, vendor: &str) -> Self {
        fs::write(
            self.path.join("mem_info_vram_vendor"),
            format!("{vendor}\n"),
        )
        .expect("write");
        self
    }
}

/// Builds a topology from a fixture tree, with no `lspci` and no fingerprint
/// reader.
fn detect(sysfs: &Sysfs, cpuinfo: &Path) -> HardwareTopology {
    HardwareTopology::detect_from(sysfs.root(), cpuinfo, None, false).expect("detect")
}

mod lspci {
    use super::*;

    #[test]
    fn nested_brackets_and_revision_keep_the_whole_description() {
        let line = "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106M \
                    [GeForce RTX 3060 Mobile / Max-Q] [10de:2520] (rev a1)";
        let devices = parse_lspci(line);
        assert_eq!(
            devices.get("01:00.0").map(String::as_str),
            Some(
                "NVIDIA Corporation GA106M [GeForce RTX 3060 Mobile / Max-Q] \
                 [10de:2520] (rev a1)"
            )
        );
    }

    #[test]
    fn a_3d_controller_is_parsed_like_a_vga_one() {
        let line = "01:00.0 3D controller [0302]: NVIDIA Corporation GA107M [8086:0000]";
        assert!(parse_lspci(line).contains_key("01:00.0"));
    }

    #[test]
    fn a_domain_prefix_is_dropped_from_the_key() {
        let line = "0000:c5:00.0 VGA compatible controller [0300]: AMD Phoenix1 [1002:15bf]";
        let devices = parse_lspci(line);
        assert!(devices.contains_key("c5:00.0"));
        assert!(!devices.contains_key("0000:c5:00.0"));
    }

    #[test]
    fn garbage_yields_no_device_and_does_not_panic() {
        assert!(parse_lspci("").is_empty());
        assert!(parse_lspci("not a device line\n\n").is_empty());
    }
}

mod generation {
    use super::*;

    /// `volta` selects the `legacy_580` NVIDIA branch, so a missing `GV`
    /// prefix hands a Volta card the current driver, which cannot drive it.
    #[test]
    fn volta_is_recognised() {
        assert_eq!(
            generation_for(
                GpuVendor::Nvidia,
                Some("NVIDIA Corporation GV100 [TITAN V]")
            ),
            Some("volta".to_string())
        );
    }

    /// The prefixes must be an alternation, not a character class: a class
    /// would match any two of their letters and the lookup that follows used
    /// to panic on the result.
    #[test]
    fn an_unlisted_two_letter_prefix_matches_nothing() {
        for description in ["NVIDIA MP204", "NVIDIA GG100", "NVIDIA DD300"] {
            assert_eq!(generation_for(GpuVendor::Nvidia, Some(description)), None);
        }
    }

    #[test]
    fn nvidia_prefixes_map_to_their_architecture() {
        for (description, expected) in [
            ("NVIDIA GF119 [GeForce GT 610]", "fermi"),
            ("NVIDIA GK208B [GeForce GT 710]", "kepler"),
            ("NVIDIA GA106M [GeForce RTX 3060 Mobile]", "ampere"),
            ("NVIDIA AD107M [GeForce RTX 4060 Max-Q]", "ada-lovelace"),
            ("NVIDIA GB203 [GeForce RTX 5080]", "blackwell"),
        ] {
            assert_eq!(
                generation_for(GpuVendor::Nvidia, Some(description)),
                Some(expected.to_string()),
                "{description}"
            );
        }
    }

    #[test]
    fn a_pre_fermi_card_has_no_generation() {
        assert_eq!(
            generation_for(GpuVendor::Nvidia, Some("NVIDIA G92 [GeForce 9800 GT]")),
            None
        );
    }

    /// The Arrandale description is a substring of the Haswell one, so table
    /// order decides the answer.
    #[test]
    fn haswell_wins_over_the_arrandale_substring() {
        assert_eq!(
            generation_for(
                GpuVendor::Intel,
                Some(
                    "Intel Corporation Xeon E3-1200 v3/4th Gen Core Processor \
                     Integrated Graphics Controller"
                )
            ),
            Some("haswell".to_string())
        );
        assert_eq!(
            generation_for(
                GpuVendor::Intel,
                Some("Intel Corporation Core Processor Integrated Graphics Controller")
            ),
            Some("ironlake".to_string())
        );
    }

    #[test]
    fn intel_legacy_codenames_are_exact() {
        for (description, expected) in [
            ("Intel 2nd Generation Core Processor Family", "sandybridge"),
            ("Intel Xeon E3-1200 v2/3rd Gen Core processor", "ivybridge"),
            ("Intel Haswell-ULT Integrated Graphics", "haswell"),
            ("Intel Arrandale Integrated Graphics", "westmere"),
        ] {
            assert_eq!(
                generation_for(GpuVendor::Intel, Some(description)),
                Some(expected.to_string()),
                "{description}"
            );
        }
    }

    /// A modern Intel chip must not land on one of the five legacy codenames,
    /// which would pick the obsolete VAAPI driver. Which modern codename it
    /// gets, or none at all, changes nothing.
    #[test]
    fn a_modern_intel_chip_is_never_legacy() {
        const LEGACY: [&str; 5] = [
            "ironlake",
            "sandybridge",
            "ivybridge",
            "haswell",
            "westmere",
        ];
        for description in [
            "Intel Corporation TigerLake-LP GT2 [Iris Xe Graphics]",
            "Intel Corporation Raptor Lake-P [Iris Xe Graphics]",
            "Intel Corporation DG2 [Arc A770]",
            "Intel Corporation Some Unreleased Thing",
        ] {
            let generation = generation_for(GpuVendor::Intel, Some(description));
            assert!(
                !generation.as_deref().is_some_and(|g| LEGACY.contains(&g)),
                "{description} -> {generation:?}"
            );
        }
    }

    #[test]
    fn amd_families_and_apus_map_to_their_architecture() {
        for (description, expected) in [
            ("AMD Navi 33 [Radeon RX 7700S]", "rdna3"),
            ("AMD Navi 21 [Radeon RX 6800]", "rdna2"),
            ("AMD Navi 48 [Radeon RX 9070]", "rdna4"),
            ("AMD Phoenix1", "rdna3"),
            ("AMD Raven Ridge [Radeon Vega Series]", "gcn-5-gen"),
            ("AMD Polaris 20 [Radeon RX 580]", "gcn-4-gen"),
        ] {
            assert_eq!(
                generation_for(GpuVendor::Amd, Some(description)),
                Some(expected.to_string()),
                "{description}"
            );
        }
    }

    #[test]
    fn a_table_never_matches_another_vendors_description() {
        assert_eq!(
            generation_for(GpuVendor::Amd, Some("NVIDIA Corporation GA102")),
            None
        );
        assert_eq!(
            generation_for(GpuVendor::Nvidia, Some("AMD Navi 33 [Radeon RX 7700S]")),
            None
        );
    }

    #[test]
    fn no_description_means_no_generation() {
        assert_eq!(generation_for(GpuVendor::Nvidia, None), None);
    }
}

mod bus_id {
    use super::*;

    /// The bus is hexadecimal in a PCI address and decimal in a NixOS bus id,
    /// which only shows on a bus above `09`.
    #[test]
    fn the_bus_is_converted_to_decimal() {
        assert_eq!(bus_id_nix("c5:00.0").as_deref(), Some("PCI:197:0:0"));
    }

    #[test]
    fn leading_zeros_are_dropped_and_the_domain_ignored() {
        assert_eq!(bus_id_nix("01:00.0").as_deref(), Some("PCI:1:0:0"));
        assert_eq!(bus_id_nix("00:02.0").as_deref(), Some("PCI:0:2:0"));
        assert_eq!(bus_id_nix("0000:03:00.0").as_deref(), Some("PCI:3:0:0"));
    }

    #[test]
    fn an_unparseable_address_is_rejected_rather_than_panicking() {
        for addr in ["", "garbage", "1:2", "zz:00.0", "01:zz.0"] {
            assert_eq!(bus_id_nix(addr), None, "{addr}");
        }
    }
}

mod slots {
    use super::*;

    #[test]
    fn a_single_card_goes_to_the_dedicated_slot() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:744c", "0x030000")
            .drm_card(0);
        let topology = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"));

        let dedicated = topology.dedicated.expect("dedicated");
        assert_eq!(dedicated.vendor, GpuVendor::Amd);
        assert_eq!(dedicated.pci_id, "1002:744c");
        assert_eq!(dedicated.pci_addr, "03:00.0");
        assert_eq!(dedicated.drm_index, Some(0));
        assert!(topology.integrated.is_none());
    }

    #[test]
    fn an_nvidia_card_is_never_integrated() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:00:02.0", "8086:9a49", "0x030000")
            .drm_card(0);
        sysfs
            .gpu("0000:01:00.0", "10de:249d", "0x030200")
            .drm_card(1);
        let topology = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel"));

        assert_eq!(
            topology.dedicated.map(|slot| slot.vendor),
            Some(GpuVendor::Nvidia)
        );
        let integrated = topology.integrated.expect("integrated");
        assert_eq!(integrated.vendor, GpuVendor::Intel);
        assert_eq!(integrated.pci_addr, "00:02.0");
    }

    /// An AMD APU next to an AMD discrete card: same vendor, and the iGPU on
    /// the *higher* bus, which is why the bus number cannot be the signal.
    /// `mem_info_vram_vendor` exists only for real video memory.
    #[test]
    fn a_same_vendor_pair_is_split_on_dedicated_memory() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:7480", "0x030000")
            .drm_card(1)
            .vram_total(8_573_157_376)
            .vram_vendor("samsung");
        sysfs
            .gpu("0000:c5:00.0", "1002:15BF", "0x030000")
            .drm_card(2)
            .boot_vga()
            .vram_total(536_870_912);
        let topology = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"));

        let dedicated = topology.dedicated.expect("dedicated");
        let integrated = topology.integrated.expect("integrated");
        assert_eq!(dedicated.pci_addr, "03:00.0");
        assert_eq!(integrated.pci_addr, "c5:00.0");
        assert_eq!(dedicated.drm_index, Some(1));
        assert_eq!(integrated.drm_index, Some(2));
        assert_eq!(integrated.pci_id, "1002:15bf", "the id must be lowercased");
    }

    /// Same machine with no driver bound, so none of the `amdgpu` memory
    /// attributes exists. The firmware's boot device still settles it.
    #[test]
    fn a_same_vendor_pair_falls_back_to_the_boot_device() {
        let sysfs = Sysfs::new();
        sysfs.gpu("0000:03:00.0", "1002:7480", "0x030000");
        sysfs
            .gpu("0000:c5:00.0", "1002:15bf", "0x030000")
            .boot_vga();
        let topology = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"));

        assert_eq!(
            topology.dedicated.map(|slot| slot.pci_addr),
            Some("03:00.0".to_string())
        );
        assert_eq!(
            topology.integrated.map(|slot| slot.pci_addr),
            Some("c5:00.0".to_string())
        );
    }

    #[test]
    fn an_intel_pair_is_split_on_the_root_bus() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:00:02.0", "8086:9a49", "0x030000")
            .drm_card(0);
        sysfs
            .gpu("0000:03:00.0", "8086:56a0", "0x030000")
            .drm_card(1);
        let topology = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel"));

        assert_eq!(
            topology.integrated.map(|slot| slot.pci_addr),
            Some("00:02.0".to_string())
        );
        assert_eq!(
            topology.dedicated.map(|slot| slot.pci_addr),
            Some("03:00.0".to_string())
        );
    }

    /// Two discrete cards and no iGPU: filling the integrated slot would make
    /// mxpkgs build an `igpu-launch` wrapper aimed at a discrete card.
    #[test]
    fn two_discrete_cards_leave_the_integrated_slot_empty() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:744c", "0x030000")
            .drm_card(0)
            .vram_total(17_163_091_968)
            .vram_vendor("samsung");
        sysfs
            .gpu("0000:0a:00.0", "1002:7480", "0x030000")
            .drm_card(1)
            .vram_total(8_573_157_376)
            .vram_vendor("hynix");
        let topology = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"));

        assert!(topology.dedicated.is_some());
        assert!(topology.integrated.is_none());
    }

    #[test]
    fn a_machine_without_a_gpu_is_not_an_error() {
        let sysfs = Sysfs::new();
        sysfs.write("bus/pci/devices/0000:00:1f.3/class", "0x040300\n");
        let topology = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel"));

        assert!(topology.dedicated.is_none());
        assert!(topology.integrated.is_none());
    }

    /// A virtio or QXL adapter cannot be written: the `vendor` option is an
    /// enum of three values.
    #[test]
    fn an_unsupported_vendor_fills_no_slot() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:00:01.0", "1af4:1050", "0x030000")
            .drm_card(0);
        let topology = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel"));

        assert!(topology.dedicated.is_none());
        assert!(topology.integrated.is_none());
    }

    #[test]
    fn a_card_with_no_drm_node_has_no_index() {
        let sysfs = Sysfs::new();
        sysfs.gpu("0000:01:00.0", "10de:249d", "0x030200");
        let dedicated = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel"))
            .dedicated
            .expect("dedicated");

        assert_eq!(dedicated.drm_index, None);
        assert_eq!(dedicated.pci_id, "10de:249d");
    }

    /// The card number is the one in `cardN`, not a rank: this machine has no
    /// `card0` at all.
    #[test]
    fn the_drm_index_is_the_card_number_not_a_rank() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:7480", "0x030000")
            .drm_card(7);
        let dedicated = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"))
            .dedicated
            .expect("dedicated");

        assert_eq!(dedicated.drm_index, Some(7));
    }

    /// Without `lspci` every other field must still be detected.
    #[test]
    fn a_missing_lspci_costs_only_the_generation() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:7480", "0x030000")
            .drm_card(1);

        let without = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD"));
        let described = parse_lspci(
            "03:00.0 VGA compatible controller [0300]: AMD Navi 33 [Radeon RX 7700S] [1002:7480]",
        );
        let with = HardwareTopology::detect_from(
            sysfs.root(),
            &sysfs.cpuinfo("AuthenticAMD"),
            Some(&described),
            false,
        )
        .expect("detect");

        let without = without.dedicated.expect("dedicated");
        let with = with.dedicated.expect("dedicated");
        assert_eq!(without.generation, None);
        assert_eq!(with.generation, Some("rdna3".to_string()));
        assert_eq!(without.pci_id, with.pci_id);
        assert_eq!(without.pci_addr, with.pci_addr);
        assert_eq!(without.drm_index, with.drm_index);
    }

    /// With no CPU vendor to lean on, a discrete AMD card next to a discrete
    /// NVIDIA one must not be taken for an iGPU.
    #[test]
    fn a_discrete_amd_card_is_not_integrated_without_a_cpu_vendor() {
        let sysfs = Sysfs::new();
        sysfs
            .gpu("0000:03:00.0", "1002:744c", "0x030000")
            .drm_card(0)
            .vram_total(17_163_091_968)
            .vram_vendor("samsung");
        sysfs
            .gpu("0000:0a:00.0", "10de:2484", "0x030000")
            .drm_card(1);
        let topology = detect(&sysfs, &sysfs.no_cpuinfo());

        assert_eq!(
            topology.dedicated.map(|slot| slot.vendor),
            Some(GpuVendor::Nvidia)
        );
        assert!(topology.integrated.is_none());
    }
}

mod prime {
    use super::*;

    /// Builds a hybrid laptop fixture and returns its bus ids.
    fn bus_ids(igpu: &str, igpu_id: &str, laptop: bool) -> PrimeBusIds {
        let sysfs = Sysfs::new();
        sysfs.chassis(if laptop { "10" } else { "3" });
        sysfs
            .gpu(&format!("0000:{igpu}"), igpu_id, "0x030000")
            .drm_card(0);
        sysfs
            .gpu("0000:01:00.0", "10de:249d", "0x030200")
            .drm_card(1);
        let vendor = if igpu_id.starts_with("8086") {
            "GenuineIntel"
        } else {
            "AuthenticAMD"
        };
        detect(&sysfs, &sysfs.cpuinfo(vendor)).prime_bus_ids()
    }

    #[test]
    fn an_intel_igpu_gets_the_intel_bus_id() {
        let ids = bus_ids("00:02.0", "8086:9a49", true);
        assert_eq!(ids.nvidia.as_deref(), Some("PCI:1:0:0"));
        assert_eq!(ids.intel.as_deref(), Some("PCI:0:2:0"));
        assert_eq!(ids.amd, None);
    }

    #[test]
    fn an_amd_igpu_gets_the_amd_bus_id() {
        let ids = bus_ids("c5:00.0", "1002:15bf", true);
        assert_eq!(ids.nvidia.as_deref(), Some("PCI:1:0:0"));
        assert_eq!(ids.amd.as_deref(), Some("PCI:197:0:0"));
        assert_eq!(ids.intel, None);
    }

    /// PRIME is a laptop feature and the assertion that needs the pair only
    /// bites on a laptop.
    #[test]
    fn a_desktop_gets_no_bus_id() {
        let ids = bus_ids("00:02.0", "8086:9a49", false);
        assert!(ids.nvidia.is_none() && ids.intel.is_none() && ids.amd.is_none());
    }

    /// `nvidiaBusId` alone on a laptop is exactly what the second assertion
    /// forbids.
    #[test]
    fn an_nvidia_only_laptop_gets_no_bus_id() {
        let sysfs = Sysfs::new();
        sysfs.chassis("10");
        sysfs
            .gpu("0000:01:00.0", "10de:249d", "0x030200")
            .drm_card(0);
        let ids = detect(&sysfs, &sysfs.cpuinfo("GenuineIntel")).prime_bus_ids();

        assert!(ids.nvidia.is_none() && ids.intel.is_none() && ids.amd.is_none());
    }

    #[test]
    fn a_non_nvidia_dedicated_card_gets_no_bus_id() {
        let sysfs = Sysfs::new();
        sysfs.chassis("10");
        sysfs
            .gpu("0000:03:00.0", "1002:7480", "0x030000")
            .vram_total(8_573_157_376)
            .vram_vendor("samsung");
        sysfs
            .gpu("0000:c5:00.0", "1002:15bf", "0x030000")
            .vram_total(536_870_912);
        let ids = detect(&sysfs, &sysfs.cpuinfo("AuthenticAMD")).prime_bus_ids();

        assert!(ids.nvidia.is_none() && ids.intel.is_none() && ids.amd.is_none());
    }

    /// The two invariants the mxpkgs assertions rest on, over every case
    /// above.
    #[test]
    fn the_assertion_invariants_hold() {
        let cases = [
            bus_ids("00:02.0", "8086:9a49", true),
            bus_ids("c5:00.0", "1002:15bf", true),
            bus_ids("00:02.0", "8086:9a49", false),
            PrimeBusIds::default(),
        ];
        for ids in cases {
            assert!(
                !(ids.intel.is_some() && ids.amd.is_some()),
                "intelBusId and amdBusId cannot both be set"
            );
            assert!(
                ids.nvidia.is_none() || ids.intel.is_some() || ids.amd.is_some(),
                "nvidiaBusId requires intelBusId or amdBusId"
            );
        }
    }
}

mod laptop {
    use super::*;

    #[test]
    fn dmi_decides_when_it_says_anything() {
        for (chassis, expected) in [
            ("8", true),
            ("9", true),
            ("10", true),
            ("31", true),
            ("3", false),
            ("13", false),
            ("23", false),
        ] {
            let sysfs = Sysfs::new();
            sysfs.chassis(chassis);
            assert_eq!(
                detect(&sysfs, &sysfs.no_cpuinfo()).laptop,
                expected,
                "chassis_type {chassis}"
            );
        }
    }

    #[test]
    fn a_system_battery_is_the_fallback_when_dmi_is_absent() {
        let sysfs = Sysfs::new();
        sysfs.power_supply("BAT0", "Battery", None);
        assert!(detect(&sysfs, &sysfs.no_cpuinfo()).laptop);
    }

    /// A wireless mouse's battery lives in the same directory and must not
    /// turn a desktop into a laptop.
    #[test]
    fn a_peripheral_battery_is_not_a_laptop() {
        let sysfs = Sysfs::new();
        sysfs.power_supply("hidpp_battery_0", "Battery", Some("Device"));
        assert!(!detect(&sysfs, &sysfs.no_cpuinfo()).laptop);
    }

    #[test]
    fn an_uninformative_chassis_type_falls_through_to_the_battery() {
        let sysfs = Sysfs::new();
        sysfs.chassis("2");
        sysfs.power_supply("BAT0", "Battery", None);
        assert!(detect(&sysfs, &sysfs.no_cpuinfo()).laptop);
    }

    #[test]
    fn a_machine_with_neither_dmi_nor_a_battery_is_not_a_laptop() {
        let sysfs = Sysfs::new();
        assert!(!detect(&sysfs, &sysfs.no_cpuinfo()).laptop);
    }
}
