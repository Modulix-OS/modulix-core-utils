//! Installs, uninstalls and lists nixpkgs packages through
//! `environment.systemPackages`.
//!
//! A package is recorded as the bare `pkgs.<attr>` reference (e.g.
//! `pkgs.firefox`, or `pkgs.firefox.dev` to request a specific output), never
//! as a Nix string: `crate::core::list::List` treats every list element as
//! opaque Nix text, so install and uninstall only have to agree on that exact
//! spelling. Neither `install_no_transaction` nor [`install`] checks that the
//! attribute exists in nixpkgs; an unknown or misspelled attribute is only
//! caught once the transaction's `nixos-rebuild switch` evaluates the
//! configuration, as a nix evaluation failure wrapped in
//! `mx::ErrorKind::BuildError` - at which point `transaction::make_transaction`
//! has already rolled the edit back.
//!
//! Listing installed packages is a separate, read-only path: it parses the
//! current `environment.systemPackages` value out of `package.nix`, then
//! resolves display metadata (`pname`, `version`, `description`) for those
//! attributes with a single `nix eval` against nixpkgs, located through
//! `NIXPKGS_LOOKUP`. That expression maps an attribute no longer present in
//! nixpkgs to `null` and filters it out, so a stale entry left over from a
//! removed package silently disappears from the listing instead of failing
//! it.

use std::collections::HashMap;
use std::path;

#[cfg(feature = "app-info-gui")]
use tokio::sync::OnceCell;

use crate::core::transaction::transaction::UpdateInput;
use crate::{
    core::{
        list::List as mxList,
        transaction::{self, file_lock::NixFile, transaction::BuildCommand},
    },
    mx,
    package_info::NixPackage,
};

/// Relative path, under a configuration directory, of the Nix file this
/// module reads and writes.
///
/// Passed as `file_path` to [`transaction::make_transaction`] /
/// [`transaction::make_transaction_read_only`] by every public entry point
/// below.
const FILE_PACKAGE_PATH: &str = "package.nix";

/// Appends each of `packages` to `environment.systemPackages`, as the
/// composable half of [`install`].
///
/// # Parameters
/// * `file` - the open `package.nix` to edit; must belong to a writable
///   transaction.
/// * `packages` - nixpkgs attribute names (e.g. `"firefox"`), written
///   verbatim after `pkgs.`; an output suffix is not added here, so passing
///   `"firefox.dev"` records that literal attribute path.
///
/// # Pre-conditions
/// `file` was opened for writing (`transaction::file_lock::NixFile::begin`
/// with write permission).
///
/// # Post-conditions
/// Every name in `packages` already present in `environment.systemPackages`
/// (exact text match on `pkgs.<name>`) is left as a single entry, since the
/// underlying `mxList` is built with `unique_value = true`. Nothing is
/// validated against nixpkgs; an attribute that does not exist there is only
/// caught when the enclosing transaction later runs `nixos-rebuild switch`.
///
/// # Returns
/// `Ok(())` once every package has been added.
///
/// # Errors
/// [`mx::ErrorKind::OptionIsNotList`] if `environment.systemPackages` is
/// declared as something other than a list, plus any error
/// `mxList::add` propagates from parsing or rewriting the file.
pub fn install_no_transaction(file: &mut NixFile, packages: &[&str]) -> mx::Result<()> {
    let list = mxList::new("environment.systemPackages", true);
    for package_name in packages {
        list.add(file, &format!("pkgs.{}", package_name))?;
    }
    Ok(())
}

/// Removes each of `packages` from `environment.systemPackages`, as the
/// composable half of [`uninstall`].
///
/// # Parameters
/// * `file` - the open `package.nix` to edit; must belong to a writable
///   transaction.
/// * `packages` - nixpkgs attribute names, matched against the file as
///   `pkgs.<name>`; must be spelled exactly as they were passed to
///   [`install_no_transaction`], output suffix included.
///
/// # Pre-conditions
/// `file` was opened for writing.
///
/// # Post-conditions
/// A name not currently installed, or an altogether absent
/// `environment.systemPackages` declaration, is silently ignored for that
/// name - see `mxList::remove`. Removing the last remaining package drops
/// the whole option declaration rather than leaving `[]`.
///
/// # Returns
/// `Ok(())` once every package has been processed, whether or not it was
/// actually present.
///
/// # Errors
/// Any error `mxList::remove` propagates from parsing or rewriting the
/// file.
pub fn uninstall_no_transaction(file: &mut NixFile, packages: &[&str]) -> mx::Result<()> {
    let list = mxList::new("environment.systemPackages", true);
    for package_name in packages {
        list.remove(file, &format!("pkgs.{}", package_name))?;
    }
    Ok(())
}

/// Nix output names recognised as an explicit output suffix by
/// [`parse_pkg_entry`] (e.g. the `dev` in `pkgs.firefox.dev`).
///
/// Not exhaustive of every output nixpkgs can define - just the common ones a
/// package entry is realistically qualified with - so an entry using a rarer
/// output name is parsed as an unqualified attribute whose last path segment
/// happens to be that name.
const NIX_OUTPUTS: &[&str] = &["out", "dev", "lib", "doc", "man", "info", "static"];

/// Splits one `environment.systemPackages` entry into an attribute name and
/// an output.
///
/// # Parameters
/// * `raw` - a single list element as read from the file, expected in
///   `pkgs.<name>` or `pkgs.<name>.<output>` form (the `pkgs.` prefix this
///   module writes); an entry lacking that prefix, e.g. hand-edited into the
///   file, is parsed the same way on whatever text follows.
///
/// # Returns
/// `(name, output)`: `output` is the trailing path segment when it is a
/// member of [`NIX_OUTPUTS`] (and `name` is what precedes it), otherwise
/// `output` defaults to `"out"` and `name` is `raw` with only the `pkgs.`
/// prefix stripped.
fn parse_pkg_entry(raw: &str) -> (String, String) {
    let stripped = raw.strip_prefix("pkgs.").unwrap_or(raw);
    match stripped.rsplit_once('.') {
        Some((name, output)) if NIX_OUTPUTS.contains(&output) => {
            (name.to_string(), output.to_string())
        }
        _ => (stripped.to_string(), "out".to_string()),
    }
}

/// Reads and parses the current `environment.systemPackages` value.
///
/// # Parameters
/// * `file` - the file to read; a read-only transaction's file works.
///
/// # Returns
/// One `(name, output)` pair per list element, in declaration order, via
/// [`parse_pkg_entry`]; an empty `Vec` when `environment.systemPackages` is
/// not declared at all (nothing has ever been installed through this
/// module).
///
/// # Errors
/// [`mx::ErrorKind::OptionIsNotList`] if the option is declared as something
/// other than a list, plus any error [`mxList::get_element_in_list`]
/// propagates from parsing the file.
fn collect_entries(file: &NixFile) -> mx::Result<Vec<(String, String)>> {
    let pkgs = mxList::new("environment.systemPackages", true);
    let entries: Vec<(String, String)> = match pkgs.get_element_in_list(file) {
        Ok(e) => e.map(|n| parse_pkg_entry(n)).collect(),
        Err(mx::ErrorKind::OptionNotFound) => vec![],
        Err(e) => return Err(e),
    };
    Ok(entries)
}

/// Where `build_nix_expr` looks for nixpkgs in the configuration flake.
///
/// A Modulix configuration consumes nixpkgs *through* `mxpkgs` — its own
/// inputs are `mxpkgs` and `nixos-hardware` only — so the direct
/// `inputs.nixpkgs` this used to assume fails outright there with
/// `attribute 'nixpkgs' missing`, taking the whole installed listing with it.
/// The direct input is still tried first so a configuration that does expose
/// nixpkgs keeps working.
///
/// Deliberately not `nixosConfigurations.<name>.pkgs`, which would be more
/// faithful (overlays included) but evaluates the entire system configuration
/// on every listing; `legacyPackages` is a cheap lookup.
///
/// # Returns
/// A Nix expression fragment (not a full program) meant to be spliced after
/// `nixpkgs = ` in [`build_nix_expr`]'s `let` block, evaluating to the
/// `nixpkgs` flake input reached from `flake`: the direct input if present,
/// else the one `mxpkgs` re-exports, else a `throw` that fails the whole
/// `nix eval` with an explicit message instead of the interpreter's own
/// `attribute 'nixpkgs' missing`.
const NIXPKGS_LOOKUP: &str = "flake.inputs.nixpkgs \
     or flake.inputs.mxpkgs.inputs.nixpkgs \
     or (throw \"no nixpkgs input in the Modulix configuration flake\")";

/// Builds the `nix eval` expression that resolves display metadata for a set
/// of package entries.
///
/// # Parameters
/// * `config_dir` - path passed to `builtins.getFlake`, i.e. the
///   configuration's flake directory.
/// * `entries` - `(name, output)` pairs as returned by [`collect_entries`];
///   only `name` is used here, one per attribute to look up in nixpkgs - the
///   output is metadata-only and does not affect which attribute is
///   evaluated.
///
/// # Returns
/// A self-contained Nix expression string that, once evaluated, is a JSON
/// object keyed by attribute name, each value holding `pname`, `description`
/// and `version` (each falling back to `name` or `""` when nixpkgs does not
/// set it); an attribute from `entries` that nixpkgs does not define is
/// dropped from the result rather than failing the evaluation - see
/// [`NIXPKGS_LOOKUP`] for how `nixpkgs` itself is located.
fn build_nix_expr(config_dir: &str, entries: &[(String, String)]) -> String {
    let nix_list = entries
        .iter()
        .map(|(name, _)| format!("\"{}\"", name))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "let \
           flake = builtins.getFlake \"{}\"; \
           nixpkgs = {}; \
           pkgs = nixpkgs.legacyPackages.${{builtins.currentSystem}}; in \
         builtins.listToAttrs \
           (builtins.filter (x: x != null) \
             (map (name: \
               let pkg = pkgs.${{name}} or null; in \
               if pkg == null then null \
               else {{ name = name; value = {{ \
                 pname = pkg.pname or name; \
                 description = pkg.meta.description or \"\"; \
                 version = pkg.version or \"\"; \
               }}; }}) \
             [ {} ]))",
        config_dir, NIXPKGS_LOOKUP, nix_list
    )
}

/// Runs `nix eval --impure --json --expr <expr>` and parses its JSON output.
///
/// # Parameters
/// * `expr` - a complete Nix expression, e.g. one built by
///   [`build_nix_expr`], evaluating to a JSON-representable value.
///
/// # Returns
/// The parsed JSON object, as a map from key to raw [`serde_json::Value`].
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the `nix` binary cannot be spawned;
/// [`mx::ErrorKind::NixCommandError`] if the process exits non-zero (payload
/// is its stderr) or if its stdout does not parse as JSON (payload is the
/// parse error's message).
fn eval_nix_expr(expr: &str) -> mx::Result<HashMap<String, serde_json::Value>> {
    let cmd_output = std::process::Command::new("nix")
        .args(["eval", "--impure", "--json", "--expr", expr])
        .output()
        .map_err(mx::ErrorKind::IOError)?;
    if !cmd_output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&cmd_output.stderr).to_string(),
        ));
    }
    serde_json::from_slice(&cmd_output.stdout)
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))
}

/// Assembles one [`NixPackage`] from a parsed entry and the metadata `nix
/// eval` returned for it.
///
/// # Parameters
/// * `name` - the nixpkgs attribute name; becomes `pkg_name`, and the
///   fallback `pname` when `pkg_map` has nothing for it.
/// * `explicit_output` - the output parsed from the list entry (e.g.
///   `"out"`, `"dev"`); becomes the sole element of `outputs`, since this
///   path only knows the output the configuration actually requested, not
///   every output nixpkgs exposes for the package.
/// * `pkg_map` - the map [`eval_nix_expr`] returned for the whole batch;
///   looked up by `name`.
///
/// # Returns
/// A [`NixPackage`] with `pname`/`description`/`version` taken from
/// `pkg_map[name]` when present, each falling back independently (`pname` to
/// `name`, `description` and `version` to an empty string) when `pkg_map`
/// has no entry for `name` or is missing that particular field - which
/// happens for an attribute nixpkgs no longer defines, since
/// [`build_nix_expr`] filters those out rather than erroring.
fn build_package(
    name: String,
    explicit_output: String,
    pkg_map: &HashMap<String, serde_json::Value>,
) -> NixPackage {
    let info = pkg_map.get(&name);
    NixPackage {
        pname: info
            .and_then(|v| v["pname"].as_str())
            .unwrap_or(&name)
            .to_string(),
        description: info
            .and_then(|v| v["description"].as_str())
            .unwrap_or_default()
            .to_string(),
        version: info
            .and_then(|v| v["version"].as_str())
            .unwrap_or_default()
            .to_string(),
        outputs: vec![explicit_output],
        pkg_name: name,
        #[cfg(feature = "app-info-gui")]
        flatpak: OnceCell::new(),
    }
}

/// Reads `environment.systemPackages` from `file` and resolves display
/// metadata for every entry via one `nix eval`.
///
/// # Parameters
/// * `config_dir` - the configuration's flake directory, passed through to
///   `build_nix_expr`'s `builtins.getFlake`.
/// * `file` - the open `package.nix` to read; a read-only transaction's file
///   works, since nothing is written.
///
/// # Returns
/// One [`NixPackage`] per entry in `environment.systemPackages`, in
/// declaration order; empty when the option is not declared.
///
/// # Errors
/// Whatever `collect_entries` or `eval_nix_expr` returns, including
/// [`mx::ErrorKind::OptionIsNotList`], [`mx::ErrorKind::IOError`] and
/// [`mx::ErrorKind::NixCommandError`].
pub fn list_installed_package_no_transaction(
    config_dir: &str,
    file: &NixFile,
) -> mx::Result<Vec<NixPackage>> {
    let entries = collect_entries(file)?;
    let nix_expr = build_nix_expr(config_dir, &entries);
    let pkg_map = eval_nix_expr(&nix_expr)?;
    Ok(entries
        .into_iter()
        .map(|(name, output)| build_package(name, output, &pkg_map))
        .collect())
}

/// Adds `packages` to `environment.systemPackages` and rebuilds the system,
/// as one atomic, git-versioned transaction.
///
/// # Parameters
/// * `config_dir` - the configuration directory to edit.
/// * `packages` - nixpkgs attribute names to install; see
///   [`install_no_transaction`] for the exact text they are written as.
///
/// # Pre-conditions
/// The caller has write access to `config_dir` and holds whatever privilege
/// `nixos-rebuild switch` needs (this is expected to run privileged).
///
/// # Post-conditions
/// On success, `package.nix` now lists every package in `packages` and the
/// running system has been switched to the new configuration
/// (`nixos-rebuild switch`, via `transaction::make_transaction`) - this
/// blocks for as long as the rebuild takes, possibly minutes, and is
/// serialised against other transactions' rebuilds. On any failure,
/// including an unknown nixpkgs attribute rejected during evaluation, the
/// configuration is rolled back to its previous commit and nothing is
/// switched.
///
/// # Returns
/// `Ok(())` once the switch has succeeded.
///
/// # Errors
/// Any [`mx::ErrorKind`] `transaction::make_transaction` or
/// [`install_no_transaction`] can produce, notably
/// [`mx::ErrorKind::BuildError`] when `nixos-rebuild switch` fails (e.g. an
/// unknown attribute) and [`mx::ErrorKind::GitNotCommitted`] when the
/// configuration repository is not clean enough to proceed.
pub fn install(config_dir: &str, packages: &[&str]) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Install packages {}", packages.join(", ")),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| install_no_transaction(file, packages),
    )
}

/// Removes `packages` from `environment.systemPackages` and rebuilds the
/// system, as one atomic, git-versioned transaction.
///
/// # Parameters
/// * `config_dir` - the configuration directory to edit.
/// * `packages` - nixpkgs attribute names to uninstall; must match the
///   spelling used at install time, output suffix included - see
///   [`uninstall_no_transaction`].
///
/// # Pre-conditions
/// Same as [`install`]: write access to `config_dir` and the privilege
/// `nixos-rebuild switch` needs.
///
/// # Post-conditions
/// A name in `packages` that was not actually installed is silently
/// skipped, not an error. On success the running system has been switched
/// to the configuration with those packages removed; on any failure the
/// configuration is rolled back and nothing is switched.
///
/// # Returns
/// `Ok(())` once the switch has succeeded.
///
/// # Errors
/// Any [`mx::ErrorKind`] `transaction::make_transaction` or
/// [`uninstall_no_transaction`] can produce, notably
/// [`mx::ErrorKind::BuildError`] when `nixos-rebuild switch` fails.
pub fn uninstall(config_dir: &str, packages: &[&str]) -> mx::Result<()> {
    transaction::make_transaction(
        &format!("Uninstall {}", packages.join(", ")),
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| uninstall_no_transaction(file, packages),
    )
}

/// Lists every package currently declared in `environment.systemPackages`,
/// with display metadata resolved from nixpkgs.
///
/// # Parameters
/// * `config_dir` - the configuration directory to read.
///
/// # Pre-conditions
/// None beyond read access to `config_dir`; safe to call even when nothing
/// was ever installed.
///
/// # Post-conditions
/// The configuration is left untouched: this opens a read-only transaction
/// (`BuildCommand::Boot` is passed but never actually run - a read-only
/// transaction commits nothing), so no rebuild happens and no attribute
/// validity is checked.
///
/// # Returns
/// One [`NixPackage`] per declared package. `Ok(Vec::new())` specifically
/// when `package.nix` does not exist yet - i.e. nothing has ever been
/// installed through this module - which is treated as "no packages", not
/// an error.
///
/// # Errors
/// Any [`mx::ErrorKind`] `transaction::make_transaction_read_only` or
/// [`list_installed_package_no_transaction`] can produce, notably
/// [`mx::ErrorKind::IOError`] / [`mx::ErrorKind::NixCommandError`] from the
/// metadata `nix eval`.
pub fn list_installed_package(config_dir: &str) -> mx::Result<Vec<NixPackage>> {
    if !path::Path::new(&format!("{config_dir}{FILE_PACKAGE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    transaction::make_transaction_read_only(
        "List installed package",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| list_installed_package_no_transaction(config_dir, file),
    )
}

/// nixpkgs attributes listed in `environment.systemPackages`, without the
/// `nix eval` [`list_installed_package`] pays to resolve pname/version/
/// description. Parsing `package.nix` is all it takes to answer "is this
/// installed?", which callers ask on hot paths (every store search).
///
/// A configuration where nothing has ever been installed has no `package.nix`
/// at all: that is "no package installed", not an error (a read-only
/// transaction does not create the file, it returns `FileNotFound`). The check
/// is done here rather than by catching `FileNotFound` from the transaction,
/// which also opens `configuration.nix` and would make a genuinely broken
/// configuration look like an empty one. The existence check concatenates
/// `config_dir` and `FILE_PACKAGE_PATH` directly — the same concatenation
/// `NixFile::new` uses — so `config_dir` is expected to already carry a
/// trailing separator.
///
/// # Parameters
/// * `config_dir` - the configuration directory to read.
///
/// # Pre-conditions
/// None beyond read access to `config_dir`; safe to call even when nothing
/// was ever installed.
///
/// # Post-conditions
/// The configuration is left untouched; no `nix eval` runs, unlike
/// [`list_installed_package`].
///
/// # Returns
/// The nixpkgs attribute name of every entry in `environment.systemPackages`
/// (the output suffix, if any, is dropped), in declaration order.
/// `Ok(Vec::new())` when `package.nix` does not exist yet.
///
/// # Errors
/// Any [`mx::ErrorKind`] `transaction::make_transaction_read_only` or
/// `collect_entries` can produce, notably
/// [`mx::ErrorKind::OptionIsNotList`].
pub fn list_installed_package_names(config_dir: &str) -> mx::Result<Vec<String>> {
    if !path::Path::new(&format!("{config_dir}{FILE_PACKAGE_PATH}")).exists() {
        return Ok(Vec::new());
    }
    transaction::make_transaction_read_only(
        "List installed package names",
        config_dir,
        FILE_PACKAGE_PATH,
        BuildCommand::Boot,
        |file| {
            Ok(collect_entries(file)?
                .into_iter()
                .map(|(name, _)| name)
                .collect())
        },
    )
}

/// Unit tests for this module's pure helpers.
#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that [`build_nix_expr`] locates `nixpkgs` through
    /// [`NIXPKGS_LOOKUP`] (direct input, with the `mxpkgs` fallback) and
    /// splices in the flake path and the requested package name.
    #[test]
    fn build_nix_expr_resolves_nixpkgs_through_mxpkgs() {
        let expr = build_nix_expr("/etc/modulix-os/", &[("htop".into(), "out".into())]);
        assert!(expr.contains("flake.inputs.nixpkgs"));
        assert!(expr.contains("flake.inputs.mxpkgs.inputs.nixpkgs"));
        assert!(expr.contains("builtins.getFlake \"/etc/modulix-os/\""));
        assert!(expr.contains("[ \"htop\" ]"));
    }
}
