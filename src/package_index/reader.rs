//! Reading side of the package index: validating a file's header and borrowing
//! its rows straight out of the mapping.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use super::{FORMAT_VERSION, MAGIC};

/// `off, len` pairs per row: attr, pname, version, description, attr_lc, desc_lc.
const ROW_FIELDS: usize = 6;

/// Size of one row in the table, in bytes: [`ROW_FIELDS`] pairs of `u32`.
const ROW_SIZE: usize = ROW_FIELDS * 8;

/// One package row, borrowed straight out of the mmap — no allocation.
///
/// # Fields
/// * `attr` - the nixpkgs attribute path.
/// * `pname` - the package's `pname`.
/// * `version` - its version string.
/// * `description` - its `meta.description`.
/// * `attr_lc` - `attr` lowercased, precomputed so a search need not allocate.
/// * `desc_lc` - `description` lowercased, likewise.
#[derive(Clone, Copy)]
pub(crate) struct RowView<'a> {
    pub attr: &'a str,
    pub pname: &'a str,
    pub version: &'a str,
    pub description: &'a str,
    pub attr_lc: &'a str,
    pub desc_lc: &'a str,
}

/// A validated, mmap'd package index. See `mod.rs` for the on-disk layout.
///
/// # Fields
/// * `mmap` - the whole file, mapped read-only.
/// * `rows_start` - byte offset of the row table.
/// * `arena_start` - byte offset of the string arena the rows point into.
/// * `count` - number of rows.
pub(crate) struct Index {
    mmap: Mmap,
    rows_start: usize,
    arena_start: usize,
    count: u32,
}

impl Index {
    /// Opens `path` and validates it against `expected_fingerprint` and the
    /// build's target system. Returns `None` on any mismatch, I/O error or
    /// truncated/corrupt file — never partially-trusts a bad file.
    ///
    /// # Parameters
    /// * `path` - the index file to map.
    /// * `expected_fingerprint` - nixpkgs fingerprint the file must carry for it
    ///   to be considered fresh.
    ///
    /// # Returns
    /// The validated index, or `None` on any mismatch or error.
    ///
    /// # Post-conditions
    /// The mapping stays valid for the lifetime of the value even if the file is
    /// replaced afterwards, the writer always renaming a new inode into place.
    pub(crate) fn open(path: &Path, expected_fingerprint: &[u8; 32]) -> Option<Self> {
        let file = File::open(path).ok()?;
        // SAFETY: the writer (`build::build`) never mutates a published index
        // in place — it always writes a temp file and `rename`s it into
        // place — so this mmap keeps referring to a stable, immutable inode
        // for its whole lifetime even if the file at `path` is later replaced.
        let mmap = unsafe { Mmap::map(&file) }.ok()?;
        Self::parse(mmap, expected_fingerprint)
    }

    /// Validates a mapping's header and locates its sections.
    ///
    /// # Parameters
    /// * `mmap` - the mapped file.
    /// * `expected_fingerprint` - fingerprint the header must carry.
    ///
    /// # Returns
    /// The index, or `None` when the magic, the format version, the fingerprint
    /// or the target system does not match, or when the file is too short for the
    /// row count it declares.
    fn parse(mmap: Mmap, expected_fingerprint: &[u8; 32]) -> Option<Self> {
        let buf: &[u8] = &mmap;
        let magic = u32::from_le_bytes(buf.get(0..4)?.try_into().ok()?);
        if magic != MAGIC {
            return None;
        }
        let version = u32::from_le_bytes(buf.get(4..8)?.try_into().ok()?);
        if version != FORMAT_VERSION {
            return None;
        }
        let fingerprint = buf.get(8..40)?;
        if fingerprint != expected_fingerprint {
            return None;
        }

        let mut off = 40usize;
        let system_len = u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?) as usize;
        off += 4;
        let system = buf.get(off..off + system_len)?;
        if system != env!("TARGET_NIX").as_bytes() {
            return None;
        }
        off += system_len;

        let count = u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?);
        off += 4;

        let rows_start = off;
        let arena_start = rows_start.checked_add((count as usize).checked_mul(ROW_SIZE)?)?;
        if arena_start > buf.len() {
            return None;
        }

        Some(Self {
            mmap,
            rows_start,
            arena_start,
            count,
        })
    }

    /// Number of packages in the index.
    ///
    /// # Returns
    /// The row count from the header, i.e. the exclusive upper bound on the
    /// indices [`Index::row`] accepts.
    pub(crate) fn len(&self) -> usize {
        self.count as usize
    }

    /// Row `i`, or `None` if the offsets it stores fall outside the arena —
    /// treated as "skip this row" by the caller rather than a hard error,
    /// since the header/count were already validated at open time.
    ///
    /// # Parameters
    /// * `i` - row index, below [`Index::len`].
    ///
    /// # Returns
    /// The row's fields, borrowed from the mapping without allocating, or `None`
    /// when `i` is out of range or the row's offsets or bytes are unusable.
    pub(crate) fn row(&self, i: usize) -> Option<RowView<'_>> {
        if i >= self.count as usize {
            return None;
        }
        let buf: &[u8] = &self.mmap;
        let row_off = self.rows_start + i * ROW_SIZE;
        let row = buf.get(row_off..row_off + ROW_SIZE)?;

        let mut fields = [(0u32, 0u32); ROW_FIELDS];
        let (chunks, _) = row.as_chunks::<8>();
        for (field, chunk) in fields.iter_mut().zip(chunks) {
            let o = u32::from_le_bytes(chunk[0..4].try_into().ok()?);
            let l = u32::from_le_bytes(chunk[4..8].try_into().ok()?);
            *field = (o, l);
        }

        let str_at = |(o, l): (u32, u32)| -> Option<&str> {
            let start = self.arena_start.checked_add(o as usize)?;
            let end = start.checked_add(l as usize)?;
            std::str::from_utf8(buf.get(start..end)?).ok()
        };

        Some(RowView {
            attr: str_at(fields[0])?,
            pname: str_at(fields[1])?,
            version: str_at(fields[2])?,
            description: str_at(fields[3])?,
            attr_lc: str_at(fields[4])?,
            desc_lc: str_at(fields[5])?,
        })
    }
}
