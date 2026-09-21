//! CPU identification, from the vendor and microarchitecture codename `cpuid`
//! reports.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::process::Command;

use crate::mx;

/// The CPU of the running machine, as far as driver selection needs it.
///
/// # Fields
/// * `constructor` - vendor, lowercased: `intel` or `amd`.
/// * `codename` - microarchitecture codename, lowercased (e.g. `zen 3`).
#[derive(Serialize, Deserialize, Debug)]
pub struct CpuInfo {
    constructor: String,
    codename: String,
}
impl CpuInfo {
    /// Reads the CPU's synthesised description from `cpuid`.
    ///
    /// # Returns
    /// The text of the last `(synth)` line, without that marker - the line that
    /// names the vendor and the codename.
    ///
    /// # Pre-conditions
    /// `cpuid` must be on `PATH`.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the command cannot be spawned, and
    /// [`mx::ErrorKind::CPUInfoNofFound`] if its output holds no `(synth)` line.
    fn cpu_info() -> mx::Result<String> {
        let output = Command::new("cpuid")
            .output()
            .map_err(mx::ErrorKind::IOError)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(stdout
            .trim()
            .split('\n')
            .rfind(|s| s.trim_start().starts_with("(synth)"))
            .ok_or(mx::ErrorKind::CPUInfoNofFound)?
            .trim_start()
            .strip_prefix("(synth)")
            .unwrap()
            .trim()
            .to_string());
    }

    /// Extracts the vendor from a `cpuid` description.
    ///
    /// # Parameters
    /// * `cpu_info` - the description, as returned by [`CpuInfo::cpu_info`].
    ///
    /// # Returns
    /// `"intel"` or `"amd"`, lowercased.
    ///
    /// # Errors
    /// [`mx::ErrorKind::UnknowCPUConstructor`] for any other vendor: only these
    /// two are recognised.
    fn cpu_constructor(cpu_info: &str) -> mx::Result<String> {
        let pattern_constructor = Regex::new(r"AMD|Intel").unwrap();
        Ok(pattern_constructor
            .find(cpu_info)
            .ok_or(mx::ErrorKind::UnknowCPUConstructor)?
            .as_str()
            .to_lowercase())
    }

    /// Extracts the microarchitecture codename from a `cpuid` description.
    ///
    /// # Parameters
    /// * `cpu_info` - the description, as returned by [`CpuInfo::cpu_info`].
    ///
    /// # Returns
    /// The contents of the first parenthesised group, lowercased.
    ///
    /// # Errors
    /// [`mx::ErrorKind::ErrorParseCPUCodename`] when the description carries no
    /// parenthesised group.
    fn cpu_codename(cpu_info: &str) -> mx::Result<String> {
        let pattern_codename = Regex::new(r"\(.*?\)").unwrap();
        Ok(pattern_codename
            .find(cpu_info)
            .ok_or(mx::ErrorKind::ErrorParseCPUCodename)?
            .as_str()
            .strip_prefix('(')
            .unwrap()
            .strip_suffix(')')
            .unwrap()
            .to_lowercase())
    }

    /// Identifies the CPU of the running machine.
    ///
    /// # Returns
    /// Its vendor and codename.
    ///
    /// # Errors
    /// Any error from [`CpuInfo::cpu_info`], [`CpuInfo::cpu_constructor`] or
    /// [`CpuInfo::cpu_codename`]; an unrecognised vendor is reported before an
    /// unparseable codename.
    pub fn new() -> mx::Result<CpuInfo> {
        let cpu_info = Self::cpu_info()?;
        let constructor = Self::cpu_constructor(&cpu_info);
        let codename = Self::cpu_codename(&cpu_info);
        Ok(CpuInfo {
            constructor: constructor?,
            codename: codename?,
        })
    }

    /// The CPU's vendor.
    ///
    /// # Returns
    /// `"intel"` or `"amd"`, borrowed from `self`.
    #[allow(dead_code)]
    pub fn get_constructor(&self) -> &str {
        return &self.constructor;
    }

    /// The CPU's microarchitecture codename.
    ///
    /// # Returns
    /// The lowercased codename, borrowed from `self`.
    #[allow(dead_code)]
    pub fn get_codename(&self) -> &str {
        return &self.codename;
    }
}
