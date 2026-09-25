//! [`NixFile`]: one configuration file opened under an exclusive lock, edited
//! in memory, and rewritten in a single pass at commit time.

use crate::mx;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
};

use nix::libc;
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

/// What a transaction is allowed to do with a file.
///
/// # Variants
/// * `ReadOnly` - the content can be read; any edit attempt is refused.
/// * `Writtable` - the content can be edited and written back at commit.
pub enum NixFilePermission {
    ReadOnly,
    Writtable,
}

impl From<&NixFilePermission> for bool {
    /// Reduces a permission to the writability bit [`NixFile`] stores.
    ///
    /// # Parameters
    /// * `p` - the permission to convert.
    ///
    /// # Returns
    /// `true` for `Writtable`, `false` for `ReadOnly`.
    fn from(p: &NixFilePermission) -> bool {
        matches!(p, NixFilePermission::Writtable)
    }
}

/// NixFile represents a file handle with atomic commit semantics.
///
/// This struct enables atomic manipulation of files by writing all modifications
/// only at commit time. It handles files marked as immutable using EXT2 flags,
/// and enforces exclusive file locking: a lock is acquired at begin() and released at commit().
/// The object can only be instantiated through a transaction context to maintain
/// file system consistency and prevent partial writes.
///
/// # Atomicity Guarantee
/// All content modifications accumulated during the transaction are written to disk
/// atomically only when commit() is called. This prevents partial writes from being
/// visible to other processes.
///
/// # Immutable File Support
/// After successful commit(), files are marked with the immutable flag to prevent
/// accidental modifications or deletions. This is particularly important for NixOS
/// configuration files that must remain stable between rebuilds.
///
/// # Fields
/// * `file` - the open handle, holding the exclusive lock; `None` outside an
///   open transaction, which is what every method checks first.
/// * `path` - absolute path of the file.
/// * `file_content` - edit buffer, holding the whole file while it is open.
/// * `was_created` - whether `begin` had to create the file, so a rollback
///   knows it must delete it rather than restore it.
/// * `writable` - whether edits are allowed, from the permission `begin` got.
pub struct NixFile {
    file: Option<fs::File>,

    path: String,

    file_content: String,

    was_created: bool,

    writable: bool,
}

impl NixFile {
    /// Creates a new NixFile instance associated with the given repository path
    /// and relative file path.
    ///
    /// This constructor can only be used within an active transaction context.
    /// The returned object will participate in the current transaction and support
    /// atomic file modifications via begin()/commit()/close() lifecycle.
    ///
    /// # Arguments
    /// * `repo_path` - The root path of the NixOS repository (e.g., `/etc/nixos`)
    /// * `relative_path` - The relative path to the file within the repository
    ///
    /// # Returns
    /// A new NixFile instance initialized with the provided paths.
    pub fn new(repo_path: &str, relative_path: &str) -> Self {
        NixFile {
            file: None,
            path: String::from(repo_path) + relative_path,
            file_content: String::new(),
            was_created: false,
            writable: false,
        }
    }

    /// Bit of the ext2 inode flags that marks a file immutable.
    const FS_IMMUTABLE_FL: libc::c_long = 0x00000010;

    /// `ioctl` request reading an inode's ext2 flags.
    const FS_IOC_GETFLAGS: libc::c_ulong = 0x80086601;

    /// `ioctl` request writing an inode's ext2 flags.
    const FS_IOC_SETFLAGS: libc::c_ulong = 0x40086602;

    /// Tells whether a path belongs to root.
    ///
    /// # Parameters
    /// * `path` - the file to inspect.
    ///
    /// # Returns
    /// `true` when the owner's uid is 0. The immutable flag is only ever
    /// touched on such files, so the whole mechanism no-ops on a development
    /// fixture owned by a normal user.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the path cannot be stat'ed.
    fn is_owned_by_root(path: &str) -> mx::Result<bool> {
        let metadata = std::fs::metadata(path).map_err(mx::ErrorKind::IOError)?;
        Ok(metadata.uid() == 0)
    }

    /// Reads a file's ext2 inode flags.
    ///
    /// # Parameters
    /// * `path` - the file to inspect; it is opened read-only for the duration
    ///   of the call.
    ///
    /// # Returns
    /// The raw flag word, to be tested against [`NixFile::FS_IMMUTABLE_FL`].
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the file cannot be opened, and
    /// [`mx::ErrorKind::UnixError`] if the `ioctl` fails - notably on a
    /// filesystem that does not support these flags.
    fn get_flags(path: &str) -> mx::Result<libc::c_long> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(mx::ErrorKind::IOError)?;
        let fd = file.as_raw_fd();
        let mut flags: libc::c_long = 0;

        // SAFETY: `fd` is owned by `file`, which outlives the call, and
        // `FS_IOC_GETFLAGS` writes exactly one `c_long` into the pointer it is
        // given, which is what `&mut flags` provides.
        unsafe {
            if libc::ioctl(fd, Self::FS_IOC_GETFLAGS, &mut flags) < 0 {
                return Err(mx::ErrorKind::UnixError(nix::Error::last()));
            }
        }
        Ok(flags)
    }

    /// Reports whether the immutable flag is currently set on `path`.
    ///
    /// Callers that clear the flag around an external write use this to restore
    /// the *previous* state instead of unconditionally sealing the file: a
    /// config repo not produced by `init` may legitimately keep `flake.lock`
    /// writable, and sealing it would break the admin's own `nix flake update`.
    ///
    /// # Parameters
    /// * `path` - the file to inspect.
    ///
    /// # Returns
    /// `true` when the immutable flag is set.
    ///
    /// # Errors
    /// As in [`NixFile::get_flags`].
    pub(super) fn is_immutable(path: &str) -> mx::Result<bool> {
        Ok(Self::get_flags(path)? & Self::FS_IMMUTABLE_FL != 0)
    }

    /// Seals a file by setting its immutable flag, so nothing can modify or
    /// delete it - not even root - until the flag is cleared again.
    ///
    /// # Parameters
    /// * `path` - the file to seal.
    ///
    /// # Post-conditions
    /// No-op unless the file belongs to root, which keeps development fixtures
    /// untouched. The other flags of the inode are preserved.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the file cannot be stat'ed or opened, and
    /// [`mx::ErrorKind::UnixError`] if either `ioctl` fails.
    pub(crate) fn make_immutable(path: &str) -> mx::Result<()> {
        if Self::is_owned_by_root(path)? {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(mx::ErrorKind::IOError)?;
            let fd = file.as_raw_fd();
            let mut flags = Self::get_flags(path)?;

            flags |= Self::FS_IMMUTABLE_FL;

            unsafe {
                if libc::ioctl(fd, Self::FS_IOC_SETFLAGS, &flags) < 0 {
                    return Err(mx::ErrorKind::UnixError(nix::Error::last()));
                }
            }
        }
        Ok(())
    }

    /// Clears a file's immutable flag so it can be written again.
    ///
    /// # Parameters
    /// * `path` - the file to unseal.
    ///
    /// # Post-conditions
    /// No-op unless the file belongs to root. The caller is responsible for
    /// re-sealing it (commit, close and rollback all do).
    ///
    /// # Errors
    /// As in [`NixFile::make_immutable`].
    pub(super) fn make_mutable(path: &str) -> mx::Result<()> {
        if Self::is_owned_by_root(path)? {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(mx::ErrorKind::IOError)?;
            let fd = file.as_raw_fd();
            let mut flags = Self::get_flags(path)?;

            flags &= !Self::FS_IMMUTABLE_FL;

            unsafe {
                if libc::ioctl(fd, Self::FS_IOC_SETFLAGS, &flags) < 0 {
                    return Err(mx::ErrorKind::UnixError(nix::Error::last()));
                }
            }
        }
        Ok(())
    }

    /// Creates the file with an empty NixOS module as content, for a
    /// transaction that targets a file the configuration does not have yet.
    ///
    /// # Post-conditions
    /// The file holds the stub `{config, lib, pkgs, ...}:\n{\n}\n`, is sealed
    /// immutable, and is flagged as created so a rollback deletes it instead of
    /// restoring it. An existing file is truncated and replaced by the stub.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the file cannot be created or written,
    /// plus any error from sealing it.
    pub(super) fn create_file(&mut self) -> mx::Result<()> {
        let mut file = fs::File::create(&self.path).map_err(mx::ErrorKind::IOError)?;
        file.write_all("{config, lib, pkgs, ...}:\n{\n}\n".as_bytes())
            .map_err(mx::ErrorKind::IOError)?;
        self.was_created = true;
        Self::make_immutable(&self.path)?;
        Ok(())
    }

    /// Whether this file was created by the current transaction.
    ///
    /// # Returns
    /// `true` when [`NixFile::create_file`] made it, which is what tells a
    /// rollback to delete the file rather than check it out again.
    pub fn was_created(&self) -> bool {
        self.was_created
    }

    /// Absolute path of the file.
    ///
    /// # Returns
    /// The repository path and the relative path joined at construction,
    /// borrowed from `self`. Available whether the file is open or not.
    pub fn get_file_path(&self) -> &str {
        return &self.path;
    }

    /// Borrows the edit buffer for modification: the only way to change the
    /// file's content.
    ///
    /// # Returns
    /// The whole content, mutable. Edits stay in memory until the transaction
    /// commits, so nothing on disk changes here.
    ///
    /// # Errors
    /// [`mx::ErrorKind::PermissionDenied`] on a read-only file, and
    /// [`mx::ErrorKind::TransactionNotBegin`] when the file is not open.
    pub fn get_mut_file_content(&mut self) -> mx::Result<&mut String> {
        if !self.writable {
            return Err(mx::ErrorKind::PermissionDenied);
        }
        if self.file.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        Ok(&mut self.file_content)
    }

    /// Borrows the edit buffer for reading, including the edits made so far.
    ///
    /// # Returns
    /// The whole content as loaded by `begin` and modified since.
    ///
    /// # Errors
    /// [`mx::ErrorKind::TransactionNotBegin`] when the file is not open. A
    /// read-only file is fine here.
    pub fn get_file_content(&self) -> mx::Result<&String> {
        if self.file.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        Ok(&self.file_content)
    }

    /// Begins a transaction on the file by making it mutable, acquiring an
    /// exclusive lock, and loading its content into memory.
    ///
    /// This method:
    /// - Removes the immutable flag if present (making the file writable)
    /// - Opens the file in read-write mode
    /// - Acquires an exclusive lock on the file
    /// - Reads the entire file content into memory
    ///
    /// Only one transaction can be active at a time. If a transaction is already
    /// in progress, this call has no effect.
    ///
    /// # Errors
    /// * `mx::ErrorKind::FileNotFound` - The file does not exist
    /// * `mx::ErrorKind::PermissionDenied` - Insufficient permissions
    /// * `mx::ErrorKind::FailToLock` - Failed to acquire exclusive file lock
    ///
    /// # Arguments
    /// * `permission` - whether the file may be edited; a read-only open leaves
    ///   the immutable flag alone and opens without write access.
    ///
    /// # Post-conditions
    /// Blocks until the exclusive lock is free, so a concurrent transaction on
    /// the same file makes this wait. The lock is held until `commit` or
    /// `close`: an early return elsewhere in the transaction must still reach
    /// one of them, otherwise the lock leaks for the lifetime of the process.
    pub(super) fn begin(&mut self, permission: NixFilePermission) -> mx::Result<()> {
        self.writable = bool::from(&permission);
        if self.file.is_none() {
            if self.writable {
                match Self::make_mutable(&self.path) {
                    Ok(()) => (),
                    Err(e) => match e {
                        mx::ErrorKind::IOError(ioe) => match ioe.kind() {
                            io::ErrorKind::NotFound => {
                                return Err(mx::ErrorKind::FileNotFound(self.path.clone()));
                            }
                            _ => return Err(mx::ErrorKind::IOError(ioe)),
                        },
                        err => return Err(err),
                    },
                };
            }

            self.file = Some(
                File::options()
                    .create(false)
                    .read(true)
                    .write(self.writable)
                    .open(&self.path)
                    .map_err(|e| match e.kind() {
                        io::ErrorKind::PermissionDenied => mx::ErrorKind::PermissionDenied,
                        io::ErrorKind::NotFound => mx::ErrorKind::FileNotFound(self.path.clone()),
                        _ => mx::ErrorKind::IOError(e),
                    })?,
            )
        }

        if let Some(f) = self.file.as_mut() {
            f.lock().or(Err(mx::ErrorKind::FailToLock))?;
            f.read_to_string(&mut self.file_content)
                .map_err(mx::ErrorKind::IOError)?;
            Ok(())
        } else {
            Err(mx::ErrorKind::InvalidFile)
        }
    }

    /// Commits the transaction by atomically writing the modified content to disk,
    /// restoring the immutable flag, releasing the lock, and resetting the state.
    ///
    /// This method performs all operations atomically in this order:
    /// 1. Truncates the file to zero length
    /// 2. Writes the complete modified content
    /// 3. Restores the immutable flag
    /// 4. Releases the exclusive lock
    /// 5. Resets the internal state (clears content and file handle)
    ///
    /// Calling commit() ensures that all modifications are durably persisted
    /// before the file becomes immutable again. If the write fails, no partial
    /// changes are visible to other processes.
    ///
    /// Note: Unlike close(), commit() persists changes before releasing the file.
    /// close() only releases the lock without persisting content.
    ///
    /// # Errors
    /// * `mx::ErrorKind::InvalidFile` - No active transaction (file was already committed)
    /// * `mx::ErrorKind::PermissionDenied` - Failed to write to file
    ///
    /// # Post-conditions
    /// The file is closed, sealed immutable and unlocked, and the buffer is
    /// cleared: the handle is back to the state [`NixFile::new`] left it in, so
    /// another `begin` is possible. A read-only file cannot be committed.
    pub(super) fn commit(&mut self) -> mx::Result<()> {
        if self.file.is_none() || !self.writable {
            return Err(mx::ErrorKind::InvalidFile);
        }

        self.file
            .as_mut()
            .unwrap()
            .seek(io::SeekFrom::Start(0))
            .unwrap();
        self.file.as_ref().unwrap().set_len(0).unwrap();

        self.file
            .as_ref()
            .unwrap()
            .write_all(&self.file_content.as_bytes())
            .or(Err(mx::ErrorKind::PermissionDenied))?;

        Self::make_immutable(&self.path)?;
        self.file
            .as_ref()
            .unwrap()
            .unlock()
            .map_err(mx::ErrorKind::IOError)?;

        self.file_content = String::new();
        self.file = None;
        Ok(())
    }

    /// Closes the transaction by releasing the lock and resetting the state,
    /// without persisting any modifications to disk.
    ///
    /// This method:
    /// - Releases the exclusive lock on the file
    /// - Clears the in-memory content
    /// - Closes the file handle
    ///
    /// Unlike commit(), this method does NOT write the content back to disk or
    /// restore the immutable flag. Any modifications made during the transaction
    /// are discarded when close() is called. Use commit() to persist changes.
    ///
    /// This method always returns Ok(()) even if unlocking fails, as per the
    /// intentional design choice to avoid propagating lock-related errors.
    ///
    /// # Returns
    /// `Ok(())`, unless re-sealing a writable file fails - the one error that
    /// does propagate.
    ///
    /// # Post-conditions
    /// A writable file is sealed immutable again. Closing a file that was never
    /// opened is harmless.
    pub(super) fn close(&mut self) -> mx::Result<()> {
        if self.writable {
            Self::make_immutable(&self.path)?;
        }
        if let Some(f) = self.file.as_ref() {
            #[allow(unused_must_use)]
            f.unlock();
        }
        self.file_content = String::new();
        self.file = None;
        Ok(())
    }

    /// Deletes a configuration file, clearing its immutable flag first.
    ///
    /// # Parameters
    /// * `path` - the file to delete.
    ///
    /// # Post-conditions
    /// The file is gone. Used by a rollback to undo the files the transaction
    /// created; nothing checks that the path is not open elsewhere.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the file cannot be unsealed or removed,
    /// notably when it does not exist.
    pub(crate) fn delete(path: &str) -> mx::Result<()> {
        Self::make_mutable(path)?;
        fs::remove_file(path).map_err(mx::ErrorKind::IOError)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "file_lock_tests.rs"]
mod tests;
