use std::{fmt, io, result, string};

#[derive(fmt::Debug)]
pub enum ErrorKind {
    InvalidFile,
    FileNotFound,
    OptionNotFound,
    FailToLock,
    PermissionDenied,
    TransactionNotBegin,
    TransactionAlreadyBegin,
    GitNotCommitted,
    OptionIsNotList,
    InvalidUuid,
    PackageDoesNotHaveAPlugin,
    CPUInfoNofFound,
    UnknowCPUConstructor,
    ErrorParseCPUCodename,
    ThreadError,
    GetVGAInfoError(&'static str),
    BuildError(String),
    RequestSenderError(String),
    NixCommandError(String),
    FromUtf8Error(string::FromUtf8Error),
    IOError(io::Error),
    GitError(git2::Error),
    UnixError(nix::Error),
    ParseError(serde_json::Error),
}

pub type Result<T> = result::Result<T, ErrorKind>;

impl ToString for ErrorKind {
    fn to_string(&self) -> String {
        match self {
            Self::InvalidFile => String::from("File is not a valid Nix file"),
            Self::OptionNotFound => String::from("Option not found"),
            Self::FileNotFound => String::from("File not found"),
            Self::TransactionNotBegin => String::from("Transaction don't start"),
            Self::TransactionAlreadyBegin => String::from("Transaction already start"),
            Self::FailToLock => String::from("Impossible to take lock"),
            Self::PermissionDenied => String::from("Permission denied"),
            Self::GitNotCommitted => {
                String::from("In repository file are untracked or not committed")
            }
            Self::OptionIsNotList => String::from("This option is not a list"),
            Self::InvalidUuid => String::from("Invalid uuid for device"),
            Self::PackageDoesNotHaveAPlugin => String::from("This package does not have a plugin"),
            Self::CPUInfoNofFound => String::from("CPU info not found"),
            Self::UnknowCPUConstructor => String::from("Unknow CPU constructor"),
            Self::ErrorParseCPUCodename => String::from("Impossible to parse CPU codename"),
            Self::ThreadError => String::from("Thread error"),
            Self::RequestSenderError(s) => s.to_string(),
            Self::GetVGAInfoError(e) => e.to_string(),
            Self::IOError(e) => e.to_string(),
            Self::GitError(e) => e.to_string(),
            Self::BuildError(s) => s.to_string(),
            Self::NixCommandError(s) => s.to_string(),
            Self::FromUtf8Error(e) => e.to_string(),
            Self::UnixError(e) => e.to_string(),
            Self::ParseError(e) => e.to_string(),
        }
    }
}
