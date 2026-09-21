//! Offline generator for the `flathub_basic_info.rs` lookup table consumed by
//! `src/package_info/package_basic_info.rs`.
//!
//! # Running it
//! ```bash
//! cargo run --bin flathub-info-gen --features flathub-info-gen
//! ```
//! (see the crate's `CLAUDE.md`). It must be run with the current working
//! directory set to the `modulix-core-utils` crate root: the output file is
//! written to the relative path `flathub_basic_info.rs`, and
//! `package_basic_info.rs` (in `src/package_info/`) locates it via
//! `include!("../../flathub_basic_info.rs")`, i.e. two levels up from there -
//! the crate root.
//!
//! # What it queries
//! Two independent sources, joined locally by [`match_score`]:
//! - **nixpkgs**, via [`get_nix_packages`]: a `nix eval` that walks *every*
//!   attribute of `legacyPackages.x86_64-linux` and keeps those exposing
//!   `meta.mainProgram`. This evaluates the whole package set and is the slow
//!   step - expect it to take on the order of minutes.
//! - **Flathub**, via [`get_flathub_id`] (one request for the full AppStream
//!   catalog) and [`get_flathub_app_info`] (one request per nix package that
//!   [`match_score`] paired with a Flathub app id, run with up to 24 requests
//!   in flight and up to 4 immediate retries each on transient failure). The
//!   request volume therefore scales with the number of matched packages, not
//!   with the size of either catalog.
//!
//! # Output
//! [`generate_file`] unconditionally overwrites `flathub_basic_info.rs` in the
//! current working directory with freshly rendered `phf` maps - there is no
//! merge with the previous contents. Since `package_basic_info.rs` only
//! `include!`s that file when the `flathub-info-gen` feature is *disabled*,
//! the regenerated file must be committed for the new lookups to take effect
//! for normal (non-generator) builds.
//!
//! # Generated table shape
//! - `NIX_INFO: phf::Map<&'static str, NixInfo>` - keyed by nixpkgs attribute
//!   name, one entry per attribute that resolved to a Flathub match. Mirrors
//!   the `NixInfo` struct defined in `package_basic_info.rs`, with `icon_name`
//!   populated from the nix package's `exe` (its `meta.mainProgram`).
//! - `APP_ID_TO_PACKAGES: phf::Map<&'static str, &'static [&'static str]>` -
//!   the reverse index, keyed by Flathub app id, mapping to the sorted list of
//!   nixpkgs attribute names that matched it.
//!
//! Attributes with no Flathub match appear in neither map.

use futures::StreamExt;
use modulix_core_utils::mx;
use phf::phf_map;
use serde::Deserialize;
use std::collections::HashMap;

/// Deserialized subset of a single Flathub AppStream component, as returned
/// by `GET /api/v2/appstream/{app_id}`.
///
/// # Fields
/// * `name` - the app's display name; `None` when the API omits it.
/// * `keywords` - AppStream search keywords; `None` when the API omits them.
#[derive(Deserialize)]
struct FlathubAppInfo {
    name: Option<String>,
    keywords: Option<Vec<String>>,
}

/// Fetches display name and keywords for one Flathub app id, retrying a few
/// times on transient network or HTTP failures.
///
/// The immediate retries matter because the generator fans out many requests
/// at once and Flathub intermittently drops or rate-limits them; a transient
/// failure must not silently discard an otherwise-valid match.
///
/// # Parameters
/// * `client` - shared HTTP client the caller pools across concurrent calls.
/// * `app_id` - Flathub AppStream component id (e.g. `com.obsproject.Studio`).
///
/// # Returns
/// `Some((name, keywords))` on the first successful, parseable response
/// (missing JSON fields default to an empty `String`/`Vec`); `None` after 4
/// consecutive failed attempts (connection error, non-2xx status, or a body
/// that does not deserialize as [`FlathubAppInfo`]).
async fn get_flathub_app_info(
    client: &reqwest::Client,
    app_id: &str,
) -> Option<(String, Vec<String>)> {
    let url = format!("https://flathub.org/api/v2/appstream/{app_id}");
    for _ in 0..4u32 {
        let Ok(resp) = client.get(url.as_str()).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(info) = resp.json::<FlathubAppInfo>().await else {
            continue;
        };
        return Some((
            info.name.unwrap_or_default(),
            info.keywords.unwrap_or_default(),
        ));
    }
    None
}

/// Fetches the full Flathub AppStream catalog as a flat list of app ids.
///
/// # Returns
/// Every AppStream component id currently published on Flathub, in the
/// server's response order.
///
/// # Errors
/// [`mx::ErrorKind::HttpError`] if the request fails or the response body
/// does not deserialize as a JSON array of strings.
async fn get_flathub_id() -> mx::Result<Vec<String>> {
    reqwest::get("https://flathub.org/api/v2/appstream")
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))?
        .json()
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))
}

/// Hand-curated exe-to-app-id overrides for pairs [`match_score`]'s heuristics
/// cannot resolve on their own (typically because the exe name is an
/// unrelated CLI tool's name, see [`EXE_COLLISION_DENYLIST`]).
///
/// Looked up first in [`match_score`]; a hit short-circuits with the maximum
/// possible score ([`u32::MAX`]) when `app_id` equals the mapped value, and
/// `None` otherwise - bypassing every other tier, including the denylist.
static KNOWN_ID_MATCHES: phf::Map<&'static str, &'static str> = phf_map! {
    "obs" => "com.obsproject.Studio",
    "zeditor" => "dev.zed.Zed",
};

/// Exe names that collide with an unrelated app and must not auto-match. `zed` is
/// SpiceDB's CLI (`spicedb-zed`); left alone it grabs the Zed editor's
/// `dev.zed.Zed` via the last-segment tier. SpiceDB has no Flathub GUI app.
const EXE_COLLISION_DENYLIST: &[&str] = &["zed"];

/// Packaging/build noise dropped from the exe before comparison, so that e.g.
/// `google-chrome-stable` reduces to `googlechrome`.
const NOISE_TOKENS: &[&str] = &[
    "stable",
    "bin",
    "unwrapped",
    "unstable",
    "git",
    "nightly",
    "beta",
    "dev",
    "wrapped",
    "fhs",
    "electron",
    "appimage",
    "gtk",
    "gtk3",
    "gtk4",
    "qt",
    "qt5",
    "qt6",
    "wayland",
    "x11",
];

/// Generic words too weak to match on their own (token-level fallback only).
const STOPWORDS: &[&str] = &[
    "app",
    "application",
    "desktop",
    "client",
    "browser",
    "studio",
    "player",
    "editor",
    "gnome",
    "kde",
    "www",
    "com",
    "org",
    "net",
    "io",
    "dev",
];

/// Strips every non-ASCII-alphanumeric character and lowercases the rest, so
/// that e.g. `app_id` segments and exe names can be compared regardless of
/// separators or casing.
///
/// # Parameters
/// * `s` - the string to normalize.
///
/// # Returns
/// The lowercased, separator-free string; empty if `s` has no ASCII
/// alphanumeric character.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Splits `s` on every non-ASCII-alphanumeric character into lowercased,
/// non-empty tokens, unlike [`normalize`] which discards separators instead
/// of splitting on them.
///
/// # Parameters
/// * `s` - the string to tokenize.
///
/// # Returns
/// The lowercased tokens, in order; empty if `s` has no ASCII alphanumeric
/// character.
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Scores how likely a nixpkgs `exe` (a `meta.mainProgram`) is to be the
/// binary of the Flathub app identified by `app_id`.
///
/// Tries, in order: the [`KNOWN_ID_MATCHES`] override; the
/// [`EXE_COLLISION_DENYLIST`]; an exact match of `exe` (in full or with
/// [`NOISE_TOKENS`] stripped) against the last 3, last 2, or last reverse-DNS
/// segment of `app_id`; a match against the second-to-last segment; and,
/// only when the last two segments of `app_id` repeat (e.g. `...Foo.Foo`), a
/// [`STOPWORDS`]-filtered token match against the last segment. Longer,
/// more specific matches score higher, so the best candidate among several
/// `app_id`s can be picked by taking the maximum score.
///
/// # Parameters
/// * `exe` - the nixpkgs package's `meta.mainProgram`.
/// * `app_id` - a candidate Flathub AppStream component id.
///
/// # Returns
/// `None` when no tier matches (or `exe` is denylisted, or `app_id` has no
/// non-empty segment). Otherwise `Some(score)`: [`u32::MAX`] for a
/// [`KNOWN_ID_MATCHES`] hit, or a tier-dependent base (4000/3000/2000/1000/500)
/// plus the length of the matched text, so longer matches within the same
/// tier outrank shorter ones.
pub fn match_score(exe: &str, app_id: &str) -> Option<u32> {
    if let Some(match_id) = KNOWN_ID_MATCHES.get(exe) {
        return (app_id == *match_id).then_some(u32::MAX);
    }
    if EXE_COLLISION_DENYLIST.contains(&exe) {
        return None;
    }

    let segs: Vec<String> = app_id
        .split('.')
        .map(normalize)
        .filter(|s| !s.is_empty())
        .collect();
    let n = segs.len();
    if n == 0 {
        return None;
    }

    let exe_tokens = tokens(exe);
    let full = normalize(exe);
    let stripped: String = exe_tokens
        .iter()
        .filter(|t| !NOISE_TOKENS.contains(&t.as_str()))
        .flat_map(|t| t.chars())
        .collect();
    let exe_forms = [full.as_str(), stripped.as_str()];
    let exe_eq = |form: &str| !form.is_empty() && exe_forms.contains(&form);

    let last = segs[n - 1].as_str();
    if n >= 3 {
        let last3 = format!("{}{}{}", segs[n - 3], segs[n - 2], segs[n - 1]);
        if exe_eq(&last3) {
            return Some(4000 + last3.len() as u32);
        }
    }
    if n >= 2 {
        let last2 = format!("{}{}", segs[n - 2], segs[n - 1]);
        if exe_eq(&last2) {
            return Some(3000 + last2.len() as u32);
        }
    }
    if exe_eq(last) {
        return Some(2000 + last.len() as u32);
    }
    if n >= 2 {
        let middle = segs[n - 2].as_str();
        if exe_eq(middle) {
            return Some(1000 + middle.len() as u32);
        }
    }

    if n >= 2 && segs[n - 2] == segs[n - 1] {
        for tok in &exe_tokens {
            if !STOPWORDS.contains(&tok.as_str()) && tok == last && !tok.is_empty() {
                return Some(500 + tok.len() as u32);
            }
        }
    }

    None
}

/// One nixpkgs attribute exposing a `meta.mainProgram`, as deserialized from
/// the `nix eval` output in [`get_nix_packages`].
///
/// # Fields
/// * `name` - the nixpkgs attribute path (e.g. `firefox`).
/// * `exe` - that attribute's `meta.mainProgram`.
#[derive(serde::Deserialize)]
struct NixPackage {
    name: String,
    exe: String,
}

/// Enumerates every nixpkgs attribute of `legacyPackages.x86_64-linux` that
/// declares a `meta.mainProgram`, by shelling out to `nix eval`.
///
/// The Nix expression walks `builtins.attrNames pkgs` and wraps each
/// attribute access in `builtins.tryEval` so that attributes which throw
/// during evaluation are skipped rather than aborting the whole run. This
/// touches the entire nixpkgs attribute set and is the slowest step of the
/// generator.
///
/// # Returns
/// `(name, exe)` pairs, one per matching attribute, in the order `nix eval`
/// emits them.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the `nix` process cannot be spawned;
/// [`mx::ErrorKind::ParseError`] if its stdout does not deserialize as a JSON
/// array of [`NixPackage`] - this is also what happens if `nix eval` itself
/// fails, since its exit status is not checked here.
async fn get_nix_packages() -> mx::Result<Vec<(String, String)>> {
    let output = tokio::process::Command::new("nix")
        .args([
            "eval",
            "--json",
            "nixpkgs#legacyPackages.x86_64-linux",
            "--apply",
            r#"pkgs: builtins.foldl' (acc: name:
                let tried = builtins.tryEval (
                    let pkg = pkgs.${name};
                    in pkg ? meta && pkg.meta ? mainProgram
                );
                in if tried.success && tried.value
                then acc ++ [{ inherit name; exe = pkgs.${name}.meta.mainProgram; }]
                else acc
            ) [] (builtins.attrNames pkgs)"#,
        ])
        .output()
        .await
        .map_err(|e| mx::ErrorKind::IOError(e))?;

    Ok(serde_json::from_slice::<Vec<NixPackage>>(&output.stdout)
        .map_err(|e| mx::ErrorKind::ParseError(e))?
        .into_iter()
        .map(|p| (p.name, p.exe))
        .collect())
}

/// Pairs each distinct nix package `exe` with its best-scoring Flathub app id
/// (via [`match_score`]), then fetches that app's info from Flathub.
///
/// Ties on score are broken in favor of the longer `app_id`, on the
/// assumption that a longer reverse-DNS id is more specific. Nix packages
/// with no matching Flathub id are silently dropped, as are matched ones
/// whose Flathub lookup ultimately fails (see [`get_flathub_app_info`]).
///
/// # Parameters
/// * `flathub_ids` - the full Flathub AppStream catalog, as returned by
///   [`get_flathub_id`].
/// * `nix_packages` - `(name, exe)` pairs, as returned by [`get_nix_packages`];
///   only `exe` is used here, `name` is carried by the caller separately.
///
/// # Returns
/// A map from nix `exe` to `(display_name, app_id, keywords)`, covering only
/// the `exe`s that both matched a Flathub app id and whose Flathub lookup
/// succeeded.
async fn resolve_flathub_info(
    flathub_ids: &[String],
    nix_packages: &[(String, String)],
) -> HashMap<String, (String, String, Vec<String>)> {
    let matched: HashMap<String, String> = nix_packages
        .iter()
        .filter_map(|(_, exe)| {
            let (_, app_id) = flathub_ids
                .iter()
                .filter_map(|id| match_score(exe, id).map(|s| (s, id)))
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.len().cmp(&a.1.len())))?;
            Some((exe.clone(), app_id.clone()))
        })
        .collect();

    let client = reqwest::Client::new();

    /// Maximum number of concurrent Flathub app-info requests in flight, used
    /// by the `buffer_unordered` stream below.
    const CONCURRENCY: usize = 24;

    futures::stream::iter(matched.into_iter().map(|(exe, app_id)| {
        let client = &client;
        async move {
            let result = get_flathub_app_info(client, &app_id).await;
            (exe, app_id, result)
        }
    }))
    .buffer_unordered(CONCURRENCY)
    .filter_map(|(exe, app_id, result)| async move {
        let (name, keywords) = result?;
        Some((exe, (name, app_id, keywords)))
    })
    .collect()
    .await
}

/// Escapes backslashes and double quotes so `s` can be embedded verbatim
/// inside a Rust string literal in the generated source.
///
/// # Parameters
/// * `s` - the raw string to embed.
///
/// # Returns
/// `s` with `\` and `"` backslash-escaped; safe to place between `"` … `"` in
/// generated Rust source.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Renders `enriched` into the two `phf` maps that make up
/// `flathub_basic_info.rs` and overwrites that file (relative to the current
/// working directory) with the result.
///
/// Entries whose `info` is `None` (no Flathub match) are omitted from both
/// generated maps. `NIX_INFO` is emitted first, one line per matched nix
/// package, keyed by package name, while building an `app_id -> packages`
/// map along the way; `APP_ID_TO_PACKAGES` is derived from that map as the
/// reverse index, keyed by app id with its package list sorted, and is
/// itself emitted in app-id-sorted order for a deterministic diff. Building
/// the reverse index alongside `NIX_INFO` avoids an O(n) scan of `NIX_INFO`
/// per `packages_for_app_id` call at lookup time.
///
/// # Parameters
/// * `enriched` - `(package_name, exe, Option<(display_name, app_id,
///   keywords)>)` triples, one per nix package considered, as built by
///   `main`.
///
/// # Post-conditions
/// `flathub_basic_info.rs` in the current working directory is replaced
/// atomically-at-the-syscall-level (single `std::fs::write`) with the newly
/// rendered content; any previous content is lost.
///
/// # Panics
/// If writing `flathub_basic_info.rs` fails (e.g. the current directory is
/// not writable).
fn generate_file(enriched: &[(String, String, Option<(String, String, Vec<String>)>)]) {
    let mut out = String::new();

    out.push_str("static NIX_INFO: phf::Map<&'static str, NixInfo> = phf::phf_map! {\n");
    let mut by_app_id: HashMap<String, Vec<String>> = HashMap::new();
    for (pkg, exe, info) in enriched {
        if let Some((name, app_id, keywords)) = info {
            let kw = keywords
                .iter()
                .map(|k| format!("\"{}\"", escape(k)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "    \"{}\" => NixInfo {{ name: \"{}\", app_id: \"{}\", icon_name: \"{}\", keywords: &[{}] }},\n",
                escape(pkg),
                escape(name),
                escape(app_id),
                escape(exe),
                kw,
            ));
            by_app_id
                .entry(app_id.clone())
                .or_default()
                .push(pkg.clone());
        }
    }
    out.push_str("};\n\n");

    out.push_str(
        "static APP_ID_TO_PACKAGES: phf::Map<&'static str, &'static [&'static str]> = phf::phf_map! {\n",
    );
    let mut app_ids: Vec<&String> = by_app_id.keys().collect();
    app_ids.sort();
    for app_id in app_ids {
        let mut pkgs = by_app_id[app_id].clone();
        pkgs.sort();
        let list = pkgs
            .iter()
            .map(|p| format!("\"{}\"", escape(p)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("    \"{}\" => &[{}],\n", escape(app_id), list));
    }
    out.push_str("};\n");

    std::fs::write("flathub_basic_info.rs", out)
        .expect("Imposible to generate flathub_basic_info.rs");
}

#[cfg(test)]
mod tests {
    use super::match_score;

    /// A handful of previously-unmatched exe/app-id pairs now score as matches.
    #[test]
    fn newly_recognized_apps() {
        assert!(match_score("google-chrome-stable", "com.google.Chrome").is_some());
        assert!(match_score("microsoft-edge", "com.microsoft.Edge").is_some());
        assert!(match_score("signal-desktop", "org.signal.Signal").is_some());
    }

    /// The canonical app id scores at least as high as a `Dev`/variant id for
    /// the same exe, so `resolve_flathub_info`'s `max_by` prefers it.
    #[test]
    fn canonical_beats_variant() {
        let chrome = match_score("google-chrome-stable", "com.google.Chrome").unwrap();
        let dev = match_score("google-chrome-stable", "com.google.ChromeDev");
        assert!(dev.is_none_or(|d| d < chrome));
    }

    /// Previously-passing exe/app-id pairs keep matching (or not matching),
    /// including the [`KNOWN_ID_MATCHES`] override for `obs`.
    #[test]
    fn regressions_still_match() {
        assert!(match_score("firefox", "org.mozilla.firefox").is_some());
        assert!(match_score("spotify", "com.spotify.Client").is_some());
        assert!(match_score("code", "com.visualstudio.code").is_some());
        assert!(match_score("Telegram", "org.telegram.desktop").is_some());
        assert_eq!(match_score("obs", "com.obsproject.Studio"), Some(u32::MAX));
        assert!(match_score("obs", "org.something.obs").is_none());
    }

    /// [`EXE_COLLISION_DENYLIST`] blocks `zed` (SpiceDB's CLI) from matching
    /// the Zed editor, while the unambiguous `zeditor` exe still matches via
    /// [`KNOWN_ID_MATCHES`].
    #[test]
    fn zed_editor_vs_spicedb_collision() {
        assert_eq!(match_score("zeditor", "dev.zed.Zed"), Some(u32::MAX));
        assert!(match_score("zed", "dev.zed.Zed").is_none());
    }

    /// Unrelated exe/app-id pairs, including same-segment-count near misses,
    /// score `None`.
    #[test]
    fn rejects_unrelated() {
        assert!(match_score("microsoft-edge", "com.microsoft.Teams").is_none());
        assert!(match_score("signal-desktop", "org.telegram.desktop").is_none());
        assert!(match_score("gnome-terminal", "br.app.pw3270.terminal").is_none());
        assert!(match_score("muse-sounds-manager", "com.evepreview.manager").is_none());
        assert!(match_score("opencloud-dolphin", "org.kde.dolphin").is_none());
    }
}

/// Entry point of the `flathub-info-gen` binary: fetches the Flathub catalog
/// and the nixpkgs package list concurrently, resolves matches between them,
/// and writes `flathub_basic_info.rs`. See the crate-level documentation for
/// how to run it and what it writes.
///
/// # Panics
/// If either [`get_flathub_id`] or [`get_nix_packages`] returns an `Err`
/// (propagated via `.unwrap()`), or if [`generate_file`] fails to write its
/// output.
#[tokio::main]
async fn main() {
    let (flathub, nix) = tokio::try_join!(get_flathub_id(), get_nix_packages()).unwrap();
    let flathub_info = resolve_flathub_info(&flathub, &nix).await;
    let enriched: Vec<(String, String, Option<(String, String, Vec<String>)>)> = nix
        .into_iter()
        .map(|(pkg, exe)| {
            let info = flathub_info.get(&exe).cloned();
            (pkg, exe, info)
        })
        .collect();
    generate_file(&enriched);
}
