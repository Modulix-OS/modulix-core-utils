use serde_json;
use std::process::Command;

use crate::mx;

#[derive(Debug)]
pub struct HardwareModule {
    module_computer: Vec<String>,
    module_common: Vec<String>,
}

impl HardwareModule {
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

    pub fn get_computer_module(&self) -> &[String] {
        &self.module_computer
    }

    pub fn get_common_module(&self) -> &[String] {
        &self.module_common
    }
}
