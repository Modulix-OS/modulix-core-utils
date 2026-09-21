use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::mx;

/// Ceiling on a single `nix eval` invocation. Bounds an otherwise-unbounded
/// hang (e.g. a namespace enumeration that forces evaluation across a large
/// nixpkgs subtree).
const EVAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Run `nix <args>` (JSON output assumed) and deserialize its stdout into `T`,
/// bounded by `timeout`. Shared by every `nix` subprocess call in this crate
/// so none of them can hang indefinitely — see [`eval_json`] for the `nix
/// eval` specialisation.
///
/// `NIXPKGS_ALLOW_UNFREE=1` is set so evaluating unfree attributes does not
/// abort. A non-zero exit surfaces stderr as [`mx::ErrorKind::NixCommandError`];
/// taking longer than `timeout` kills the process and does the same.
///
/// # Type parameters
/// * `T` - type the JSON output is deserialised into.
///
/// # Parameters
/// * `args` - arguments passed to `nix`, verbatim and in order; they must make
///   it print JSON on stdout.
/// * `timeout` - ceiling on the whole invocation.
///
/// # Pre-conditions
/// `nix` must be on `PATH`, with flakes enabled for the installables the caller
/// passes.
///
/// # Returns
/// The deserialised output.
///
/// # Errors
/// [`mx::ErrorKind::NixCommandError`] on timeout, on a non-zero exit (payload
/// is stderr), or when the output does not deserialise into `T`;
/// [`mx::ErrorKind::IOError`] when the process cannot be spawned; and
/// [`mx::ErrorKind::FromUtf8Error`] when stdout is not UTF-8.
///
/// # Post-conditions
/// Dropping the future kills the child, so a cancelled caller leaves no `nix`
/// process behind.
pub async fn run_json<T: DeserializeOwned>(args: &[&str], timeout: Duration) -> mx::Result<T> {
    let mut command = tokio::process::Command::new("nix");
    command
        .args(args)
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .kill_on_drop(true);

    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| mx::ErrorKind::NixCommandError(format!("nix {} timed out", args.join(" "))))?
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::NixCommandError(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;
    serde_json::from_str(&stdout).map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))
}

/// Run `nix eval --json <args>` and deserialize its stdout into `T`.
///
/// `args` are appended verbatim after `eval --json`, so callers pass the
/// installable and any extra flags themselves, e.g. `["nixpkgs#hello.meta.mainProgram"]`,
/// `["--expr", "<expr>"]`, or `["<flakeref>", "--apply", "<lambda>"]`.
///
/// # Type parameters
/// * `T` - type the JSON output is deserialised into.
///
/// # Parameters
/// * `args` - what follows `eval --json`: the installable, plus any flag.
///
/// # Returns
/// The deserialised value.
///
/// # Errors
/// As in [`run_json`], with the invocation bounded by the module's own
/// evaluation timeout rather than a caller-supplied one.
pub async fn eval_json<T: DeserializeOwned>(args: &[&str]) -> mx::Result<T> {
    let mut full = Vec::with_capacity(args.len() + 2);
    full.push("eval");
    full.push("--json");
    full.extend_from_slice(args);
    run_json(&full, EVAL_TIMEOUT).await
}
