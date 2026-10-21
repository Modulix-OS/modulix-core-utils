//! Prints the hardware topology the detection finds on this machine, without
//! opening a transaction or writing anything.
//!
//! Run it to confront what `mx-init` would seed into `hardware.nix` with what
//! the machine actually is:
//!
//! ```sh
//! cargo run --example hardware_options --features hardware-config
//! ```
//!
//! `lspci` must be on `PATH` for the `generation` fields to be named; without
//! it every other field is still detected (the fingerprint reader is read from
//! `sysfs`, so it needs nothing on `PATH`). The nix devShell provides `lspci`.

use modulix_core_utils::detect_hardware::system_info::gpu_info::HardwareTopology;

/// Probes the machine and prints the result, one line per detected fact.
fn main() {
    let topology = HardwareTopology::detect().expect("hardware detection failed");

    println!("laptop           = {}", topology.laptop);
    println!("has_fingerprint  = {}", topology.has_fingerprint);
    println!("chipset names    = {}", topology.chipset_names);

    for (name, slot) in [
        ("dedicated", &topology.dedicated),
        ("integrated", &topology.integrated),
    ] {
        match slot {
            Some(slot) => println!(
                "{name:<16} = vendor {}, generation {:?}, pciId {}, pciAddr {}, drmIndex {:?}",
                slot.vendor.as_str(),
                slot.generation,
                slot.pci_id,
                slot.pci_addr,
                slot.drm_index,
            ),
            None => println!("{name:<16} = none"),
        }
    }

    println!("prime bus ids    = {:?}", topology.prime_bus_ids());
}
