use crate::{CONFIG_DIRECTORY, mx};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
};

use nix::libc;
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

pub struct NixFile {
    file: Option<fs::File>,
    path: String,
    file_content: String,
    was_created: bool,
}

impl NixFile {
    pub fn new(path: &str) -> Self {
        NixFile {
            file: None,
            path: CONFIG_DIRECTORY.to_owned() + path,
            file_content: String::new(),
            was_created: false,
        }
    }

    const EXT2_IMMUTABLE_FL: libc::c_long = 0x00000010;
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

            flags |= Self::EXT2_IMMUTABLE_FL; // active le bit immutable

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

            flags &= !Self::EXT2_IMMUTABLE_FL; // désactive le bit immutable

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

    pub fn get_file_path(&self) -> &str {
        return &self.path;
    }

    pub fn get_mut_file_content(&mut self) -> mx::Result<&mut String> {
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

    pub(super) fn begin(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
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

            self.file = Some(
                File::options()
                    .create(false)
                    .read(true)
                    .write(true)
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

    pub(super) fn commit(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
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
        Ok(())
    }

    pub(super) fn close(&mut self) -> mx::Result<()> {
        #[allow(unused_must_use)]
        self.file.as_ref().unwrap().unlock();
        self.file_content = String::new();
        self.file = None;
        Ok(())
    }
}
