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
//!
//! [`check_update`] is the same question asked the other way round, and is the
//! path the daemon actually drives: rather than probing each input, it lets
//! `nix` resolve the whole lockfile into a scratch file
//! (`--output-lock-file`) and hands the result back as text. A caller can then
//! keep that candidate around, describe it locally with [`diff_locks`], and
//! apply exactly it with [`update_with_lock`] - so the refresh's network cost
//! is paid once, and the revisions announced are the revisions installed.
//! [`update`] remains the one-shot variant for a caller with no candidate in
//! hand.

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

/// Relative path, under a configuration directory, of the lockfile this module
/// reads and produces candidates for.
const FILE_FLAKE_LOCK_PATH: &str = "flake.lock";

/// Name, under [`crate::cache_dir`], of the scratch lockfile
/// [`check_update`] hands to `nix flake update --output-lock-file`.
const UPDATE_PROBE_FILE: &str = "update-probe.lock";

/// Ceiling on the candidate-lock probe. A full `nix flake update` refetches
/// every input, so this is minutes, not seconds.
const UPDATE_PROBE_TIMEOUT: Duration = Duration::from_secs(900);

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
    let lock_path = path::Path::new(config_dir).join(FILE_FLAKE_LOCK_PATH);
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

/// Locked reference of one of `lock`'s **direct** inputs, by input name.
///
/// # Arguments
/// * `lock` - parsed lockfile to look into.
/// * `name` - input name as declared under `inputs` in `flake.nix`.
///
/// # Returns
/// `Some(locked)` when `root` declares `name` and the node it points at carries
/// a `locked` block; `None` otherwise, including when the lockfile has no
/// `root` node.
fn direct_input<'a>(lock: &'a FlakeLock, name: &str) -> Option<&'a LockedRef> {
    let root = lock.nodes.get(&lock.root)?;
    let key = root.inputs.get(name)?.as_str()?;
    lock.nodes.get(key)?.locked.as_ref()
}

/// Lists the direct inputs whose pinned revision differs between two
/// lockfiles.
///
/// The local counterpart of [`outdated_inputs`]: it spawns no process and
/// touches no network, so once [`check_update`] has produced a candidate
/// lockfile the same report can be rebuilt for free, and is consistent by
/// construction with what [`update_with_lock`] will apply.
///
/// # Arguments
/// * `old` - lockfile currently pinned, usually `<config_dir>/flake.lock`.
/// * `new` - candidate lockfile, as returned by [`check_update`].
///
/// # Returns
/// One [`OutdatedInput`] per direct input of `new`'s `root` node whose
/// revision moved, with `current_rev` read from `old` and `new_rev` /
/// `last_modified` from `new`. Order follows `new`'s iteration order, which is
/// unspecified.
///
/// # Post-conditions
/// An input absent from `old` (newly declared) is skipped rather than reported
/// as outdated, as is one whose `original` block is missing or of type `path`,
/// and one carrying no `locked` block. An input dropped in `new` is not
/// reported either: only `new`'s inputs are walked.
///
/// # Errors
/// [`mx::ErrorKind::ParseError`] if either argument is not valid JSON in the
/// `flake.lock` shape.
pub fn diff_locks(old: &str, new: &str) -> mx::Result<Vec<OutdatedInput>> {
    let old_lock: FlakeLock = serde_json::from_str(old).map_err(mx::ErrorKind::ParseError)?;
    let new_lock: FlakeLock = serde_json::from_str(new).map_err(mx::ErrorKind::ParseError)?;

    let Some(new_root) = new_lock.nodes.get(&new_lock.root) else {
        return Ok(vec![]);
    };

    let mut outdated = Vec::new();
    for (name, node_ref) in &new_root.inputs {
        let Some(node) = node_ref.as_str().and_then(|key| new_lock.nodes.get(key)) else {
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
        let Some(old_locked) = direct_input(&old_lock, name) else {
            continue;
        };

        let current_rev = compare_rev(old_locked);
        let new_rev = compare_rev(locked);
        if current_rev != new_rev {
            outdated.push(OutdatedInput {
                name: name.clone(),
                current_rev,
                new_rev,
                last_modified: locked.last_modified,
            });
        }
    }
    Ok(outdated)
}

/// Computes the lockfile a full refresh would produce, without applying it.
///
/// Runs `nix flake update` with `--output-lock-file`, so the new lock lands in
/// a scratch file under [`crate::cache_dir`] and **nothing inside `config_dir`
/// is written** - no transaction, no immutable-flag handling, no privilege
/// beyond reading the configuration. The caller can therefore show the pending
/// change (via [`diff_locks`]) and apply the very same revisions later with
/// [`update_with_lock`], instead of re-resolving them a second time.
///
/// # Arguments
/// * `config_dir` - configuration repository to probe.
///
/// # Pre-conditions
/// `<config_dir>/flake.lock` must exist: this is the update path of an already
/// initialised system, not a bootstrap.
///
/// # Returns
/// `Some(lockfile)` with the full text of the candidate `flake.lock` when it
/// differs from the current one, `None` when every input is already current.
/// The comparison is made on the parsed JSON, so a pure reformatting is not
/// reported as an update.
///
/// # Post-conditions
/// `<config_dir>/flake.lock` is untouched. The scratch file is left behind on
/// purpose - it is overwritten by the next probe and costs one lockfile.
/// Blocks for the whole refresh, which refetches every input and can take
/// minutes.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] if `nix flake update` fails or times out,
/// [`mx::ErrorKind::IOError`] if a lockfile cannot be read or the scratch
/// directory cannot be created, [`mx::ErrorKind::ParseError`] if either
/// lockfile is not valid JSON, [`mx::ErrorKind::InvalidFile`] if a path is not
/// UTF-8.
pub async fn check_update(config_dir: &str) -> mx::Result<Option<String>> {
    let lock_path = path::Path::new(config_dir).join(FILE_FLAKE_LOCK_PATH);
    let current = tokio::fs::read_to_string(&lock_path)
        .await
        .map_err(mx::ErrorKind::IOError)?;

    let probe_path = crate::cache_dir().join(UPDATE_PROBE_FILE);
    if let Some(parent) = probe_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(mx::ErrorKind::IOError)?;
    }

    let lock_arg = lock_path.to_str().ok_or(mx::ErrorKind::InvalidFile)?;
    let probe_arg = probe_path.to_str().ok_or(mx::ErrorKind::InvalidFile)?;

    nix_eval::run(
        &[
            "flake",
            "update",
            "--flake",
            config_dir,
            "--reference-lock-file",
            lock_arg,
            "--output-lock-file",
            probe_arg,
            "--refresh",
        ],
        UPDATE_PROBE_TIMEOUT,
    )
    .await?;

    let candidate = tokio::fs::read_to_string(&probe_path)
        .await
        .map_err(mx::ErrorKind::IOError)?;

    let current_json: serde_json::Value =
        serde_json::from_str(&current).map_err(mx::ErrorKind::ParseError)?;
    let candidate_json: serde_json::Value =
        serde_json::from_str(&candidate).map_err(mx::ErrorKind::ParseError)?;

    if current_json == candidate_json {
        Ok(None)
    } else {
        Ok(Some(candidate))
    }
}

/// Applies a lockfile computed beforehand, then rebuilds.
///
/// The counterpart of [`update`] for a caller that already holds a candidate
/// lockfile from [`check_update`]: the transaction writes `lock` as
/// `flake.lock` instead of running `nix flake update` again, so the system ends
/// up on exactly the revisions that were announced, and the refresh's network
/// cost is paid once rather than twice.
///
/// # Arguments
/// * `config_dir` - configuration repository to update.
/// * `lock` - full `flake.lock` text to write, verbatim.
/// * `build_command` - `Switch` to rebuild and switch immediately, `Boot` to
///   only prepare the next boot.
/// * `cores` - caps the rebuild's `nix` build to this many CPU cores
///   (`nixos-rebuild --cores`); `None` leaves the Nix default (all of them).
///
/// # Pre-conditions
/// `lock` must be a lockfile `nix` accepts for this very `flake.nix`; a stale
/// candidate, computed before an input was added or removed, makes the rebuild
/// fail rather than silently applying the wrong thing.
///
/// # Post-conditions
/// If `lock` is byte-identical to the repository's current `flake.lock`, no
/// commit is created and no rebuild runs. Blocks for the whole rebuild.
///
/// # Errors
/// As [`transaction::make_transaction_update`]: a failure to write the
/// lockfile surfaces as [`mx::ErrorKind::IOError`], a rebuild failure as
/// [`mx::ErrorKind::BuildError`].
pub fn update_with_lock(
    config_dir: &str,
    lock: String,
    build_command: BuildCommand,
    cores: Option<u32>,
) -> mx::Result<()> {
    make_transaction_update(
        "update system inputs",
        config_dir,
        FILE_FLAKE_PATH,
        build_command,
        UpdateInput::UseLock(lock),
        cores,
        update_no_transaction,
    )
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
