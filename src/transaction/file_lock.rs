use crate::{mx, transaction::Transaction};
use std::hash::{Hash, Hasher};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
};

pub struct NixFile {
    file: Option<fs::File>,
    path: String,
    file_content: String,
    file_content_old: String,
}

impl NixFile {
    pub fn new(path: &str) -> Self {
        NixFile {
            file: None,
            path: path.to_string(),
            file_content: String::new(),
            file_content_old: String::new(),
        }
    }

    pub fn get_file_path(&self) -> &str {
        return &self.path;
    }

    pub(super) fn get_mut_file_content(&mut self) -> mx::Result<&mut String> {
        if self.file.is_none() {
            return Err(mx::ErrorType::TransactionNotBegin);
        }
        Ok(&mut self.file_content)
    }

    pub(super) fn get_file_content(&self) -> mx::Result<&String> {
        if self.file.is_none() {
            return Err(mx::ErrorType::TransactionNotBegin);
        }
        Ok(&self.file_content)
    }

    pub fn attach_on_transaction<'a>(
        &'a mut self,
        transaction: &mut Transaction<'a>,
    ) -> mx::Result<()> {
        if !transaction.as_begin() {
            return Err(mx::ErrorType::TransactionNotBegin);
        }
        if self.file.is_none() {
            self.file = Some(
                match File::options()
                    .create(false)
                    .read(true)
                    .write(true)
                    .open(&self.path)
                {
                    Ok(f) => f,
                    Err(e) => match e.kind() {
                        io::ErrorKind::PermissionDenied => {
                            return Err(mx::ErrorType::PermissionDenied);
                        }
                        io::ErrorKind::NotFound => return Err(mx::ErrorType::FileNotFound),
                        _ => return Err(mx::ErrorType::IOError(e)),
                    },
                },
            )
        }

        if let Some(f) = self.file.as_mut() {
            f.lock().or(Err(mx::ErrorType::FailToLock))?;
            match f.read_to_string(&mut self.file_content) {
                Ok(_) => (),
                Err(e) => return Err(mx::ErrorType::IOError(e)),
            };
            match f.read_to_string(&mut self.file_content_old) {
                Ok(_) => (),
                Err(e) => return Err(mx::ErrorType::IOError(e)),
            };
            transaction.add_file(&mut *self);
            Ok(())
        } else {
            Err(mx::ErrorType::InvalidFile)
        }
    }

    pub(super) fn commit(&mut self) -> mx::Result<()> {
        if self.file.is_none() {
            return Err(mx::ErrorType::InvalidFile);
        }

        self.file
            .as_ref()
            .unwrap()
            .write(&self.file_content.as_bytes())
            .or(Err(mx::ErrorType::PermissionDenied))?;
        match self.file.as_ref().unwrap().unlock() {
            Ok(_) => (),
            Err(e) => return Err(mx::ErrorType::IOError(e)),
        }
        Ok(())
    }

    pub(super) fn rollback(&mut self) -> mx::Result<()> {
        self.file_content = String::new();
        match self
            .file
            .as_mut()
            .unwrap()
            .write(&self.file_content_old.as_bytes())
        {
            Ok(_) => Ok(()),
            Err(e) => return Err(mx::ErrorType::IOError(e)),
        }
    }

    pub(super) fn close(&mut self) -> mx::Result<()> {
        #[allow(unused_must_use)]
        self.file.as_ref().unwrap().unlock();
        self.file_content = String::new();
        self.file_content_old = String::new();
        self.file = None;
        Ok(())
    }
}
