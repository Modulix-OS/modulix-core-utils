use modulix_core_utils::mx;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct FlathubAppInfo {
    icon: Option<String>,
    keywords: Option<Vec<String>>,
}

async fn get_flathub_app_info(
    client: &reqwest::Client,
    app_id: &str,
) -> Option<(String, Vec<String>)> {
    let info = client
        .get(format!("https://flathub.org/api/v2/appstream/{app_id}"))
        .send()
        .await
        .ok()?
        .json::<FlathubAppInfo>()
        .await
        .ok()?;
    let icon = info.icon?;
    let keywords = info.keywords.unwrap_or_default();
    Some((icon, keywords))
}

async fn get_flathub_id() -> mx::Result<Vec<String>> {
    reqwest::get("https://flathub.org/api/v2/appstream")
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))?
        .json()
        .await
        .map_err(|e| mx::ErrorKind::HttpError(e))
}

pub fn matches_flathub(exe: &str, app_id: &str) -> bool {
    let segments: Vec<&str> = app_id.split('.').collect();
    let n = segments.len();
    if n == 0 {
        return false;
    }
    let exe_lower = exe.to_lowercase();
    let last = segments[n - 1].to_lowercase();
    if exe_lower == last {
        return true;
    }
    if n >= 2 {
        let middle = segments[n - 2].to_lowercase();
        let two = format!("{}{}", segments[n - 2].to_lowercase(), last);
        if exe_lower == two || exe_lower == middle {
            return true;
        }
    }
    false
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
            let app_id = flathub_ids
                .iter()
                .find(|id| matches_flathub(exe, id))?
                .clone();
            Some((exe.clone(), app_id))
        })
        .collect();

    let client = reqwest::Client::new();

    let futures: Vec<_> = matched
        .into_iter()
        .map(|(exe, app_id)| async {
            let result = get_flathub_app_info(&client, &app_id).await;
            (exe, app_id, result)
        })
        .collect();

    futures::future::join_all(futures)
        .await
        .into_iter()
        .filter_map(|(exe, app_id, result)| {
            let (icon, keywords) = result?;
            Some((exe, (app_id, icon, keywords)))
        })
        .collect()
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn generate_file(enriched: &[(String, String, Option<(String, String, Vec<String>)>)]) {
    let mut out = String::new();

    out.push_str("static NIX_INFO: phf::Map<&'static str, NixInfo> = phf::phf_map! {\n");
    for (pkg, _, info) in enriched {
        if let Some((app_id, icon, keywords)) = info {
            let kw = keywords
                .iter()
                .map(|k| format!("\"{}\"", escape(k)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "    \"{}\" => NixInfo {{ app_id: \"{}\", icon: \"{}\", keywords: &[{}] }},\n",
                escape(pkg),
                escape(app_id),
                escape(icon),
                kw,
            ));
        }
    }
    out.push_str("};\n");

    std::fs::write("flathub_basic_info.rs", out)
        .expect("Imposible to generate flathub_basic_info.rs");
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
