//! Enables and disables Modulix modules under `modulix.modules.*` in the
//! configuration's `modules.nix`.
//!
//! This is the raw `enable` toggle on a single module path. The higher-level
//! [`crate::install_module`] is what front-ends call: it resolves
//! meta-modules and drives the plugin lists too.

use crate::{
    core::{
        option::Option as mxOption,
        transaction::{
            self,
            file_lock::NixFile,
            transaction::{BuildCommand, UpdateInput},
        },
    },
    mx,
};

/// Configuration file, relative to the config directory, holding the
/// `modulix.modules` tree.
const FILE_MODULE_PATH: &str = "modules.nix";

/// Enables one module in an already-open `modules.nix`.
///
/// # Parameters
/// * `nix_file` - the open configuration file to edit.
/// * `module_path` - dotted module path below `modulix.modules` (e.g.
///   `programs.git`), whose `enable` is set to `true`.
///
/// # Post-conditions
/// Setting an already-enabled module is a no-op. No check is made that the
/// path names a module that exists.
pub fn add_module_no_transaction(nix_file: &mut NixFile, module_path: &str) -> mx::Result<()> {
    mxOption::new(&format!("modulix.modules.{}.enable", module_path)).set(nix_file, "true")?;
    Ok(())
}

/// Disables one module in an already-open `modules.nix`.
///
/// # Parameters
/// * `nix_file` - the open configuration file to edit.
/// * `module_path` - dotted module path below `modulix.modules`.
///
/// # Post-conditions
/// The `enable` declaration is deleted rather than set to `false`, so the
/// module falls back to its own default. Only the first declaration goes, and a
/// module that was not enabled is not an error.
pub fn remove_module_no_transaction(nix_file: &mut NixFile, module_path: &str) -> mx::Result<()> {
    mxOption::new(&format!("modulix.modules.{}.enable", module_path))
        .set_option_to_default(nix_file)?;
    Ok(())
}

/// Enables one module and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `module_path` - dotted module path below `modulix.modules`.
///
/// # Post-conditions
/// On success the module is part of the active generation; on error the
/// configuration is rolled back. Blocks for the whole `nixos-rebuild switch`.
pub fn add_modules(config_dir: &str, module_path: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add module {}", module_path),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| add_module_no_transaction(file, module_path),
    )
}

/// Disables one module and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `module_path` - dotted module path below `modulix.modules`.
///
/// # Post-conditions
/// As in [`add_modules`], with the module gone from the active generation on
/// success. The commit message reads `Add module …`, a wording slip that has no
/// effect on what is applied.
pub fn remove_modules(config_dir: &str, module_path: &str) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Add module {}", module_path),
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| remove_module_no_transaction(file, module_path),
    )
}
