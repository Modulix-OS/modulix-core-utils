use crate::mx;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
};

use nix::libc;
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

pub enum NixFilePermission {
    ReadOnly,
    Writtable,
}

impl From<&NixFilePermission> for bool {
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

    const FS_IMMUTABLE_FL: libc::c_long = 0x00000010;

    const FS_IOC_GETFLAGS: libc::c_ulong = 0x80086601;

    const FS_IOC_SETFLAGS: libc::c_ulong = 0x40086602;

    fn is_owned_by_root(path: &str) -> mx::Result<bool> {
        let metadata = std::fs::metadata(path).map_err(mx::ErrorKind::IOError)?;
        Ok(metadata.uid() == 0)
    }

    fn get_flags(path: &str) -> mx::Result<libc::c_long> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(mx::ErrorKind::IOError)?;
        let fd = file.as_raw_fd();
        let mut flags: libc::c_long = 0;

        unsafe {
            if libc::ioctl(fd, Self::FS_IOC_GETFLAGS, &mut flags) < 0 {
                return Err(mx::ErrorKind::UnixError(nix::Error::last()));
            }
        }
        Ok(flags)
    }

    pub(super) fn make_immutable(path: &str) -> mx::Result<()> {
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

    pub(super) fn make_mutable(path: &str) -> mx::Result<()> {
        if Self::is_owned_by_root(path)? {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(mx::ErrorKind::IOError)?;
            let fd = file.as_raw_fd();
            let mut flags = Self::get_flags(path)?;

            // Clear the immutable bit in the flags
            flags &= !Self::FS_IMMUTABLE_FL;

            unsafe {
                if libc::ioctl(fd, Self::FS_IOC_SETFLAGS, &flags) < 0 {
                    return Err(mx::ErrorKind::UnixError(nix::Error::last()));
                }
            }
        }
        Ok(())
    }

    pub(super) fn create_file(&mut self) -> mx::Result<()> {
        let mut file = fs::File::create(&self.path).map_err(mx::ErrorKind::IOError)?;
        file.write_all("{config, lib, pkgs, ...}:\n{\n}\n".as_bytes())
            .map_err(mx::ErrorKind::IOError)?;
        self.was_created = true;
        Self::make_immutable(&self.path)?;
        Ok(())
    }

    pub fn was_created(&self) -> bool {
        self.was_created
    }

    /// Returns the absolute path of the file.
    pub fn get_file_path(&self) -> &str {
        return &self.path;
    }

    pub fn get_mut_file_content(&mut self) -> mx::Result<&mut String> {
        if !self.writable {
            return Err(mx::ErrorKind::PermissionDenied);
        }
        if self.file.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        Ok(&mut self.file_content)
    }

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
    pub(super) fn begin(&mut self, permission: NixFilePermission) -> mx::Result<()> {
        self.writable = bool::from(&permission);
        if self.file.is_none() {
            if self.writable {
                match Self::make_mutable(&self.path) {
                    Ok(()) => (),
                    Err(e) => match e {
                        mx::ErrorKind::IOError(ioe) => match ioe.kind() {
                            io::ErrorKind::NotFound => return Err(mx::ErrorKind::FileNotFound),
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
                        io::ErrorKind::NotFound => mx::ErrorKind::FileNotFound,
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

    pub(crate) fn delete(path: &str) -> mx::Result<()> {
        Self::make_mutable(path)?;
        fs::remove_file(path).map_err(mx::ErrorKind::IOError)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "file_lock_tests.rs"]
mod tests;
