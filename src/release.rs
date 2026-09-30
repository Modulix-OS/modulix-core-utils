//! The Modulix OS release identity published by `mxpkgs`.
//!
//! `mxpkgs/release.json` carries the two fields `mx.branding` needs
//! (`version`, `codeName`) and is the single source of truth for both the
//! branding of a built system and the release a running system could move to.
//! The nix side reads the file from the flake source; this module reads the
//! same file over HTTP from the tracked branch, so a system can tell whether
//! upstream has moved to a newer release.
//!
//! Read-only by design: nothing here writes to the configuration or feeds a
//! nix evaluation. The two fields are compared and displayed, never executed,
//! which is why a strict validation of both (see [`Release::validate`]) is
//! enough to make an untrusted branch harmless.

use serde::Deserialize;

use crate::REMOTE_RELEASE_URL;
use crate::mx;

/// Largest `release.json` body accepted, in bytes.
///
/// The real file is under 60 bytes; the cap only exists so a hostile or broken
/// endpoint cannot stream an unbounded body into memory within the shared
/// client's total timeout.
const MAX_BODY_BYTES: usize = 4096;

/// Longest accepted `version`, in bytes.
const MAX_VERSION_LEN: usize = 32;

/// Longest accepted `codeName`, in bytes.
const MAX_CODE_NAME_LEN: usize = 64;

/// The release identity `mxpkgs` publishes.
///
/// # Fields
/// * `version` - release number, the same string a built system exposes as
///   `VERSION_ID` in `/etc/os-release`.
/// * `code_name` - human-readable release name, `VERSION_CODENAME`'s source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(rename = "codeName")]
    pub code_name: String,
}

impl Release {
    /// Rejects a release whose fields are not plausible identities.
    ///
    /// `version` must start with a digit and be made only of
    /// `[0-9A-Za-z._-]`, at most [`MAX_VERSION_LEN`] bytes; `code_name` must
    /// be non-empty, at most [`MAX_CODE_NAME_LEN`] bytes, and carry no control
    /// character. Both end up in UI strings, so this is what keeps a hostile
    /// branch from injecting an oversized or malformed label.
    ///
    /// # Post-conditions
    /// Leaves `self` untouched; it only reports.
    ///
    /// # Errors
    /// [`mx::ErrorKind::InvalidArgument`] naming the offending field.
    fn validate(&self) -> mx::Result<()> {
        let version_ok = !self.version.is_empty()
            && self.version.len() <= MAX_VERSION_LEN
            && self.version.starts_with(|c: char| c.is_ascii_digit())
            && self
                .version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !version_ok {
            return Err(mx::ErrorKind::InvalidArgument(format!(
                "release.json: implausible version {:?}",
                self.version
                    .chars()
                    .take(MAX_VERSION_LEN)
                    .collect::<String>()
            )));
        }

        let code_name_ok = !self.code_name.is_empty()
            && self.code_name.len() <= MAX_CODE_NAME_LEN
            && !self.code_name.chars().any(char::is_control);
        if !code_name_ok {
            return Err(mx::ErrorKind::InvalidArgument(
                "release.json: implausible codeName".to_string(),
            ));
        }

        Ok(())
    }
}

/// Parses and validates a `release.json` body.
///
/// # Parameters
/// * `body` - the raw file content.
///
/// # Returns
/// The parsed [`Release`], guaranteed to have passed [`Release::validate`].
///
/// # Errors
/// [`mx::ErrorKind::ParseError`] when `body` is not the expected JSON,
/// [`mx::ErrorKind::InvalidArgument`] when a field is implausible.
fn parse_release(body: &str) -> mx::Result<Release> {
    let release: Release = serde_json::from_str(body).map_err(mx::ErrorKind::ParseError)?;
    release.validate()?;
    Ok(release)
}

/// Fetches the release `mxpkgs` currently publishes on its tracked branch.
///
/// # Returns
/// The upstream [`Release`]. Comparing its `version` with the running system's
/// `VERSION_ID` is what tells a caller whether a release upgrade exists; this
/// function makes no such comparison itself.
///
/// # Pre-conditions
/// None. Performs one HTTPS request to [`REMOTE_RELEASE_URL`] on every call —
/// callers that poll are expected to cache the result themselves.
///
/// # Post-conditions
/// Writes nothing, evaluates nothing. A body larger than [`MAX_BODY_BYTES`] is
/// refused before parsing.
///
/// # Errors
/// [`mx::ErrorKind::RequestSenderError`] if the shared HTTP client cannot be
/// built, [`mx::ErrorKind::HttpError`] on a failed request or error status,
/// [`mx::ErrorKind::InvalidArgument`] on an oversized body or an implausible
/// field, [`mx::ErrorKind::ParseError`] on malformed JSON.
pub async fn remote_release() -> mx::Result<Release> {
    let response = crate::core::http_client::client()?
        .get(REMOTE_RELEASE_URL)
        .send()
        .await
        .map_err(mx::ErrorKind::HttpError)?
        .error_for_status()
        .map_err(mx::ErrorKind::HttpError)?;

    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        return Err(mx::ErrorKind::InvalidArgument(
            "release.json: body too large".to_string(),
        ));
    }

    let body = response.text().await.map_err(mx::ErrorKind::HttpError)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(mx::ErrorKind::InvalidArgument(
            "release.json: body too large".to_string(),
        ));
    }

    parse_release(&body)
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
