use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::core::app_info_trait::PLUGIN_NAMESPACE_PREFIXES;
use crate::core::nix_eval;
use crate::mx;

use super::{FORMAT_VERSION, MAGIC};

/// `nix search nixpkgs --json '^'` measured ~1.7s warm, ~20s cold (first-ever
/// eval-cache build) — generous headroom over the worst case.
const DUMP_TIMEOUT: Duration = Duration::from_secs(60);
const METADATA_TIMEOUT: Duration = Duration::from_secs(20);

/// `off, len` pairs per row, in field order (see `reader::RowView`).
const ROW_FIELDS: usize = 6;
const ROW_SIZE: usize = ROW_FIELDS * 8;

#[derive(Deserialize)]
struct RawPackage {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct FlakeMetadata {
    fingerprint: String,
}

/// The nixpkgs flake fingerprint, packed into a fixed 32-byte field (truncated
/// or zero-padded) so the on-disk header has a stable size. Only ever compared
/// for equality — never decoded — so this lossy packing is safe.
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

async fn nix_search_all() -> mx::Result<HashMap<String, RawPackage>> {
    nix_eval::run_json(&["search", "nixpkgs", "--json", "^"], DUMP_TIMEOUT).await
}

/// Best-effort mutual exclusion between concurrent builders of the same
/// index file: a lock file created with `O_EXCL`, removed on drop. Not a real
/// `flock` (no extra dependency for it), so a builder that crashes mid-build
/// leaves it behind — `try_acquire` steals a lock file older than
/// [`STALE_LOCK_AGE`] rather than wedging every future build forever.
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);

struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
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
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn push_str(arena: &mut Vec<u8>, s: &str) -> (u32, u32) {
    let off = arena.len() as u32;
    arena.extend_from_slice(s.as_bytes());
    (off, s.len() as u32)
}

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

fn write_atomically(dest: &Path, bytes: &[u8]) -> mx::Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| mx::ErrorKind::InvalidArgument("index path has no parent".to_string()))?;
    std::fs::create_dir_all(dir).map_err(mx::ErrorKind::IOError)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("index")
    ));
    std::fs::write(&tmp, bytes).map_err(mx::ErrorKind::IOError)?;
    // `rename` (not an in-place write) so a reader with the old file already
    // mmap'd keeps a valid, unchanged view — see `reader::Index::open`.
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
