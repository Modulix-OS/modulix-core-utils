use std::io::Read;
use std::process;

use crate::mx;

// ─── Unfree package source ───────────────────────────────────────────────────
//
// Maps a nixpkgs package name to its .desktop source:
//   - FlathubId(&str)    : fetch from Flathub AppStream API using this app-id
//   - SameAs(&str)       : reuse the .desktop of another nixpkgs package
//
enum UnfreeDesktopSource {
    FlathubId(&'static str),
    SameAs(&'static str),
}

use UnfreeDesktopSource::*;

static UNFREE_DESKTOP_SOURCES: phf::Map<&'static str, UnfreeDesktopSource> = phf::phf_map! {
    "discord"        => FlathubId("com.discordapp.Discord"),
    "slack"          => FlathubId("com.slack.Slack"),
    "spotify"        => FlathubId("com.spotify.Client"),
    "zoom-us"        => FlathubId("us.zoom.Zoom"),
    "obsidian"       => FlathubId("md.obsidian.Obsidian"),
    "vscode"         => FlathubId("com.visualstudio.code"),
    "teams"          => FlathubId("com.microsoft.Teams"),
    "firefox-bin" => SameAs("firefox"),
};

// ─── NAR parser ───────────────────────────────────────────────────────────────
//
// Format NAR : chaque string = u64 LE + bytes + padding au multiple de 8
//
// Règle : chaque parse_node reçoit un noeud dont le "(" a déjà été consommé
//         par l'appelant, et consomme lui-même son ")" final.
//
struct NarReader<R: Read> {
    inner: R,
}

impl<R: Read> NarReader<R> {
    fn new(inner: R) -> Self {
        Self { inner }
    }

    fn read_bytes(&mut self) -> mx::Result<Vec<u8>> {
        let mut len_buf = [0u8; 8];
        self.inner
            .read_exact(&mut len_buf)
            .map_err(mx::ErrorKind::IOError)?;
        let len = u64::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.inner
            .read_exact(&mut buf)
            .map_err(mx::ErrorKind::IOError)?;

        let pad = (8 - (len % 8)) % 8;
        if pad > 0 {
            let mut pad_buf = vec![0u8; pad];
            self.inner
                .read_exact(&mut pad_buf)
                .map_err(mx::ErrorKind::IOError)?;
        }

        Ok(buf)
    }

    fn read_str(&mut self) -> mx::Result<String> {
        let bytes = self.read_bytes()?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn expect(&mut self, expected: &str) -> mx::Result<()> {
        let s = self.read_str()?;
        if s != expected {
            return Err(mx::ErrorKind::NixCommandError(format!(
                "NAR parse error: expected {:?}, got {:?}",
                expected, s
            )));
        }
        Ok(())
    }

    fn find_desktop(&mut self) -> mx::Result<String> {
        let magic = self.read_str()?;
        if magic != "nix-archive-1" {
            return Err(mx::ErrorKind::NixCommandError(format!(
                "Invalid NAR magic: {}",
                magic
            )));
        }
        self.expect("(")?;
        self.parse_node(&mut Vec::new())
    }

    fn parse_node(&mut self, path: &mut Vec<String>) -> mx::Result<String> {
        self.expect("type")?;
        let node_type = self.read_str()?;

        match node_type.as_str() {
            "regular" => {
                let is_desktop = path
                    .last()
                    .map(|n| n.ends_with(".desktop"))
                    .unwrap_or(false);

                let mut desktop_content: Option<String> = None;

                loop {
                    let token = self.read_str()?;
                    match token.as_str() {
                        "executable" => {
                            self.read_str()?; // ""
                        }
                        "contents" => {
                            let bytes = self.read_bytes()?;
                            if is_desktop {
                                desktop_content =
                                    Some(String::from_utf8_lossy(&bytes).into_owned());
                            }
                        }
                        ")" => break,
                        _ => {}
                    }
                }

                desktop_content.ok_or(mx::ErrorKind::DesktopFileNotFound)
            }

            "directory" => {
                let mut found: Option<String> = None;

                loop {
                    let token = self.read_str()?;
                    match token.as_str() {
                        "entry" => {
                            self.expect("(")?;
                            self.expect("name")?;
                            let entry_name = self.read_str()?;
                            self.expect("node")?;
                            self.expect("(")?;

                            path.push(entry_name);
                            let result = self.parse_node(path);
                            path.pop();

                            self.expect(")")?; // ferme "entry ("

                            if found.is_none() {
                                found = result.ok();
                            }
                        }
                        ")" => break,
                        _ => {}
                    }
                }

                found.ok_or(mx::ErrorKind::DesktopFileNotFound)
            }

            "symlink" => {
                self.expect("target")?;
                self.read_str()?;
                self.expect(")")?;
                Err(mx::ErrorKind::DesktopFileNotFound)
            }

            _ => Err(mx::ErrorKind::NixCommandError(format!(
                "Unknown NAR node type: {}",
                node_type
            ))),
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn is_unfree(package: &str) -> bool {
    process::Command::new("nix")
        .args([
            "eval",
            "--json",
            &format!("nixpkgs#{}.meta.unfree", package),
        ])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                serde_json::from_slice::<bool>(&o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or(false)
}

fn fetch_url(client: &reqwest::blocking::Client, url: &str) -> mx::Result<String> {
    client
        .get(url)
        .send()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?
        .text()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))
}

// ─── Flathub AppStream ────────────────────────────────────────────────────────

fn fetch_flathub_desktop(app_id: &str) -> mx::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("get-desktop/0.1")
        .build()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let url = format!("https://flathub.org/api/v2/appstream/{}", app_id);
    let resp: serde_json::Value = client
        .get(&url)
        .send()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?
        .json()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let name = resp["name"].as_str().unwrap_or(app_id);
    let comment = resp["summary"].as_str().unwrap_or("");
    let categories = resp["categories"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(";")
        })
        .unwrap_or_default();

    // Reconstruit un .desktop minimal depuis les métadonnées AppStream
    let desktop = format!(
        "[Desktop Entry]
         Type=Application
         Name={name}
         Comment={comment}
         Exec={app_id}
         Icon={app_id}
         Categories={categories};
         Terminal=false
",
    );

    Ok(desktop)
}

// ─── GitHub ───────────────────────────────────────────────────────────────────

fn try_github(package: &str) -> mx::Result<String> {
    let output = process::Command::new("nix")
        .args([
            "eval",
            "--raw",
            &format!("nixpkgs#{}.meta.homepage", package),
        ])
        .output()
        .map_err(mx::ErrorKind::IOError)?;

    if !output.status.success() {
        return Err(mx::ErrorKind::DesktopFileNotFound);
    }

    let homepage = String::from_utf8(output.stdout).map_err(mx::ErrorKind::FromUtf8Error)?;

    if !homepage.contains("github.com") {
        return Err(mx::ErrorKind::DesktopFileNotFound);
    }

    let repo_path = homepage
        .split("github.com/")
        .nth(1)
        .and_then(|s| {
            let parts: Vec<&str> = s.trim_end_matches('/').splitn(2, '/').collect();
            if parts.len() == 2 {
                Some(format!(
                    "{}/{}",
                    parts[0],
                    parts[1].split('/').next().unwrap_or("")
                ))
            } else {
                None
            }
        })
        .ok_or(mx::ErrorKind::DesktopFileNotFound)?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("get-desktop/0.1")
        .build()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let api_url = format!(
        "https://api.github.com/search/code?q=extension:desktop+repo:{}",
        repo_path
    );

    let resp: serde_json::Value = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?
        .json()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let items = resp["items"]
        .as_array()
        .ok_or(mx::ErrorKind::DesktopFileNotFound)?;

    if items.is_empty() {
        return Err(mx::ErrorKind::DesktopFileNotFound);
    }

    let html_url = items[0]["html_url"]
        .as_str()
        .ok_or(mx::ErrorKind::DesktopFileNotFound)?;

    let raw_url = html_url
        .replace("github.com", "raw.githubusercontent.com")
        .replace("/blob/", "/");

    fetch_url(&client, &raw_url)
}

// ─── NAR ──────────────────────────────────────────────────────────────────────

fn try_nar(package: &str) -> mx::Result<String> {
    let out_path = process::Command::new("nix")
        .args(["eval", "--raw", &format!("nixpkgs#{}.out.outPath", package)])
        .output()
        .map_err(mx::ErrorKind::IOError)
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).map_err(mx::ErrorKind::FromUtf8Error)
            } else {
                Err(mx::ErrorKind::DesktopFileNotFound)
            }
        })
        .or_else(|_| {
            process::Command::new("nix")
                .args(["eval", "--raw", &format!("nixpkgs#{}.outPath", package)])
                .output()
                .map_err(mx::ErrorKind::IOError)
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout).map_err(mx::ErrorKind::FromUtf8Error)
                    } else {
                        Err(mx::ErrorKind::DesktopFileNotFound)
                    }
                })
        })?;

    let hash = out_path
        .split('/')
        .nth(3)
        .and_then(|s| s.split('-').next())
        .ok_or(mx::ErrorKind::DesktopFileNotFound)?
        .to_string();

    let client = reqwest::blocking::Client::new();

    let narinfo = client
        .get(format!("https://cache.nixos.org/{}.narinfo", hash))
        .send()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?
        .text()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    let nar_url = narinfo
        .lines()
        .find(|l| l.starts_with("URL:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .ok_or(mx::ErrorKind::DesktopFileNotFound)?
        .to_string();

    let compression = narinfo
        .lines()
        .find(|l| l.starts_with("Compression:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("xz")
        .to_string();

    let nar_bytes = client
        .get(format!("https://cache.nixos.org/{}", nar_url))
        .send()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?
        .bytes()
        .map_err(|e| mx::ErrorKind::NixCommandError(e.to_string()))?;

    match compression.as_str() {
        "xz" => NarReader::new(xz2::read::XzDecoder::new(nar_bytes.as_ref())).find_desktop(),
        "none" => NarReader::new(nar_bytes.as_ref()).find_desktop(),
        other => Err(mx::ErrorKind::NixCommandError(format!(
            "Unsupported compression: {}",
            other
        ))),
    }
}

pub fn get_desktop_file(package: &str) -> mx::Result<String> {
    if is_unfree(package) {
        return match UNFREE_DESKTOP_SOURCES.get(package) {
            Some(FlathubId(app_id)) => fetch_flathub_desktop(app_id),
            Some(SameAs(other)) => get_desktop_file(other),
            None => Err(mx::ErrorKind::NixCommandError(format!(
                "Unfree package '{}' has no registered .desktop source",
                package
            ))),
        };
    }

    try_github(package).or_else(|_| try_nar(package))
}
