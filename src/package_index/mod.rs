//! On-disk, mmap'd index of the whole nixpkgs package set.
//!
//! `nix search` spawns a subprocess and opens nix's eval-cache on every call —
//! ~1.3-1.7s warm, ~20s cold — regardless of how narrow the query is. Dumping
//! all of nixpkgs once (`nix search nixpkgs --json '^'`) costs the same as one
//! query, so this module builds that dump into a single mmap'd file and serves
//! every subsequent search out of it in-process, in the low milliseconds.
//!
//! `get` returns the current index if a fresh one is on disk; `Index::open`
//! validates the nixpkgs fingerprint and target system before trusting it, so
//! a stale or foreign-arch file is never served. [`ensure_fresh_in_background`]
//! is the only entry point that builds/rebuilds the file — callers on the
//! interactive path never block on it, they just fall back to a live `nix
//! search` for as long as `get` returns `None`.

mod build;
mod reader;
mod search;

use std::sync::{Arc, OnceLock, RwLock};

use crate::mx;

pub(crate) use reader::Index;
pub(crate) use search::search;

/// Magic number every index file starts with, so a foreign file is rejected
/// before anything else is read.
pub(crate) const MAGIC: u32 = u32::from_le_bytes(*b"MXPI");

/// On-disk layout version; a file written by another version is not read.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// File name of the single system-wide index (phase 1.6: owned and rebuilt by
/// the daemon, shared by every user — no per-user copy). It lives in
/// [`crate::cache_dir`], alongside the module index.
const SYSTEM_CACHE_FILE: &str = "nix-index-v1.bin";

/// Path of the system-wide index file.
///
/// # Returns
/// [`SYSTEM_CACHE_FILE`] inside [`crate::cache_dir`], whether it exists or not.
fn system_cache_path() -> std::path::PathBuf {
    crate::cache_dir().join(SYSTEM_CACHE_FILE)
}

/// The index this process currently serves, `None` until one is loaded.
static SLOT: OnceLock<RwLock<Option<Arc<Index>>>> = OnceLock::new();

/// The nixpkgs fingerprint the loaded index was validated against.
static FINGERPRINT: RwLock<Option<[u8; 32]>> = RwLock::new(None);

/// The process-wide index slot, created on first access.
///
/// # Returns
/// The lock guarding the currently loaded index.
fn slot() -> &'static RwLock<Option<Arc<Index>>> {
    SLOT.get_or_init(|| RwLock::new(None))
}

/// The nixpkgs flake fingerprint, fetched once per process (`nix flake
/// metadata`, ~70ms) and cached until [`invalidate_fingerprint`] clears it.
///
/// # Returns
/// The fingerprint identifying the nixpkgs revision in use, or `None` when it
/// cannot be computed - `nix` missing, offline, or an unresolvable registry
/// entry.
async fn fingerprint() -> Option<[u8; 32]> {
    if let Some(fp) = *FINGERPRINT.read().unwrap() {
        return Some(fp);
    }
    let fp = build::current_fingerprint().await.ok()?;
    *FINGERPRINT.write().unwrap() = Some(fp);
    Some(fp)
}

/// Forces the next `get` call to recompute the nixpkgs fingerprint and
/// reload the index from disk, instead of serving whatever was cached at
/// process start. A long-lived daemon must call this after a rebuild or
/// index refresh, or it keeps serving a stale index until restarted.
///
/// # Post-conditions
/// Both the cached fingerprint and the loaded index are dropped, so the next
/// `get` recomputes and reloads. Searches already in flight keep using the
/// index they hold - it is reference-counted, not invalidated under them.
pub fn invalidate_fingerprint() {
    *FINGERPRINT.write().unwrap() = None;
    *slot().write().unwrap() = None;
}

/// Loads the on-disk index if it matches a given fingerprint.
///
/// # Parameters
/// * `fingerprint` - the nixpkgs fingerprint the file must carry.
///
/// # Returns
/// The mmap'd index, or `None` when the file is absent, stale, of a foreign
/// target system, or corrupt.
fn try_load(fingerprint: &[u8; 32]) -> Option<Arc<Index>> {
    Index::open(&system_cache_path(), fingerprint).map(Arc::new)
}

/// The mmap'd package index, if a fresh one is available on disk. `None`
/// means absent, stale (fingerprint/system mismatch) or corrupt — callers
/// fall back to a live `nix search` for this call.
///
/// # Returns
/// A handle on the mmap'd index, shared between callers, or `None` when none is
/// servable.
///
/// # Post-conditions
/// The first successful call memoises the index for the process; later calls are
/// a lock read. Never builds anything - see [`ensure_fresh_in_background`] and
/// [`build_system_index`] for that.
pub(crate) async fn get() -> Option<Arc<Index>> {
    if let Some(index) = slot().read().unwrap().clone() {
        return Some(index);
    }
    let fp = fingerprint().await?;
    let index = try_load(&fp)?;
    *slot().write().unwrap() = Some(index.clone());
    Some(index)
}

/// Whether a fresh index is currently servable — the daemon exposes this as
/// its `IndexReady` D-Bus property.
///
/// # Returns
/// `true` when a search would be served from the index, `false` when it would
/// fall back to a live `nix search`.
pub async fn is_ready() -> bool {
    get().await.is_some()
}

/// Build (or rebuild) the on-disk index if `get` currently has nothing to
/// serve, then make it immediately visible to this process — no restart
/// needed. Best-effort: errors are logged, never propagated, since the only
/// caller is a detached warm-up task racing nothing.
///
/// # Post-conditions
/// A no-op when an index is already servable. Otherwise the whole nixpkgs set is
/// dumped, which costs minutes and a lot of memory, and the fresh index becomes
/// visible to this process on success. A failure is printed on stderr and leaves
/// callers on the `nix search` fallback.
pub async fn ensure_fresh_in_background() {
    if get().await.is_some() {
        return;
    }
    if let Err(e) = build_system_index().await {
        eprintln!("[package_index] build failed: {e}");
        return;
    }
    let Some(fp) = fingerprint().await else {
        return;
    };
    if let Some(index) = Index::open(&system_cache_path(), &fp) {
        *slot().write().unwrap() = Some(Arc::new(index));
    }
}

/// Builds the system-wide index at `system_cache_path` unconditionally —
/// the entry point for the daemon's startup/timer/rebuild-signal paths, not
/// for interactive callers (which want [`ensure_fresh_in_background`]'s
/// "only if missing" and immediate-visibility behaviour instead).
///
/// # Pre-conditions
/// `nix` on `PATH` with flakes enabled, network access on a cold cache, and
/// write access to [`crate::cache_dir`], which the daemon has and a user does
/// not.
///
/// # Post-conditions
/// The file is written to a temporary path and renamed into place, so a
/// concurrent reader either keeps the old index or sees the new one, never a
/// half-written file. The new index is *not* made visible to this process; call
/// [`invalidate_fingerprint`] for that.
///
/// # Errors
/// Any error from dumping nixpkgs or writing the file, notably
/// [`mx::ErrorKind::NixCommandError`] and [`mx::ErrorKind::IOError`].
pub async fn build_system_index() -> mx::Result<()> {
    build::build(&system_cache_path()).await
}
