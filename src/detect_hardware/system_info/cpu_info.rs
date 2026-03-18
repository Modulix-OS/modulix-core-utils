use regex::Regex;
use serde::{Deserialize, Serialize};
use std::process::Command;

use crate::mx;

#[derive(Serialize, Deserialize, Debug)]
pub struct CpuInfo {
    constructor: String,
    codename: String,
}
impl CpuInfo {
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

    fn cpu_constructor(cpu_info: &str) -> mx::Result<String> {
        let pattern_constructor = Regex::new(r"AMD|Intel").unwrap();
        Ok(pattern_constructor
            .find(cpu_info)
            .ok_or(mx::ErrorKind::UnknowCPUConstructor)?
            .as_str()
            .to_lowercase())
    }

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

    pub fn new() -> mx::Result<CpuInfo> {
        let cpu_info = Self::cpu_info()?;
        let constructor = Self::cpu_constructor(&cpu_info);
        let codename = Self::cpu_codename(&cpu_info);
        Ok(CpuInfo {
            constructor: constructor?,
            codename: codename?,
        })
    }

    #[allow(dead_code)]
    pub fn get_constructor(&self) -> &str {
        return &self.constructor;
    }

    #[allow(dead_code)]
    pub fn get_codename(&self) -> &str {
        return &self.codename;
    }
}
