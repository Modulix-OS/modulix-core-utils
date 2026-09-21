use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::core::app_info_trait::PLUGIN_NAMESPACE_PREFIXES;
use crate::core::nix_eval;
use crate::mx;

use super::{FORMAT_VERSION, MAGIC};

/// `nix search nixpkgs --json '^'` measured ~1.7s warm, ~20s cold (first-ever
/// eval-cache build) — generous headroom over the worst case.
const DUMP_TIMEOUT: Duration = Duration::from_secs(60);

/// Ceiling on the `nix flake metadata` call that yields the fingerprint.
const METADATA_TIMEOUT: Duration = Duration::from_secs(20);

/// `off, len` pairs per row, in field order (see `reader::RowView`).
const ROW_FIELDS: usize = 6;

/// Size of one row in the table, in bytes.
const ROW_SIZE: usize = ROW_FIELDS * 8;

/// One entry of the `nix search --json` dump, as far as the index needs it.
///
/// # Fields
/// * `pname` - the package's `pname`; empty when absent from the dump.
/// * `version` - its version; empty when absent.
/// * `description` - its `meta.description`; empty when absent.
#[derive(Deserialize)]
struct RawPackage {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

/// The part of `nix flake metadata --json` this module reads.
///
/// # Fields
/// * `fingerprint` - the flake's fingerprint, identifying its exact revision.
#[derive(Deserialize)]
struct FlakeMetadata {
    fingerprint: String,
}

/// The nixpkgs flake fingerprint, packed into a fixed 32-byte field (truncated
/// or zero-padded) so the on-disk header has a stable size. Only ever compared
/// for equality — never decoded — so this lossy packing is safe.
///
/// # Returns
/// The fingerprint of the `nixpkgs` registry entry, truncated or zero-padded to
/// 32 bytes.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] when `nix flake metadata` fails or takes
/// longer than the metadata timeout.
pub(super) async fn current_fingerprint() -> mx::Result<[u8; 32]> {
    let meta: FlakeMetadata = nix_eval::run_json(
        &["flake", "metadata", "nixpkgs", "--json"],
        METADATA_TIMEOUT,
    )
    .await?;
    let bytes = meta.fingerprint.as_bytes();
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    Ok(out)
}

/// Dumps the whole nixpkgs package set with a match-everything search.
///
/// # Returns
/// Every attribute of the target system, keyed by its full
/// `legacyPackages.<system>.<attr>` path.
///
/// # Post-conditions
/// Costs as much as a single `nix search` but holds the whole set in memory,
/// which is hundreds of megabytes.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] when the dump fails or exceeds the dump
/// timeout.
async fn nix_search_all() -> mx::Result<HashMap<String, RawPackage>> {
    nix_eval::run_json(&["search", "nixpkgs", "--json", "^"], DUMP_TIMEOUT).await
}

/// Best-effort mutual exclusion between concurrent builders of the same
/// index file: a lock file created with `O_EXCL`, removed on drop. Not a real
/// `flock` (no extra dependency for it), so a builder that crashes mid-build
/// leaves it behind — `try_acquire` steals a lock file older than
/// [`STALE_LOCK_AGE`] rather than wedging every future build forever.
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);

/// Holds the build lock of one index file for as long as it lives.
///
/// # Fields
/// * `path` - the lock file, removed on drop.
struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    /// Takes the build lock of an index file.
    ///
    /// # Parameters
    /// * `dest` - the index file about to be built; the lock sits next to it.
    ///
    /// # Returns
    /// The guard when the lock was taken, `None` when another builder holds it -
    /// in which case the caller must not build.
    ///
    /// # Post-conditions
    /// A lock file older than [`STALE_LOCK_AGE`] is stolen, on the assumption
    /// that its owner died; two builders can therefore legitimately run at once,
    /// which is why each writes its own temp file.
    fn try_acquire(dest: &Path) -> Option<Self> {
        let dir = dest.parent()?;
        std::fs::create_dir_all(dir).ok()?;
        let lock_path = dir.join(format!(".{}.lock", dest.file_name()?.to_str()?));

        let create = || {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
        };

        if create().is_ok() {
            return Some(Self { path: lock_path });
        }

        let stale = std::fs::metadata(&lock_path)
            .and_then(|m| m.modified())
            .is_ok_and(|m| m.elapsed().is_ok_and(|age| age > STALE_LOCK_AGE));
        if stale {
            let _ = std::fs::remove_file(&lock_path);
            if create().is_ok() {
                return Some(Self { path: lock_path });
            }
        }
        None
    }
}

impl Drop for LockGuard {
    /// Releases the build lock by deleting its file.
    ///
    /// # Post-conditions
    /// A failure to delete is ignored: the file then ages out as a stale lock.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Appends a string to the arena.
///
/// # Parameters
/// * `arena` - the string arena being built.
/// * `s` - the string to append, stored without a terminator.
///
/// # Returns
/// Its `(offset, length)` inside the arena, which is what a row stores.
fn push_str(arena: &mut Vec<u8>, s: &str) -> (u32, u32) {
    let off = arena.len() as u32;
    arena.extend_from_slice(s.as_bytes());
    (off, s.len() as u32)
}

/// Lays a whole dump out in the on-disk index format.
///
/// # Parameters
/// * `fingerprint` - nixpkgs fingerprint to stamp in the header, which is what
///   a reader validates freshness against.
/// * `raw` - the dump, keyed by full attribute path.
///
/// # Returns
/// The complete file: header, row table, then string arena. Attribute paths are
/// stored stripped of their `legacyPackages.<system>.` prefix, the lowercased
/// forms are precomputed, and the plugin namespaces are left out - they are
/// surfaced through modules, never as packages.
fn serialize(fingerprint: [u8; 32], raw: HashMap<String, RawPackage>) -> Vec<u8> {
    let prefix = format!("legacyPackages.{}.", env!("TARGET_NIX"));
    let mut arena = Vec::new();
    let mut row_bytes = Vec::with_capacity(raw.len() * ROW_SIZE);
    let mut count: u32 = 0;

    for (key, value) in raw {
        let name = key.strip_prefix(&prefix).unwrap_or(&key);
        if PLUGIN_NAMESPACE_PREFIXES
            .iter()
            .any(|ns| name.starts_with(ns))
        {
            continue;
        }

        let attr_lc = name.to_lowercase();
        let desc_lc = value.description.to_lowercase();

        for (o, l) in [
            push_str(&mut arena, name),
            push_str(&mut arena, &value.pname),
            push_str(&mut arena, &value.version),
            push_str(&mut arena, &value.description),
            push_str(&mut arena, &attr_lc),
            push_str(&mut arena, &desc_lc),
        ] {
            row_bytes.extend_from_slice(&o.to_le_bytes());
            row_bytes.extend_from_slice(&l.to_le_bytes());
        }
        count += 1;
    }

    let system = env!("TARGET_NIX").as_bytes();
    let mut out = Vec::with_capacity(48 + row_bytes.len() + arena.len());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&fingerprint);
    out.extend_from_slice(&(system.len() as u32).to_le_bytes());
    out.extend_from_slice(system);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&row_bytes);
    out.extend_from_slice(&arena);
    out
}

/// Disambiguates temp files written by two builders inside the same process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Publishes an index file without ever exposing a partial one.
///
/// # Parameters
/// * `dest` - final path of the index.
/// * `bytes` - the serialised file.
///
/// # Post-conditions
/// The bytes go to a temp file unique to this builder - `LockGuard::try_acquire`
/// deliberately steals a stale lock, so two builders can legitimately run at
/// once; sharing one fixed temp path would let them interleave their writes
/// into the same file, and the renamed result would carry a valid header over
/// another build's arena, i.e. wrong names and descriptions rather than a
/// detectable corruption. The temp file is then `rename`d (not written in
/// place) over `dest`, so a reader with the old file already mmap'd keeps a
/// valid, unchanged view - see `reader::Index::open` - and any other reader
/// either keeps the previous file mapped or sees the new one whole. Missing
/// parent directories are created.
///
/// # Errors
/// [`mx::ErrorKind::InvalidArgument`] when `dest` has no parent directory, and
/// [`mx::ErrorKind::IOError`] when the directory, the write or the rename fails -
/// which is also how a caller without write access to the cache directory finds
/// out.
fn write_atomically(dest: &Path, bytes: &[u8]) -> mx::Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| mx::ErrorKind::InvalidArgument("index path has no parent".to_string()))?;
    std::fs::create_dir_all(dir).map_err(mx::ErrorKind::IOError)?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("index"),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes).map_err(mx::ErrorKind::IOError)?;
    std::fs::rename(&tmp, dest).map_err(mx::ErrorKind::IOError)
}

/// Builds a fresh index at `dest`. A concurrent build already in progress
/// (same or another process) makes this a no-op success — the in-flight
/// build will produce the file soon enough.
pub(super) async fn build(dest: &Path) -> mx::Result<()> {
    let Some(_guard) = LockGuard::try_acquire(dest) else {
        return Ok(());
    };

    let fingerprint = current_fingerprint().await?;
    let raw = nix_search_all().await?;
    let bytes = serialize(fingerprint, raw);
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || write_atomically(&dest, &bytes))
        .await
        .map_err(|e| mx::ErrorKind::NixCommandError(format!("index write task panicked: {e}")))?
}
