//! Before/after benchmark for the package_index warm-up
//! (see `src/package_index/`): times `NixPackage::search_scored` for a
//! handful of queries against the live `nix search` fallback, builds the
//! mmap'd index, then times the same queries again.
//!
//! Run with: `cargo run --release --features package-info --example bench_search`

use std::time::Instant;

use modulix_core_utils::AppInfoMinimal;
use modulix_core_utils::package_index;
use modulix_core_utils::package_info::NixPackage;

const QUERIES: &[&str] = &[
    "fir", "fire", "firef", "firefo", "firefox", "frefox", "vlc", "gimp",
];

async fn run_queries(label: &str) {
    println!("=== {label} ===");
    for &q in QUERIES {
        let t0 = Instant::now();
        let n = NixPackage::search_scored(q, 20)
            .await
            .map(|v| v.len())
            .unwrap_or(0);
        println!("{q:>10}: {:>10.1?}  ({n} hits)", t0.elapsed());
    }
    println!();
}

#[tokio::main]
async fn main() {
    run_queries("fallback (nix search per query)").await;

    let t0 = Instant::now();
    package_index::ensure_fresh_in_background().await;
    println!("index build: {:?}\n", t0.elapsed());

    run_queries("mmap index").await;
}
