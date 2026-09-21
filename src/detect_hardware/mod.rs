//! Hardware detection and the `nixos-hardware` modules it implies.
//!
//! `system_info` probes the machine (CPU through `cpuid`, GPU through `lspci`,
//! machine model through DMI), `hardware_driver` lists the modules
//! `nixos-hardware` offers, and [`driver_config`] matches the two to decide what
//! the configuration should import.

pub mod driver_config;
mod hardware_driver;
mod system_info;
