//! The two configuration files written from what the machine actually is:
//! `hardware-configuration.nix`, which is the `nixos-generate-config` output
//! plus the `nixos-hardware` modules [`crate::detect_hardware`] selects, and
//! `hardware.nix`, which carries the `mx.hardware.*` options mxpkgs drives the
//! graphics stack from.

use std::process;

use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        param::NixParam,
        transaction::{
            self, TransactionOp,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    detect_hardware::{
        driver_config::DriverConfig,
        system_info::gpu_info::{GpuSlot, HardwareTopology},
    },
    error::io_message,
    mx,
};

/// Configuration file, relative to the config directory, holding the generated
/// hardware configuration.
pub const HARDWARE_CONFIG_PATH: &str = "hardware-configuration.nix";

/// Regenerates `hardware-configuration.nix` in an already-open file.
///
/// # Parameters
/// * `root_path` - root the detection runs against; anything other than `/` is
///   passed as `--root`, which is how the installer targets the system being
///   installed rather than the live one.
/// * `hardware_file` - the open configuration file, whose whole content is
///   replaced.
///
/// # Post-conditions
/// The file becomes `nixos-generate-config --show-hardware-config
/// --no-filesystems` (mount points stay in `fstab.nix`, see
/// [`crate::filesystem`]), takes `nixos-hardware` as a module parameter, and
/// imports one `nixos-hardware.nixosModules.*` per driver module detected.
/// Anything the file held before is discarded.
///
/// # Pre-conditions
/// `nixos-generate-config` must be on `PATH`, and the hardware probes
/// [`crate::detect_hardware`] runs need `pciutils`/`usbutils`/`cpuid`.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] if the generator cannot be spawned
/// (names it) or exits non-zero (carries its stderr),
/// [`mx::ErrorKind::InvalidFile`] if its output is not UTF-8, plus any error
/// from hardware detection.
pub fn write_hardware_config_no_transaction(
    root_path: &str,
    hardware_file: &mut NixFile,
) -> mx::Result<()> {
    let hardware_output = {
        let mut cmd = process::Command::new("nixos-generate-config");
        cmd.args(["--show-hardware-config", "--no-filesystems"]);
        if root_path != "/" {
            cmd.args(["--root", root_path]);
        }
        cmd.output().map_err(|e| {
            mx::ErrorKind::NixCommandError(format!("nixos-generate-config: {}", io_message(&e)))
        })?
    };
    if !hardware_output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(format!(
            "nixos-generate-config exited with {}: {}",
            hardware_output.status,
            String::from_utf8_lossy(&hardware_output.stderr)
        )));
    }

    let hardware_no_fs =
        String::from_utf8(hardware_output.stdout).map_err(|_| mx::ErrorKind::InvalidFile)?;

    let file_content = hardware_file.get_mut_file_content()?;
    *file_content = hardware_no_fs;

    let config = DriverConfig::new()?;

    let param = NixParam::new();
    param.add(hardware_file, "nixos-hardware")?;

    let imports = mxList::new("imports", true);

    for import in config.get_module() {
        imports.add(
            hardware_file,
            &format!("nixos-hardware.nixosModules.{}", &import),
        )?;
    }
    Ok(())
}

/// Regenerates `hardware-configuration.nix` and rebuilds the system.
///
/// # Parameters
/// * `root_path` - root the detection runs against, as in
///   [`write_hardware_config_no_transaction`].
/// * `config_dir` - configuration repository to edit.
///
/// # Post-conditions
/// On success the regenerated hardware configuration is part of the active
/// generation; on error the previous file is restored by the rollback. Blocks
/// for the whole `nixos-rebuild switch`.
pub fn write_hardware_config(root_path: &str, config_dir: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Reset hardware config",
        config_dir,
        HARDWARE_CONFIG_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| write_hardware_config_no_transaction(root_path, file),
    )
}

/// Configuration file, relative to the config directory, holding the
/// `mx.hardware.*` options the detection fills in.
///
/// Public because [`crate::init`] has to name it in `configuration.nix`'s
/// `imports` and in its sealing list, like `crate::locale::LOCALE_FILE_PATH`.
pub const HARDWARE_OPTION_PATH: &str = "hardware.nix";

/// Option paths below which this module owns every declaration.
const SLOT_DEDICATED: &str = "mx.hardware.gpu.dedicated";

/// Option path of the integrated GPU slot, counterpart of
/// [`SLOT_DEDICATED`].
const SLOT_INTEGRATED: &str = "mx.hardware.gpu.integrated";

/// Quotes a value as a Nix string literal.
///
/// # Parameters
/// * `value` - the raw text; inserted as-is, so it must need no Nix escape.
///   Every value this module writes is a vendor name, a hexadecimal id or a
///   PCI address, none of which can.
///
/// # Returns
/// `"<value>"`, ready for [`mxOption::set`].
fn nix_string(value: &str) -> String {
    format!("\"{}\"", value)
}

/// Writes an option, or removes its declaration when there is no value.
///
/// # Parameters
/// * `nix_file` - the open configuration file to edit.
/// * `path` - dotted option path.
/// * `value` - the Nix expression to assign, written verbatim, or `None`.
///
/// # Post-conditions
/// A `None` is expressed by **absence**, never by the literal `null`: `null`
/// is already the default of every `nullOr` option this module writes, so
/// removing the declaration and writing `null` mean the same thing to Nix
/// while only the former keeps the generated file to what was actually
/// detected.
///
/// # Errors
/// Any error from [`mxOption::set`] or
/// [`mxOption::set_option_to_default`].
fn set_or_clear(nix_file: &mut NixFile, path: &str, value: Option<&str>) -> mx::Result<()> {
    match value {
        Some(value) => {
            mxOption::new(path).set(nix_file, value)?;
        }
        None => {
            mxOption::new(path).set_option_to_default(nix_file)?;
        }
    }
    Ok(())
}

/// Writes one GPU slot, or removes it entirely.
///
/// # Parameters
/// * `nix_file` - the open configuration file to edit.
/// * `root` - the slot's option path, [`SLOT_DEDICATED`] or
///   [`SLOT_INTEGRATED`].
/// * `slot` - the detected GPU, `None` when the slot is empty.
///
/// # Post-conditions
/// An empty slot has its whole attribute set removed rather than its five
/// leaves cleared one by one, which would leave an `integrated = { };` husk
/// behind. That is what makes a re-detection after a card is pulled out erase
/// the card instead of leaving stale identifiers pointing at hardware that is
/// gone. `computing` and `launcher` are never written: both are `readOnly` and
/// derived by mxpkgs from the values written here.
///
/// # Errors
/// Any error from [`set_or_clear`] or
/// [`mxOption::set_option_to_default`].
fn write_gpu_slot(nix_file: &mut NixFile, root: &str, slot: Option<&GpuSlot>) -> mx::Result<()> {
    let Some(slot) = slot else {
        mxOption::new(root).set_option_to_default(nix_file)?;
        return Ok(());
    };

    let vendor = nix_string(slot.vendor.as_str());
    let generation = slot.generation.as_deref().map(nix_string);
    let pci_id = nix_string(&slot.pci_id);
    let pci_addr = nix_string(&slot.pci_addr);
    let drm_index = slot.drm_index.map(|index| index.to_string());

    set_or_clear(nix_file, &format!("{}.vendor", root), Some(&vendor))?;
    set_or_clear(
        nix_file,
        &format!("{}.generation", root),
        generation.as_deref(),
    )?;
    set_or_clear(nix_file, &format!("{}.pciId", root), Some(&pci_id))?;
    set_or_clear(nix_file, &format!("{}.pciAddr", root), Some(&pci_addr))?;
    set_or_clear(
        nix_file,
        &format!("{}.drmIndex", root),
        drm_index.as_deref(),
    )?;
    Ok(())
}

/// Writes the detected `mx.hardware.*` options into an already-open file.
///
/// # Parameters
/// * `nix_file` - the open configuration file to edit, in memory only.
/// * `topology` - what the detection found, as
///   [`HardwareTopology::detect`] returns it.
///
/// # Post-conditions
/// Writes `mx.hardware.laptop`, `mx.hardware.has_fingerprint`, the five
/// options of each GPU slot, and the three `mx.hardware.gpu.nvidia.*BusId`
/// values - and nothing else. Every one of those paths is assigned or removed
/// on every call, which makes the function idempotent and makes a stale value
/// impossible: running it twice over its own output changes nothing, and a
/// slot or a bus id that no longer applies is removed rather than left behind.
/// That is what lets a daemon re-run it after a hardware change, and what lets
/// a re-run that finds nothing new produce no commit at all, since
/// `Transaction::commit` only stages files that genuinely differ.
///
/// Three groups of options are deliberately left alone, so a user's own
/// choices and mxpkgs' own defaults survive a re-detection:
/// `mx.hardware.gpu.nvidia.disable` and `.experimental-power-management`
/// (decisions no probe can make), `mx.hardware.gpu.acceleration.enable` and
/// `.compute.enable` (whose defaults are deliberate and app-driven), and the
/// `readOnly` `computing`/`launcher` options. For that reason neither
/// `mx.hardware.gpu.nvidia` nor `mx.hardware.gpu` is ever removed as a whole -
/// each parents options this module does not own - only their individual
/// leaves are.
///
/// `laptop` and `has_fingerprint` are written even when `false`, rather than
/// removed: they are detected facts, and an explicit `false` says "detection
/// ran and found none" where an absent option would also mean "detection never
/// ran".
///
/// # Errors
/// Any error from assigning or removing one of those options, i.e. from
/// `crate::core::option::Option`'s `set`/`set_option_to_default`.
pub fn write_hardware_options_no_transaction(
    nix_file: &mut NixFile,
    topology: &HardwareTopology,
) -> mx::Result<()> {
    mxOption::new("mx.hardware.laptop").set(nix_file, bool_nix(topology.laptop))?;
    mxOption::new("mx.hardware.has_fingerprint")
        .set(nix_file, bool_nix(topology.has_fingerprint))?;

    write_gpu_slot(nix_file, SLOT_DEDICATED, topology.dedicated.as_ref())?;
    write_gpu_slot(nix_file, SLOT_INTEGRATED, topology.integrated.as_ref())?;

    let prime = topology.prime_bus_ids();
    for (option, bus_id) in [
        ("nvidiaBusId", &prime.nvidia),
        ("intelBusId", &prime.intel),
        ("amdBusId", &prime.amd),
    ] {
        let value = bus_id.as_deref().map(nix_string);
        set_or_clear(
            nix_file,
            &format!("mx.hardware.gpu.nvidia.{}", option),
            value.as_deref(),
        )?;
    }

    Ok(())
}

/// Renders a boolean as its Nix literal.
///
/// # Parameters
/// * `value` - the boolean.
///
/// # Returns
/// `"true"` or `"false"`.
fn bool_nix(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// Writes the detected `mx.hardware.*` options and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `topology` - what the detection found. Taken as a parameter rather than
///   probed here so a caller that has already probed - [`crate::init`] does -
///   does not probe twice, and so a caller can supply a topology of its own.
///
/// # Post-conditions
/// On success the options are part of the active generation; on error the
/// configuration is rolled back. Blocks for the whole `nixos-rebuild switch`.
/// A topology identical to what the file already declares produces no commit
/// and therefore no rebuild.
///
/// # Errors
/// Any error from the transaction or from
/// [`write_hardware_options_no_transaction`].
pub fn write_hardware_options(config_dir: &str, topology: &HardwareTopology) -> mx::Result<()> {
    transaction::make_transaction(
        "Update detected hardware options",
        config_dir,
        HARDWARE_OPTION_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| write_hardware_options_no_transaction(file, topology),
    )
}

/// Re-probes the machine and applies what it finds.
///
/// The entry point for a caller that reacts to a hardware change - a GPU
/// added, removed or replaced - and wants the configuration to match the
/// machine again.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
///
/// # Pre-conditions
/// Probes the **live** machine, never an installation root. `lspci` on `PATH`
/// is what names each GPU's generation; without it every `generation` is left
/// unset, which is harmless for AMD and Intel but hands a Fermi-to-Volta
/// NVIDIA card the current driver branch instead of its legacy one.
///
/// # Post-conditions
/// As in [`write_hardware_options`].
///
/// # Errors
/// Any error from [`HardwareTopology::detect`] or
/// [`write_hardware_options`].
pub fn redetect_hardware_options(config_dir: &str) -> mx::Result<()> {
    write_hardware_options(config_dir, &HardwareTopology::detect()?)
}

/// Re-runs `nixos-generate-config` against the live system and applies the
/// result.
///
/// The `hardware-configuration.nix` counterpart of
/// [`redetect_hardware_options`]: it regenerates the generator's own output
/// only, leaving the `mx.hardware.*` options alone. Use
/// [`redetect_all_hardware`] to refresh both in one commit.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
///
/// # Pre-conditions
/// Runs against the live system (`/`), never an installation root, and needs
/// `nixos-generate-config` on `PATH`.
///
/// # Post-conditions
/// As in [`write_hardware_config`].
///
/// # Errors
/// Any error from [`write_hardware_config`].
pub fn redetect_hardware_config(config_dir: &str) -> mx::Result<()> {
    write_hardware_config("/", config_dir)
}

/// Re-probes the machine and refreshes **both** hardware files in a single
/// commit and a single rebuild.
///
/// What [`redetect_hardware_config`] and [`redetect_hardware_options`] do
/// separately - two commits and two `nixos-rebuild switch` for one hardware
/// change - grouped into one transaction, through
/// [`transaction::make_group_transaction`]: one operation regenerates
/// `hardware-configuration.nix`, the other writes the `mx.hardware.*` options
/// from the same probe.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
///
/// # Pre-conditions
/// Probes the **live** machine, never an installation root, and needs
/// `nixos-generate-config` on `PATH`. `lspci` is what names each GPU's
/// generation; without it every `generation` is left unset, which is harmless
/// for AMD and Intel but hands a Fermi-to-Volta NVIDIA card the current driver
/// branch instead of its legacy one. The machine is probed once, before the
/// transaction opens, so a detection failure leaves the configuration
/// untouched.
///
/// # Post-conditions
/// On success both files are part of the active generation; on any error the
/// configuration is back to its previous commit and neither file is left
/// half-written. A machine whose topology and generated configuration already
/// match what the files declare produces no commit and therefore no rebuild.
/// Blocks for the whole `nixos-rebuild switch`.
///
/// # Errors
/// Any error from [`HardwareTopology::detect`],
/// [`write_hardware_config_no_transaction`],
/// [`write_hardware_options_no_transaction`] or
/// [`transaction::make_group_transaction`].
pub fn redetect_all_hardware(config_dir: &str) -> mx::Result<()> {
    let topology = HardwareTopology::detect()?;

    transaction::make_group_transaction(
        config_dir,
        BuildCommand::Switch,
        UpdateInput::Keep,
        vec![
            TransactionOp::new("Reset hardware config", HARDWARE_CONFIG_PATH, |file| {
                write_hardware_config_no_transaction("/", file)
            }),
            TransactionOp::new(
                "Update detected hardware options",
                HARDWARE_OPTION_PATH,
                move |file| write_hardware_options_no_transaction(file, &topology),
            ),
        ],
    )
}

#[cfg(test)]
#[path = "hardware_config_tests.rs"]
mod tests;
