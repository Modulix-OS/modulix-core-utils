//! Tests for the `hardware.nix` writer in [`super`].
//!
//! Each case runs the writer against a real file inside a real writable
//! transaction over a real git repository, then rolls the transaction back and
//! inspects the buffer - the same harness shape `crate::filesystem`'s tests
//! use, since the options are spliced into Nix source and only the rendered
//! text proves it was done right.

use std::fs;

use tempfile::TempDir;

use super::{FILE_HARDWARE_PATH, write_hardware_options_no_transaction};
use crate::core::option::Option as mxOption;
use crate::core::transaction::Transaction;
use crate::core::transaction::file_lock::NixFile;
use crate::core::transaction::transaction::{BuildCommand, TransactionPermission};
use crate::detect_hardware::system_info::gpu_info::{GpuSlot, GpuVendor, HardwareTopology};

/// The stub `NixFile::create_file` writes, i.e. what the writer starts from on
/// a fresh configuration.
const EMPTY: &str = "{config, lib, pkgs, ...}:\n{\n}\n";

/// Runs `edit` against a real `hardware.nix` seeded with `seed`, inside a
/// writable transaction that is rolled back, and returns the resulting buffer.
fn with_hardware(seed: &str, edit: impl FnOnce(&mut NixFile)) -> String {
    let dir = TempDir::new().expect("failed to create temporary directory");
    git2::Repository::init(dir.path()).expect("git init failed");
    fs::write(dir.path().join(FILE_HARDWARE_PATH), seed).expect("failed to write hardware.nix");
    fs::write(
        dir.path().join("configuration.nix"),
        "{config, lib, pkgs, ...}:\n{\n  imports = [];\n}\n",
    )
    .expect("failed to write configuration.nix");

    let repo_path = format!("{}/", dir.path().to_str().unwrap());
    let mut tx = Transaction::new(
        &repo_path,
        "test",
        BuildCommand::Boot,
        TransactionPermission::Writtable,
    )
    .expect("Transaction::new failed");
    tx.add_file(FILE_HARDWARE_PATH).expect("add_file failed");
    tx.begin().expect("begin failed");

    let file = tx.get_file_mut(FILE_HARDWARE_PATH).expect("get_file_mut");
    edit(file);
    let content = file
        .get_file_content()
        .expect("get_file_content failed")
        .clone();
    tx.rollback().expect("rollback failed");
    content
}

/// Renders `topology` into a file seeded with `seed`.
fn render(seed: &str, topology: &HardwareTopology) -> String {
    with_hardware(seed, |file| {
        write_hardware_options_no_transaction(file, topology).expect("write failed");
    })
}

/// A slot, spelled out so each case only names what it varies.
fn slot(
    vendor: GpuVendor,
    generation: Option<&str>,
    pci_id: &str,
    pci_addr: &str,
    drm_index: Option<u32>,
) -> GpuSlot {
    GpuSlot {
        vendor,
        generation: generation.map(str::to_string),
        pci_id: pci_id.to_string(),
        pci_addr: pci_addr.to_string(),
        drm_index,
    }
}

/// The hybrid AMD machine the detection was written against: an APU iGPU next
/// to a discrete Radeon, on a laptop.
fn hybrid_amd() -> HardwareTopology {
    HardwareTopology {
        dedicated: Some(slot(
            GpuVendor::Amd,
            Some("rdna3"),
            "1002:7480",
            "03:00.0",
            Some(1),
        )),
        integrated: Some(slot(
            GpuVendor::Amd,
            Some("rdna3"),
            "1002:15bf",
            "c5:00.0",
            Some(2),
        )),
        laptop: true,
        has_fingerprint: true,
        chipset_names: true,
    }
}

/// A laptop with an Intel iGPU and a discrete NVIDIA card, the case the PRIME
/// bus ids exist for.
fn hybrid_nvidia() -> HardwareTopology {
    HardwareTopology {
        dedicated: Some(slot(
            GpuVendor::Nvidia,
            Some("ampere"),
            "10de:249d",
            "01:00.0",
            Some(0),
        )),
        integrated: Some(slot(
            GpuVendor::Intel,
            Some("tigerlake"),
            "8086:9a49",
            "00:02.0",
            Some(1),
        )),
        laptop: true,
        has_fingerprint: false,
        chipset_names: true,
    }
}

#[test]
fn every_detected_value_is_declared() {
    let content = render(EMPTY, &hybrid_amd());

    for expected in [
        "laptop = true;",
        "has_fingerprint = true;",
        "vendor = \"amd\";",
        "generation = \"rdna3\";",
        "pciId = \"1002:7480\";",
        "pciAddr = \"03:00.0\";",
        "drmIndex = 1;",
        "pciId = \"1002:15bf\";",
        "pciAddr = \"c5:00.0\";",
        "drmIndex = 2;",
    ] {
        assert!(
            content.contains(expected),
            "missing {expected} in:\n{content}"
        );
    }
}

/// `drmIndex` is `types.int`: quoting it would make the module system reject
/// the configuration.
#[test]
fn the_drm_index_is_written_unquoted() {
    let content = render(EMPTY, &hybrid_amd());
    assert!(content.contains("drmIndex = 1;"));
    assert!(!content.contains("drmIndex = \""));
}

/// Every value must be readable back through the same locator that wrote it,
/// which is how a malformed nesting would show up - the tests have no Nix
/// evaluator.
#[test]
fn every_written_option_reads_back() {
    with_hardware(EMPTY, |file| {
        write_hardware_options_no_transaction(file, &hybrid_nvidia()).expect("write failed");
        for (path, expected) in [
            ("mx.hardware.laptop", "true"),
            ("mx.hardware.has_fingerprint", "false"),
            ("mx.hardware.gpu.dedicated.vendor", "\"nvidia\""),
            ("mx.hardware.gpu.dedicated.generation", "\"ampere\""),
            ("mx.hardware.gpu.dedicated.pciId", "\"10de:249d\""),
            ("mx.hardware.gpu.dedicated.pciAddr", "\"01:00.0\""),
            ("mx.hardware.gpu.dedicated.drmIndex", "0"),
            ("mx.hardware.gpu.integrated.vendor", "\"intel\""),
            ("mx.hardware.gpu.integrated.drmIndex", "1"),
            ("mx.hardware.gpu.nvidia.nvidiaBusId", "\"PCI:1:0:0\""),
            ("mx.hardware.gpu.nvidia.intelBusId", "\"PCI:0:2:0\""),
        ] {
            assert_eq!(
                mxOption::new(path).get(file).ok(),
                Some(expected),
                "reading back {path}"
            );
        }
    });
}

/// The daemon re-runs this after every hardware change, so a second pass over
/// the first pass's output has to be a no-op - otherwise each run would commit
/// and rebuild.
#[test]
fn writing_twice_changes_nothing() {
    let topology = hybrid_amd();
    let once = render(EMPTY, &topology);
    let twice = render(&once, &topology);
    assert_eq!(once, twice);
}

/// A card that is pulled out must take its identifiers with it: a stale
/// `pciAddr` would send `DRI_PRIME` at hardware that is gone.
#[test]
fn an_emptied_slot_is_removed_whole() {
    let seed = render(EMPTY, &hybrid_amd());
    assert!(seed.contains("c5:00.0"));

    let single = HardwareTopology {
        dedicated: Some(slot(
            GpuVendor::Amd,
            Some("rdna3"),
            "1002:7480",
            "03:00.0",
            Some(0),
        )),
        integrated: None,
        laptop: false,
        has_fingerprint: false,
        chipset_names: true,
    };
    let content = render(&seed, &single);

    assert!(
        !content.contains("integrated"),
        "left behind in:\n{content}"
    );
    assert!(!content.contains("c5:00.0"));
    assert!(content.contains("pciAddr = \"03:00.0\";"));
    assert!(content.contains("laptop = false;"));
}

/// A `generation` that stops being recognised must not leave the previous one
/// in place.
#[test]
fn an_unknown_generation_removes_the_previous_one() {
    let seed = render(EMPTY, &hybrid_amd());
    assert!(seed.contains("generation = \"rdna3\";"));

    let unnamed = HardwareTopology {
        dedicated: Some(slot(GpuVendor::Amd, None, "1002:7480", "03:00.0", Some(1))),
        integrated: None,
        ..HardwareTopology::default()
    };
    let content = render(&seed, &unnamed);

    assert!(
        !content.contains("generation"),
        "left behind in:\n{content}"
    );
    assert!(content.contains("vendor = \"amd\";"));
}

/// `disable` and `experimental-power-management` are the user's: no probe can
/// decide them, so clearing the bus ids must not take them along.
#[test]
fn the_users_own_nvidia_options_survive() {
    let seed = "\
{config, lib, pkgs, ...}:
{
  mx.hardware.gpu.nvidia = {
    disable = true;
    experimental-power-management = true;
    nvidiaBusId = \"PCI:9:9:9\";
    intelBusId = \"PCI:0:2:0\";
  };
}
";
    let single = HardwareTopology {
        dedicated: Some(slot(GpuVendor::Amd, None, "1002:744c", "03:00.0", Some(0))),
        ..HardwareTopology::default()
    };
    let content = render(seed, &single);

    assert!(content.contains("disable = true;"), "in:\n{content}");
    assert!(content.contains("experimental-power-management = true;"));
    assert!(!content.contains("BusId"), "stale bus id in:\n{content}");
}

/// Writing these would freeze a default that is deliberately dynamic, or
/// assign a `readOnly` option.
#[test]
fn the_derived_and_user_owned_options_are_never_written() {
    for topology in [hybrid_amd(), hybrid_nvidia(), HardwareTopology::default()] {
        let content = render(EMPTY, &topology);
        for forbidden in [
            "computing",
            "launcher",
            "acceleration",
            "compute",
            "experimental-power-management",
            "disable",
        ] {
            assert!(
                !content.contains(forbidden),
                "{forbidden} written in:\n{content}"
            );
        }
    }
}

/// PRIME is a laptop feature, and `nvidiaBusId` without a counterpart is what
/// mxpkgs asserts against.
#[test]
fn prime_bus_ids_are_written_only_for_a_hybrid_laptop() {
    let laptop = render(EMPTY, &hybrid_nvidia());
    assert!(laptop.contains("nvidiaBusId = \"PCI:1:0:0\";"));
    assert!(laptop.contains("intelBusId = \"PCI:0:2:0\";"));
    assert!(!laptop.contains("amdBusId"));

    let desktop = HardwareTopology {
        laptop: false,
        ..hybrid_nvidia()
    };
    let content = render(EMPTY, &desktop);
    assert!(!content.contains("BusId"), "in:\n{content}");
}

/// A headless machine is a legitimate outcome, and must still record that
/// detection ran.
#[test]
fn a_machine_without_a_gpu_declares_only_the_two_facts() {
    let content = render(EMPTY, &HardwareTopology::default());

    assert!(content.contains("laptop = false;"));
    assert!(content.contains("has_fingerprint = false;"));
    assert!(!content.contains("dedicated"));
    assert!(!content.contains("integrated"));
    assert!(!content.contains("BusId"));
}
