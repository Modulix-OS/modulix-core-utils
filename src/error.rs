use std::{fmt, io, result};

#[derive(fmt::Debug)]
pub enum ErrorType {
    InvalidFile,
    FileNotFound,
    OptionNotFound,
    FailToLock,
    PermissionDenied,
    TransactionNotBegin,
    GitNotCommitted,
    IOError(io::Error),
    GitError(git2::Error),
}

pub type Result<T> = result::Result<T, ErrorType>;

impl ToString for ErrorType {
    fn to_string(&self) -> String {
        match self {
            Self::InvalidFile => String::from("File is not a valid Nix file"),
            Self::OptionNotFound => String::from("Option not found"),
            Self::FileNotFound => String::from("File not found"),
            Self::TransactionNotBegin => String::from("Transaction don't start"),
            Self::FailToLock => String::from("Impossible to take lock"),
            Self::PermissionDenied => String::from("Permission denied"),
            Self::GitNotCommitted => {
                String::from("In repository file are untracked or not committed")
            }
            Self::IOError(e) => e.to_string(),
            Self::GitError(e) => e.to_string(),
        }
    }
}
