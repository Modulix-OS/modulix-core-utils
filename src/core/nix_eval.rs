use serde::de::DeserializeOwned;

use crate::mx;

/// Run `nix eval --json <args>` and deserialize its stdout into `T`.
///
/// `args` are appended verbatim after `eval --json`, so callers pass the
/// installable and any extra flags themselves, e.g. `["nixpkgs#hello.meta.mainProgram"]`,
/// `["--expr", "<expr>"]`, or `["<flakeref>", "--apply", "<lambda>"]`.
///
/// `NIXPKGS_ALLOW_UNFREE=1` is set so evaluating unfree attributes does not abort.
/// A non-zero exit surfaces stderr as [`mx::ErrorKind::NixCommandError`].
pub async fn eval_json<T: DeserializeOwned>(args: &[&str]) -> mx::Result<T> {
    let output = tokio::process::Command::new("nix")
        .args(["eval", "--json"])
        .args(args)
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .output()
        .await
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
    serde_json::from_str(&stdout).map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))
}
