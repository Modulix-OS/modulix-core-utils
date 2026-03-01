use crate::{CONFIG_DIRECTORY, mx};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
};

pub struct NixFile {
    file: Option<fs::File>,
    path: String,
    file_content: String,
}

impl NixFile {
    pub fn new(path: &str) -> Self {
        NixFile {
            file: None,
            path: CONFIG_DIRECTORY.to_owned() + path,
            file_content: String::new(),
        }
    }

    pub(super) fn create_file(&mut self) -> mx::Result<()> {
        let mut file = fs::File::create(&self.path).map_err(mx::ErrorKind::IOError)?;
        file.write_all("{config, lib, pkgs, ...}:\n{\n}\n".as_bytes())
            .map_err(mx::ErrorKind::IOError)?;
        Ok(())
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

    pub fn begin(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
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
