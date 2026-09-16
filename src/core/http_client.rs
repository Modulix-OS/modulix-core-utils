use std::sync::OnceLock;
use std::time::Duration;

/// Process-wide HTTP client shared by every Flathub / module-index request.
///
/// A bare `reqwest::get()` builds a fresh `Client` (and TLS stack) per call
/// and has no request timeout, so one stalled connection blocks its caller's
/// thread forever. This client applies sane timeouts and lets `reqwest` pool
/// connections across calls.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("failed to build the shared reqwest client")
    })
}
