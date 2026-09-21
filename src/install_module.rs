//! Enable/disable Modulix modules (`mx.<name>.enable`) and their plugins
//! (`mx.<name>.plugins`) in `module.nix`, and list what is currently
//! installed.
//!
//! Each public entry point wraps a `*_no_transaction` edit function in
//! `transaction::make_transaction` (or its read-only counterpart), so a
//! module or plugin change is applied, committed and rebuilt (or rolled
//! back on error) as one atomic step. `module_name` values that name a
//! meta-module (one with `modules` children in the remote index) are
//! expanded to themselves plus every child via
//! [`crate::module_info::resolve_with_children`] before being applied.

use crate::core::app_info_trait::AppInfoMinimal;
use crate::core::transaction::transaction::UpdateInput;
use crate::module_info::ModuleInfo;
use crate::{
    core::{
        list::List as mxList,
        option::Option as mxOption,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
};
use std::path;

/// Path, relative to `config_dir`, of the Nix file every function in this
/// module edits or reads.
const FILE_MODULE_PATH: &str = "module.nix";

/// Dotted option path toggling a module on or off.
///
/// # Parameters
/// * `module_name` - dotted module name (e.g. `programs.games.steam`).
///
/// # Returns
/// `"mx.<module_name>.enable"`.
fn enable_path(module_name: &str) -> String {
    format!("mx.{}.enable", module_name)
}

/// Dotted option path of a module's plugin list.
///
/// # Parameters
/// * `module_name` - dotted module name.
///
/// # Returns
/// `"mx.<module_name>.plugins"`.
fn plugins_path(module_name: &str) -> String {
    format!("mx.{}.plugins", module_name)
}

/// Sets `mx.<module_name>.enable = true;` in `file`, without opening or
/// committing any transaction.
///
/// # Parameters
/// * `file` - the open file to edit, in memory only.
/// * `module_name` - dotted module name.
///
/// # Post-conditions
/// The option is created if absent, or its value overwritten if already
/// declared (see `mxOption::set`).
///
/// # Returns
/// `Ok(())` once the edit is applied to `file`'s in-memory buffer.
///
/// # Errors
/// Whatever `mxOption::set` returns, e.g. [`mx::ErrorKind::PermissionDenied`]
/// if `file` was opened read-only.
pub fn install_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set(file, "true")?;
    Ok(())
}

/// Deletes the `mx.<module_name>.enable` declaration in `file`, without
/// opening or committing any transaction.
///
/// # Parameters
/// * `file` - the open file to edit, in memory only.
/// * `module_name` - dotted module name.
///
/// # Post-conditions
/// The module falls back to its own NixOS default (normally disabled). If
/// the option was not declared, `file` is left untouched — this is not an
/// error (see `mxOption::set_option_to_default`).
///
/// # Returns
/// `Ok(())` whether or not a declaration was actually removed.
///
/// # Errors
/// Whatever `mxOption::set_option_to_default` returns.
pub fn uninstall_no_transaction(file: &mut NixFile, module_name: &str) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set_option_to_default(file)?;
    Ok(())
}

/// Enables `module_name` and appends `pkgs.<plugin_namespace>.<plugin_name>`
/// to its plugin list, without opening or committing any transaction.
///
/// # Parameters
/// * `file` - the open file to edit, in memory only.
/// * `module_name` - dotted module name whose plugin list is edited; also
///   the module that gets enabled.
/// * `plugin_namespace` - nixpkgs namespace the plugin is read from (see
///   [`crate::module_info::ModuleInfo::plugins_namespace`]).
/// * `plugin_name` - attribute name of the plugin inside that namespace.
///
/// # Post-conditions
/// `mx.<module_name>.enable` is set to `true` (installing a plugin always
/// enables its module), then `pkgs.<plugin_namespace>.<plugin_name>` is
/// added to `mx.<module_name>.plugins` (declared as `[]` first if absent).
/// The plugin list is unique-valued, so an already-listed plugin is left
/// as-is (see `mxList::add`).
///
/// # Returns
/// `Ok(())` once both edits are applied to `file`'s in-memory buffer.
///
/// # Errors
/// Whatever `mxOption::set` or `mxList::add` return, e.g.
/// [`mx::ErrorKind::OptionIsNotList`] if `mx.<module_name>.plugins` already
/// holds a non-list value.
pub fn install_plugin_no_transaction(
    file: &mut NixFile,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    mxOption::new(&enable_path(module_name)).set(file, "true")?;
    mxList::new(&plugins_path(module_name), true)
        .add(file, &format!("pkgs.{}.{}", plugin_namespace, plugin_name))?;
    Ok(())
}

/// Removes `pkgs.<plugin_namespace>.<plugin_name>` from `module_name`'s
/// plugin list, without opening or committing any transaction.
///
/// # Parameters
/// * `file` - the open file to edit, in memory only.
/// * `module_name` - dotted module name whose plugin list is edited. The
///   module's `enable` option is not touched, so it stays enabled.
/// * `plugin_namespace` - nixpkgs namespace the plugin is read from.
/// * `plugin_name` - attribute name of the plugin inside that namespace.
///
/// # Post-conditions
/// Only the first matching occurrence is removed; if it was the list's last
/// element, the whole `mx.<module_name>.plugins` declaration is dropped
/// instead of being left as `[]`. If the plugin is not listed, or the list
/// option is not declared at all, `file` is left untouched — this is
/// silently accepted, not an error (see `mxList::remove`).
///
/// # Returns
/// `Ok(())` whether or not the plugin was actually present.
///
/// # Errors
/// Whatever `mxList::remove` returns.
pub fn remove_plugin_no_transaction(
    file: &mut NixFile,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    mxList::new(&plugins_path(module_name), true)
        .remove(file, &format!("pkgs.{}.{}", plugin_namespace, plugin_name))?;
    Ok(())
}

/// Runs `edit` against every name in `targets`, inside a single transaction
/// on `module.nix`.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `description` - human-readable label attached to the resulting commit.
/// * `targets` - module names to edit, in order; typically the output of
///   [`crate::module_info::resolve_with_children`] (a module plus its
///   children, for a meta-module).
/// * `edit` - one of the `*_no_transaction` functions above, applied to
///   each target in turn.
///
/// # Pre-conditions
/// Runs on a blocking thread ([`tokio::task::spawn_blocking`]), since
/// [`transaction::make_transaction`] performs blocking file/git I/O and a
/// blocking `nixos-rebuild`.
///
/// # Post-conditions
/// All of `targets` are edited within the *same* transaction: at most one
/// git commit and one `nixos-rebuild switch` cover the whole set, whether
/// `targets` has one entry or many — the loop is not one transaction per
/// name. If `edit` fails on any target, none of `targets` end up applied
/// and the configuration is rolled back to its previous commit (see
/// [`transaction::make_transaction`]). If every edit turns out to be a
/// no-op (e.g. `targets` were all already in the desired state), the
/// resulting file is byte-for-byte unchanged, so no commit is created and
/// no rebuild runs at all. `flake.lock` is left untouched
/// (`UpdateInput::Keep`) whenever a commit does happen.
///
/// # Returns
/// `Ok(())` once the transaction completes, whether or not it needed a
/// rebuild.
///
/// # Errors
/// [`mx::ErrorKind::ThreadError`] if the blocking task panics or is
/// cancelled; otherwise whatever `edit` or
/// [`transaction::make_transaction`] return (e.g. a `BuildError` from a
/// failed `nixos-rebuild`).
async fn edit_modules(
    config_dir: &str,
    description: String,
    targets: Vec<String>,
    edit: fn(&mut NixFile, &str) -> mx::Result<()>,
) -> mx::Result<()> {
    let config_dir = config_dir.to_string();
    tokio::task::spawn_blocking(move || {
        transaction::make_transaction(
            &description,
            &config_dir,
            FILE_MODULE_PATH,
            BuildCommand::Switch,
            UpdateInput::Keep,
            |file| {
                for target in &targets {
                    edit(file, target)?;
                }
                Ok(())
            },
        )
    })
    .await
    .map_err(|_| mx::ErrorKind::ThreadError)?
}

/// Enables `module_name`, and every child module if it is a meta-module, in
/// one transaction.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `module_name` - dotted module name, resolved against the remote module
///   index via [`crate::module_info::resolve_with_children`]. If it names a
///   meta-module, the meta-module itself and every child it lists are all
///   enabled together.
///
/// # Post-conditions
/// See `edit_modules`: at most one commit and one `nixos-rebuild switch`
/// cover the whole set of targets, skipped entirely if every target was
/// already enabled; on any failure none of them end up enabled.
///
/// # Returns
/// `Ok(())` once the transaction completes, whether or not it needed a
/// rebuild.
///
/// # Errors
/// Whatever [`crate::module_info::resolve_with_children`] returns (e.g. an
/// `HttpError` fetching the remote index) or whatever `edit_modules`
/// returns.
pub async fn install(config_dir: &str, module_name: &str) -> mx::Result<()> {
    let targets = crate::module_info::resolve_with_children(module_name).await?;
    edit_modules(
        config_dir,
        format!("Install module {module_name}"),
        targets,
        install_no_transaction,
    )
    .await
}

/// Disables `module_name`, and every child module if it is a meta-module, in
/// one transaction.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `module_name` - dotted module name, resolved the same way as in
///   [`install`].
///
/// # Post-conditions
/// See `edit_modules`: at most one commit and one `nixos-rebuild switch`
/// cover the whole set of targets, skipped entirely if none of them were
/// enabled to begin with. A target whose `enable` option was not declared
/// is silently left as-is (see [`uninstall_no_transaction`]) rather than
/// failing the whole operation.
///
/// # Returns
/// `Ok(())` once the transaction completes, whether or not it needed a
/// rebuild.
///
/// # Errors
/// Whatever [`crate::module_info::resolve_with_children`] or
/// `edit_modules` return.
pub async fn uninstall(config_dir: &str, module_name: &str) -> mx::Result<()> {
    let targets = crate::module_info::resolve_with_children(module_name).await?;
    edit_modules(
        config_dir,
        format!("Uninstall module {module_name}"),
        targets,
        uninstall_no_transaction,
    )
    .await
}

/// Enables `module_name` and adds one plugin to it, in a single transaction.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `module_name` - dotted module name whose plugin list is edited; also
///   the module that gets enabled. Unlike [`install`], this name is used as
///   given — no meta-module expansion.
/// * `plugin_namespace` - nixpkgs namespace the plugin is read from.
/// * `plugin_name` - attribute name of the plugin inside that namespace.
///
/// # Pre-conditions
/// Runs on a blocking thread, like `edit_modules`.
///
/// # Post-conditions
/// See [`install_plugin_no_transaction`] for the edit itself; committed as
/// one transaction, with one `nixos-rebuild switch` if the edit actually
/// changed `module.nix` (skipped if the module was already enabled with
/// this exact plugin already listed), rolled back entirely on failure.
/// `flake.lock` is left untouched.
///
/// # Returns
/// `Ok(())` once the transaction completes, whether or not it needed a
/// rebuild.
///
/// # Errors
/// [`mx::ErrorKind::ThreadError`] if the blocking task panics or is
/// cancelled; otherwise whatever [`install_plugin_no_transaction`] or
/// `transaction::make_transaction` return.
pub async fn install_plugin(
    config_dir: &str,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    let config_dir = config_dir.to_string();
    let module_name = module_name.to_string();
    let plugin_namespace = plugin_namespace.to_string();
    let plugin_name = plugin_name.to_string();
    tokio::task::spawn_blocking(move || {
        transaction::make_transaction(
            &format!("Install {} plugin for module {}", plugin_name, module_name),
            &config_dir,
            FILE_MODULE_PATH,
            BuildCommand::Switch,
            UpdateInput::Keep,
            |file| {
                install_plugin_no_transaction(file, &module_name, &plugin_namespace, &plugin_name)
            },
        )
    })
    .await
    .map_err(|_| mx::ErrorKind::ThreadError)?
}

/// Removes one plugin from `module_name`'s plugin list, in a single
/// transaction.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `module_name` - dotted module name whose plugin list is edited; its
///   `enable` option is not touched.
/// * `plugin_namespace` - nixpkgs namespace the plugin is read from.
/// * `plugin_name` - attribute name of the plugin inside that namespace.
///
/// # Pre-conditions
/// Runs on a blocking thread, like `edit_modules`.
///
/// # Post-conditions
/// See [`remove_plugin_no_transaction`]: removing a plugin that is not
/// listed is silently accepted, not an error. Since the edit then leaves
/// `module.nix` byte-for-byte unchanged, the transaction detects no diff and
/// commits nothing — no git commit and no `nixos-rebuild` happen in that
/// case. `flake.lock` is left untouched either way.
///
/// # Returns
/// `Ok(())` once the transaction completes, whether or not it needed a
/// rebuild.
///
/// # Errors
/// [`mx::ErrorKind::ThreadError`] if the blocking task panics or is
/// cancelled; otherwise whatever [`remove_plugin_no_transaction`] or
/// `transaction::make_transaction` return.
pub async fn remove_plugin(
    config_dir: &str,
    module_name: &str,
    plugin_namespace: &str,
    plugin_name: &str,
) -> mx::Result<()> {
    let config_dir = config_dir.to_string();
    let module_name = module_name.to_string();
    let plugin_namespace = plugin_namespace.to_string();
    let plugin_name = plugin_name.to_string();
    tokio::task::spawn_blocking(move || {
        transaction::make_transaction(
            &format!("Remove {} plugin for module {}", plugin_name, module_name),
            &config_dir,
            FILE_MODULE_PATH,
            BuildCommand::Switch,
            UpdateInput::Keep,
            |file| {
                remove_plugin_no_transaction(file, &module_name, &plugin_namespace, &plugin_name)
            },
        )
    })
    .await
    .map_err(|_| mx::ErrorKind::ThreadError)?
}

/// Collect the names of every module enabled via `mx.<name>.enable = true;`.
///
/// # Parameters
/// * `file` - the file to read (open under a transaction, read-only or not).
///
/// # Returns
/// One dotted module name per `enable` descendant of `mx` (at any depth —
/// module names are dotted paths of arbitrary depth, e.g.
/// `programs.studio.obs-studio`, so the candidates are every `enable`
/// descendant of `mx`, not just its immediate children) whose value trims to
/// exactly `"true"`; a descendant with any other value, or with no `enable`
/// declared at all, is skipped rather than reported.
///
/// # Errors
/// Propagates any error from [`mxOption::list_enable_descendants`] or
/// [`mxOption::get`] other than [`mx::ErrorKind::OptionNotFound`], which is
/// treated as "not enabled" rather than failing the whole listing.
fn enabled_module_names(file: &NixFile) -> mx::Result<Vec<String>> {
    let mut names = Vec::new();
    for module in mxOption::new("mx").list_enable_descendants(file)? {
        match mxOption::new(&enable_path(&module)).get(file) {
            Ok(value) if value.trim() == "true" => names.push(module),
            Ok(_) | Err(mx::ErrorKind::OptionNotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(names)
}

/// Names of the modules currently enabled in the configuration (`mx.*.enable`),
/// read from `module.nix` without resolving remote metadata.
///
/// A configuration that has never had a module installed has no `module.nix`
/// at all: that is "no module enabled", not an error. The check is done here
/// rather than by catching `FileNotFound` from the transaction, which also
/// opens `configuration.nix` and would make a genuinely broken configuration
/// look like an empty one. The existence check concatenates `config_dir` and
/// `FILE_MODULE_PATH` directly — the same concatenation `NixFile::new` uses —
/// so `config_dir` is expected to already carry a trailing separator.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
///
/// # Pre-conditions
/// None beyond `config_dir` being a valid (possibly incomplete) Modulix
/// configuration directory; unlike the writing functions above this does
/// not run on a blocking thread itself — callers doing so from async code
/// must wrap it (see [`list_installed_modules`]).
///
/// # Post-conditions
/// A read-only transaction is opened and closed on `module.nix`; nothing is
/// written, no rebuild runs (`make_transaction_read_only` never commits a
/// change).
///
/// # Returns
/// The names from `enabled_module_names`, or `Vec::new()` if `module.nix`
/// does not exist yet.
///
/// # Errors
/// Whatever `transaction::make_transaction_read_only` or
/// `enabled_module_names` return.
pub fn list_enabled_module_names(config_dir: &str) -> mx::Result<Vec<String>> {
    if !path::Path::new(&format!("{config_dir}{FILE_MODULE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    transaction::make_transaction_read_only(
        "List enabled modules",
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Boot,
        enabled_module_names,
    )
}

/// Attributes currently listed under `mx.<module_name>.plugins`, as the raw
/// `pkgs.<namespace>.<plugin>` tokens written by `install_plugin_no_transaction`.
///
/// Same "no module.nix yet" short-circuit as `list_enabled_module_names`: an
/// absent file means no plugin is installed, not an error.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
/// * `module_name` - dotted module name whose plugin list is read.
///
/// # Post-conditions
/// A read-only transaction is opened and closed on `module.nix`; nothing is
/// written, no rebuild runs.
///
/// # Returns
/// The raw `pkgs.<namespace>.<plugin>` tokens of `mx.<module_name>.plugins`,
/// or `Vec::new()` if `module.nix` does not exist, or if the option is not
/// declared for this module.
///
/// # Errors
/// Whatever `transaction::make_transaction_read_only` returns, or
/// [`mx::ErrorKind::OptionIsNotList`] if the option is declared but holds
/// something other than a list.
pub fn list_installed_plugin_attrs(config_dir: &str, module_name: &str) -> mx::Result<Vec<String>> {
    if !path::Path::new(&format!("{config_dir}{FILE_MODULE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    let module_name = module_name.to_string();
    transaction::make_transaction_read_only(
        "List module plugins",
        config_dir,
        FILE_MODULE_PATH,
        BuildCommand::Boot,
        move |file| {
            match mxList::new(&plugins_path(&module_name), true).get_element_in_list(file) {
                Ok(elems) => Ok(elems.map(str::to_string).collect()),
                Err(mx::ErrorKind::OptionNotFound) => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        },
    )
}

/// Installed (enabled) Modulix modules, resolved against the remote module index
/// so GUI metadata (display name, summary, icon) is available. Modules enabled
/// locally but absent from the index are skipped.
///
/// # Parameters
/// * `config_dir` - root directory of the NixOS configuration.
///
/// # Post-conditions
/// [`list_enabled_module_names`] runs on a blocking thread. Each enabled
/// name is then resolved with [`ModuleInfo::new`]; a name that fails to
/// resolve (e.g. [`mx::ErrorKind::PackageNotFound`] because it was removed
/// from the remote index since being enabled) is silently dropped instead
/// of failing the whole listing.
///
/// # Returns
/// One [`ModuleInfo`] per locally-enabled module that is also still present
/// in the remote index, in the order [`list_enabled_module_names`] returned
/// them.
///
/// # Errors
/// [`mx::ErrorKind::ThreadError`] if the blocking task panics or is
/// cancelled; otherwise whatever [`list_enabled_module_names`] returns.
pub async fn list_installed_modules(config_dir: &str) -> mx::Result<Vec<ModuleInfo>> {
    let config_dir = config_dir.to_string();
    let names = tokio::task::spawn_blocking(move || list_enabled_module_names(&config_dir))
        .await
        .map_err(|_| mx::ErrorKind::ThreadError)??;
    let mut modules = Vec::with_capacity(names.len());
    for name in names {
        if let Ok(module) = ModuleInfo::new(&name).await {
            modules.push(module);
        }
    }
    Ok(modules)
}

#[cfg(test)]
#[path = "install_module_tests.rs"]
mod tests;
