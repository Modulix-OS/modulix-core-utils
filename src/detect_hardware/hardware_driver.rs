//! The catalogue of `nixos-hardware` modules, split into the per-machine ones
//! and the `common-*` ones.

use serde_json;
use std::process::Command;

use crate::mx;

/// Every module name `nixos-hardware` exposes, sorted into two groups.
///
/// # Fields
/// * `module_computer` - modules named after a machine (`lenovo-thinkpad-x1`,
///   …), matched against the detected vendor and model.
/// * `module_common` - the `common-*` modules (`common-cpu-intel`,
///   `common-gpu-amd`, …), matched against the detected CPU and GPU.
#[derive(Debug)]
pub struct HardwareModule {
    module_computer: Vec<String>,
    module_common: Vec<String>,
}

impl HardwareModule {
    /// Lists the attribute names of `nixos-hardware`'s `nixosModules`.
    ///
    /// # Returns
    /// One name per module, as the flake exposes them.
    ///
    /// # Pre-conditions
    /// `nix` must be on `PATH` with flakes enabled, and the machine must be able
    /// to reach `github:NixOS/nixos-hardware` - the flake is evaluated from
    /// GitHub, not from the configuration's own input, and no lock file is
    /// written.
    ///
    /// # Errors
    /// [`mx::ErrorKind::RequestSenderError`] if `nix` cannot be run, exits
    /// non-zero (payload is its stderr), or prints something that does not
    /// deserialise.
    fn list_module_names() -> mx::Result<Vec<String>> {
        let output = Command::new("nix")
            .args([
                "eval",
                "--json",
                "--no-write-lock-file",
                "github:NixOS/nixos-hardware#nixosModules",
                "--apply",
                "builtins.attrNames",
            ])
            .output()
            .map_err(|e| {
                mx::ErrorKind::RequestSenderError(format!("Failed to run `nix`: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(mx::ErrorKind::RequestSenderError(format!(
                "`nix eval` failed: {}",
                stderr
            )));
        }

        serde_json::from_slice(&output.stdout).map_err(|e| {
            mx::ErrorKind::RequestSenderError(format!("Failed to parse JSON output: {}", e))
        })
    }

    /// Fetches the module catalogue and splits it in two.
    ///
    /// # Returns
    /// The catalogue, with names starting with `common-` in `module_common` and
    /// all the others in `module_computer`.
    ///
    /// # Errors
    /// As in [`HardwareModule::list_module_names`].
    pub fn new() -> mx::Result<HardwareModule> {
        let names = Self::list_module_names()?;

        let mut module_computer = Vec::with_capacity(100);
        let mut module_common = Vec::with_capacity(20);

        for name in names {
            if name.starts_with("common-") {
                module_common.push(name);
            } else {
                module_computer.push(name);
            }
        }

        Ok(HardwareModule {
            module_computer,
            module_common,
        })
    }

    /// The machine-specific module names.
    ///
    /// # Returns
    /// The names that are not `common-*`, borrowed from `self`, in the order
    /// `nixos-hardware` listed them.
    pub fn get_computer_module(&self) -> &[String] {
        &self.module_computer
    }

    /// The `common-*` module names.
    ///
    /// # Returns
    /// The names starting with `common-`, borrowed from `self`.
    pub fn get_common_module(&self) -> &[String] {
        &self.module_common
    }
}
