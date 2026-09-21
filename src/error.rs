//! The crate's single error type, re-exported as [`crate::mx::ErrorKind`].

use std::{fmt, io, result, string};

/// Every way a `modulix-core-utils` call can fail.
///
/// One flat enum for the whole crate: a caller (the daemon, the GNOME Software
/// plugin) maps errors to a message or a D-Bus error rather than branching on
/// them, so per-module error types would only add conversions.
///
/// # Variants
/// * `InvalidFile` - the target is not parseable as a Nix file.
/// * `FileNotFound` - the configuration file does not exist.
/// * `OptionNotFound` - the requested option is absent from the configuration.
/// * `FailToLock` - another process holds the lock on the file or the build
///   queue.
/// * `PermissionDenied` - the caller lacks the rights on the configuration
///   directory (it is expected to run privileged).
/// * `TransactionNotBegin` - an operation requiring an open transaction was
///   attempted outside one.
/// * `TransactionAlreadyBegin` - `begin` was called on an already-open
///   transaction.
/// * `GitNotCommitted` - the configuration repository has untracked or
///   uncommitted files where a clean tree is required.
/// * `OptionIsNotList` - a list operation was applied to a scalar option.
/// * `InvalidUuid` - the device UUID does not parse.
/// * `PackageDoesNotHaveAPlugin` - a plugin operation targets a module that
///   exposes none.
/// * `CPUInfoNofFound` - `/proc/cpuinfo` yielded nothing usable.
/// * `UnknowCPUConstructor` - the CPU vendor is neither Intel nor AMD.
/// * `ErrorParseCPUCodename` - the CPU model string does not match a known
///   codename.
/// * `ThreadError` - a `spawn_blocking` task panicked or was cancelled.
/// * `DesktopFileNotFound` - no `.desktop` entry matches the app.
/// * `InvalidNixString` - a configuration value is not a well-formed Nix
///   string literal.
/// * `PackageNotFound` - the attribute is absent from the package index.
/// * `GetVGAInfoError` - GPU detection failed; payload is the static reason.
/// * `BuildError` - `nixos-rebuild` exited non-zero; payload is its stderr.
/// * `RequestSenderError` - the shared HTTP client could not be built;
///   payload is the reason.
/// * `NixCommandError` - a `nix` invocation failed; payload is its stderr.
/// * `InvalidArgument` - the caller passed an unusable argument; payload is
///   the explanation.
/// * `FromUtf8Error` - a command's output is not valid UTF-8.
/// * `IOError` - filesystem failure; the path is folded into the message by
///   `io_error_at`.
/// * `GitError` - a libgit2 operation on the configuration repository failed.
/// * `UnixError` - a `nix` crate syscall wrapper failed.
/// * `ParseError` - JSON deserialisation of an index or a remote payload
///   failed.
/// * `HttpError` - a Flathub or module-index request failed.
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
    PackageNotFound,
    GetVGAInfoError(&'static str),
    BuildError(String),
    RequestSenderError(String),
    NixCommandError(String),
    InvalidArgument(String),
    FromUtf8Error(string::FromUtf8Error),
    IOError(io::Error),
    GitError(git2::Error),

    #[cfg(feature = "unix")]
    UnixError(nix::Error),

    #[cfg(feature = "serde-json")]
    ParseError(serde_json::Error),

    #[cfg(feature = "reqwest")]
    HttpError(reqwest::Error),
}

/// Result of every fallible call in this crate.
///
/// # Type parameters
/// * `T` - the success value.
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
                Self::PackageNotFound => "Package not found",
                Self::InvalidArgument(s) => s.as_str(),
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

                #[cfg(feature = "unix")]
                Self::UnixError(e) => {
                    s = e.to_string();
                    s.as_str()
                }

                #[cfg(feature = "serde-json")]
                Self::ParseError(e) => {
                    s = e.to_string();
                    s.as_str()
                }

                #[cfg(feature = "reqwest")]
                Self::HttpError(e) => {
                    s = format!("HTTP Request Error: {}", e);
                    s.as_str()
                }
            }
        )
    }
}

/// Wraps an I/O error with the path it occurred on.
///
/// A bare `Os { code: 13, kind: PermissionDenied }` is undiagnosable in a run
/// that touches `/tmp` sentinels, the config repository and the Nix store in
/// turn — the path is the whole diagnosis.
///
/// # Parameters
/// * `path` - the path the operation was performed on.
/// * `error` - the original I/O error.
///
/// # Returns
/// [`ErrorKind::IOError`] carrying a new error of the same
/// [`io::ErrorKind`], whose message is `"<path>: <error>"`.
pub(crate) fn io_error_at(path: &str, error: io::Error) -> ErrorKind {
    ErrorKind::IOError(io::Error::new(error.kind(), format!("{path}: {error}")))
}
