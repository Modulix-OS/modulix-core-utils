use futures::StreamExt;
use modulix_core_utils::mx;
use phf::phf_map;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct FlathubAppInfo {
    name: Option<String>,
    keywords: Option<Vec<String>>,
}

async fn get_flathub_app_info(
    client: &reqwest::Client,
    app_id: &str,
) -> Option<(String, Vec<String>)> {
    let url = format!("https://flathub.org/api/v2/appstream/{app_id}");
    // A few immediate retries: the generator fans out many requests at once and
    // Flathub intermittently drops or rate-limits them. A transient failure must
    // not silently discard an otherwise-valid match.
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

async fn get_flathub_id() -> mx::Result<Vec<String>> {
    reqwest::get("https://flathub.org/api/v2/appstream")
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))?
        .json()
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))
}

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

fn normalize(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

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

#[derive(serde::Deserialize)]
struct NixPackage {
    name: String,
    exe: String,
}

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

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn generate_file(enriched: &[(String, String, Option<(String, String, Vec<String>)>)]) {
    let mut out = String::new();

    out.push_str("static NIX_INFO: phf::Map<&'static str, NixInfo> = phf::phf_map! {\n");
    // app_id -> every nix attribute matched to it, for the reverse lookup
    // emitted below (`APP_ID_TO_PACKAGES`) — avoids an O(n) scan of NIX_INFO
    // per `packages_for_app_id` call.
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

    #[test]
    fn newly_recognized_apps() {
        assert!(match_score("google-chrome-stable", "com.google.Chrome").is_some());
        assert!(match_score("microsoft-edge", "com.microsoft.Edge").is_some());
        assert!(match_score("signal-desktop", "org.signal.Signal").is_some());
    }

    #[test]
    fn canonical_beats_variant() {
        let chrome = match_score("google-chrome-stable", "com.google.Chrome").unwrap();
        let dev = match_score("google-chrome-stable", "com.google.ChromeDev");
        assert!(dev.is_none_or(|d| d < chrome));
    }

    #[test]
    fn regressions_still_match() {
        assert!(match_score("firefox", "org.mozilla.firefox").is_some());
        assert!(match_score("spotify", "com.spotify.Client").is_some());
        assert!(match_score("code", "com.visualstudio.code").is_some());
        assert!(match_score("Telegram", "org.telegram.desktop").is_some());
        assert_eq!(match_score("obs", "com.obsproject.Studio"), Some(u32::MAX));
        assert!(match_score("obs", "org.something.obs").is_none());
    }

    #[test]
    fn zed_editor_vs_spicedb_collision() {
        assert_eq!(match_score("zeditor", "dev.zed.Zed"), Some(u32::MAX));
        assert!(match_score("zed", "dev.zed.Zed").is_none());
    }

    #[test]
    fn rejects_unrelated() {
        assert!(match_score("microsoft-edge", "com.microsoft.Teams").is_none());
        assert!(match_score("signal-desktop", "org.telegram.desktop").is_none());
        assert!(match_score("gnome-terminal", "br.app.pw3270.terminal").is_none());
        assert!(match_score("muse-sounds-manager", "com.evepreview.manager").is_none());
        assert!(match_score("opencloud-dolphin", "org.kde.dolphin").is_none());
    }
}

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
