//! System update: refreshing the configuration's flake inputs and detecting
//! which ones are behind their upstream revision.
//!
//! On a flake-based NixOS system, "updating" is a single operation -
//! `nix flake update` followed by a rebuild - rather than a per-package
//! action, so this module has no notion of updating one input in isolation:
//! [`update`] always refreshes every input via `UpdateInput::UpdateAll`. The
//! transaction that carries it out edits no file (there is nothing to change
//! in `flake.nix` itself), so it goes through
//! [`transaction::make_transaction_update`] rather than
//! [`transaction::make_transaction`]: the commit is forced to attempt the
//! refresh even though [`update_no_transaction`] is a no-op, and only
//! proceeds to a real commit and rebuild if `flake.lock` actually moved.
//!
//! [`outdated_inputs`] is the read side: it compares the revision already
//! pinned in `flake.lock` against each input's current upstream revision, so
//! a caller (the daemon's `Store1.ListOutdatedInputs`) can show what an
//! update would change before running it.

use std::collections::HashMap;
use std::path;
use std::time::Duration;

use serde::Deserialize;

pub use crate::core::transaction::transaction::BuildCommand;
use crate::core::{
    nix_eval,
    transaction::{make_transaction_update, transaction::UpdateInput},
};
use crate::mx;

/// Relative path, under a configuration directory, of the flake this module
/// refreshes.
const FILE_FLAKE_PATH: &str = "flake.nix";

/// Payload of the update transaction. The refresh itself is performed by the
/// commit (`UpdateInput::UpdateAll`, forced via
/// [`transaction::make_transaction_update`]), so there is nothing to edit
/// here.
///
/// # Returns
/// Always `Ok(())`.
pub fn update_no_transaction(
    _flake: &mut crate::core::transaction::file_lock::NixFile,
) -> mx::Result<()> {
    Ok(())
}

/// Refreshes every flake input and applies the result.
///
/// # Arguments
/// * `config_dir` - configuration repository to update.
/// * `build_command` - `Switch` to rebuild and switch immediately, `Boot` to
///   only prepare the next boot.
/// * `cores` - caps the rebuild's `nix` build to this many CPU cores
///   (`nixos-rebuild --cores`); `None` leaves the Nix default (all of them).
///
/// # Post-conditions
/// If `nix flake update` leaves `flake.lock` unchanged (every input was
/// already current), no commit is created and no rebuild runs. Blocks for
/// the whole `nix flake update` plus, if anything changed, the whole
/// rebuild.
///
/// # Errors
/// As [`transaction::make_transaction_update`]: a `nix flake update` failure
/// surfaces as [`mx::ErrorKind::IOError`], a rebuild failure as
/// [`mx::ErrorKind::BuildError`].
pub fn update(config_dir: &str, build_command: BuildCommand, cores: Option<u32>) -> mx::Result<()> {
    make_transaction_update(
        "update system inputs",
        config_dir,
        FILE_FLAKE_PATH,
        build_command,
        UpdateInput::UpdateAll,
        cores,
        update_no_transaction,
    )
}

/// An input pinned in `flake.lock` whose upstream revision has moved past it.
///
/// # Fields
/// * `name` - the input's attribute name, as declared under `inputs` in
///   `flake.nix`.
/// * `current_rev` - the revision (or, lacking one, the `narHash`) currently
///   pinned in `flake.lock`.
/// * `new_rev` - the revision (or `narHash`) available upstream.
/// * `last_modified` - the upstream revision's timestamp, Unix seconds.
pub struct OutdatedInput {
    pub name: String,
    pub current_rev: String,
    pub new_rev: String,
    pub last_modified: u64,
}

/// Ceiling on a single `nix flake metadata` probe, per input.
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);

/// Deserialized shape of a `flake.lock` file, restricted to the fields
/// [`outdated_inputs`] needs.
#[derive(Deserialize)]
struct FlakeLock {
    nodes: HashMap<String, FlakeLockNode>,
    root: String,
}

/// One `nodes.*` entry of a `flake.lock`.
#[derive(Deserialize)]
struct FlakeLockNode {
    #[serde(default)]
    inputs: HashMap<String, serde_json::Value>,
    locked: Option<LockedRef>,
    original: Option<OriginalRef>,
}

/// A node's `locked` block: the revision actually fetched.
#[derive(Deserialize)]
struct LockedRef {
    rev: Option<String>,
    #[serde(rename = "narHash")]
    nar_hash: Option<String>,
    #[serde(rename = "lastModified", default)]
    last_modified: u64,
}

/// A node's `original` block: the flake reference as written in `flake.nix`,
/// unresolved.
#[derive(Deserialize)]
struct OriginalRef {
    #[serde(rename = "type")]
    kind: String,
    owner: Option<String>,
    repo: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    url: Option<String>,
    id: Option<String>,
}

/// Top-level shape of `nix flake metadata --json`, restricted to the field
/// [`outdated_inputs`] needs.
#[derive(Deserialize)]
struct FlakeMetadata {
    locked: LockedRef,
}

/// Rebuilds a flake reference nix can re-resolve, from a node's `original`
/// block.
///
/// # Arguments
/// * `original` - the node's unresolved reference.
///
/// # Returns
/// `Some(flake_ref)` for `github`, `gitlab`, `git`, `tarball`/`file` and
/// `indirect` inputs; `None` for `path` (a local input, never "behind") or
/// any type this crate does not know how to re-resolve, or one missing the
/// fields its type requires.
fn flake_ref_from_original(original: &OriginalRef) -> Option<String> {
    match original.kind.as_str() {
        "github" | "gitlab" | "sourcehut" => {
            let owner = original.owner.as_deref()?;
            let repo = original.repo.as_deref()?;
            Some(match &original.git_ref {
                Some(r) => format!("{}:{owner}/{repo}/{r}", original.kind),
                None => format!("{}:{owner}/{repo}", original.kind),
            })
        }
        "git" => {
            let url = original.url.as_deref()?;
            Some(match &original.git_ref {
                Some(r) => format!("git+{url}?ref={r}"),
                None => format!("git+{url}"),
            })
        }
        "tarball" | "file" => original.url.clone(),
        "indirect" => original.id.clone(),
        _ => None,
    }
}

/// Revision the caller cares about comparing: the `rev`, or the `narHash`
/// when the input has no revision (a tarball/file input).
///
/// # Returns
/// The empty string if `locked` carries neither, so two such inputs compare
/// equal rather than one erroring out.
fn compare_rev(locked: &LockedRef) -> String {
    locked
        .rev
        .clone()
        .or_else(|| locked.nar_hash.clone())
        .unwrap_or_default()
}

/// Lists the direct flake inputs whose upstream revision has moved past the
/// one pinned in `<config_dir>/flake.lock`.
///
/// # Arguments
/// * `config_dir` - configuration repository whose `flake.lock` is read.
///
/// # Returns
/// One [`OutdatedInput`] per direct input (`root`'s own `inputs`) that is
/// behind. Order follows `flake.lock`'s iteration order, which is
/// unspecified.
///
/// # Post-conditions
/// An input whose upstream metadata cannot be fetched (network down, a
/// private ref, a removed input) is silently skipped rather than failing the
/// whole call - a single unreachable input should not blank the Updates
/// page. `path` inputs are always skipped: a local input is never "behind".
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if `flake.lock` cannot be read,
/// [`mx::ErrorKind::ParseError`] if it is not valid JSON in the expected
/// shape.
pub async fn outdated_inputs(config_dir: &str) -> mx::Result<Vec<OutdatedInput>> {
    let lock_path = path::Path::new(config_dir).join("flake.lock");
    let content = tokio::fs::read_to_string(&lock_path)
        .await
        .map_err(mx::ErrorKind::IOError)?;
    let lock: FlakeLock = serde_json::from_str(&content).map_err(mx::ErrorKind::ParseError)?;

    let Some(root_node) = lock.nodes.get(&lock.root) else {
        return Ok(vec![]);
    };

    let mut outdated = Vec::new();
    for (name, node_ref) in &root_node.inputs {
        let Some(node_key) = node_ref.as_str() else {
            continue;
        };
        let Some(node) = lock.nodes.get(node_key) else {
            continue;
        };
        let Some(original) = &node.original else {
            continue;
        };
        if original.kind == "path" {
            continue;
        }
        let Some(locked) = &node.locked else {
            continue;
        };
        let Some(flake_ref) = flake_ref_from_original(original) else {
            continue;
        };

        let metadata: FlakeMetadata = match nix_eval::run_json(
            &[
                "flake",
                "metadata",
                &flake_ref,
                "--json",
                "--refresh",
                "--no-write-lock-file",
            ],
            METADATA_TIMEOUT,
        )
        .await
        {
            Ok(m) => m,
            Err(_) => continue,
        };

        let current_rev = compare_rev(locked);
        let new_rev = compare_rev(&metadata.locked);
        if current_rev != new_rev {
            outdated.push(OutdatedInput {
                name: name.clone(),
                current_rev,
                new_rev,
                last_modified: metadata.locked.last_modified,
            });
        }
    }
    Ok(outdated)
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
