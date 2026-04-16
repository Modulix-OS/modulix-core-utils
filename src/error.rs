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
    DesktopFileNotFound,
    InvalidNixString,
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

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s: String;
        write!(
            f,
            "{}",
            match self {
                Self::InvalidFile => "File is not a valid Nix file",
                Self::OptionNotFound => "Option not found",
                Self::FileNotFound => "File not found",
                Self::TransactionNotBegin => "Transaction don't start",
                Self::TransactionAlreadyBegin => "Transaction already start",
                Self::FailToLock => "Impossible to take lock",
                Self::PermissionDenied => "Permission denied",
                Self::GitNotCommitted => "In repository file are untracked or not committed",
                Self::OptionIsNotList => "This option is not a list",
                Self::InvalidUuid => "Invalid uuid for device",
                Self::PackageDoesNotHaveAPlugin => "This package does not have a plugin",
                Self::CPUInfoNofFound => "CPU info not found",
                Self::UnknowCPUConstructor => "Unknow CPU constructor",
                Self::ErrorParseCPUCodename => "Impossible to parse CPU codename",
                Self::ThreadError => "Thread error",
                Self::DesktopFileNotFound => "Desktop icon not found",
                Self::InvalidNixString => "Impossible to parse nix string in configuration",
                Self::RequestSenderError(s) => s.as_str(),
                Self::GetVGAInfoError(e) => e,
                Self::IOError(e) => {
                    s = e.to_string();
                    s.as_str()
                }
                Self::GitError(e) => {
                    s = e.to_string();
                    s.as_str()
                }
                Self::BuildError(s) => s,
                Self::NixCommandError(s) => s.as_str(),
                Self::FromUtf8Error(e) => {
                    s = e.to_string();
                    s.as_str()
                }
                Self::UnixError(e) => {
                    s = e.to_string();
                    s.as_str()
                }
                Self::ParseError(e) => {
                    s = e.to_string();
                    s.as_str()
                }
            }
        )
    }
}
