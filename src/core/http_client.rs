use std::sync::OnceLock;
use std::time::Duration;

use crate::mx;

/// Inactivity budget, not a total budget: a Flathub AppStream payload carries
/// screenshots, releases and urls, and a whole-request deadline would turn a
/// slow-but-working link (mobile, distant mirror) into a hard `HttpError`.
/// What has to be caught is a connection that stops producing bytes.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Backstop for a peer that keeps trickling bytes forever. Generous on purpose
/// — it only exists so a request cannot hang a caller's task indefinitely.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Process-wide HTTP client shared by every Flathub / module-index request.
///
/// A bare `reqwest::get()` builds a fresh `Client` (and TLS stack) per call
/// and has no request timeout, so one stalled connection blocks its caller's
/// thread forever. This client applies sane timeouts and lets `reqwest` pool
/// connections across calls.
///
/// The build result is cached, error included: it can only fail on TLS backend
/// initialization, which is not something a library may panic the host process
/// over, and which will fail identically on every later call anyway.
static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

pub fn client() -> mx::Result<&'static reqwest::Client> {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .timeout(TOTAL_TIMEOUT)
                .build()
                .map_err(|e| format!("failed to build the shared HTTP client: {e}"))
        })
        .as_ref()
        .map_err(|e| mx::ErrorKind::RequestSenderError(e.clone()))
}
