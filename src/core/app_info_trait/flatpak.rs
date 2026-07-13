use crate::mx;

use super::app_info_screenshot::{AppScreenshot, Screenshot, SizedScreenshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct AppStreamScreenshot {
    pub sizes: Vec<AppStreamScreenshotSize>,
    pub caption: Option<String>,
    pub default: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AppStreamScreenshotSize {
    pub src: String,
    pub width: String,
    pub height: String,
    pub scale: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct AppStreamRelease {
    pub version: String,
    pub timestamp: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub release_type: Option<String>,
    pub urgency: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AppStreamUrls {
    pub homepage: Option<String>,
    pub bugtracker: Option<String>,
    pub help: Option<String>,
    pub donation: Option<String>,
    pub translate: Option<String>,
    pub faq: Option<String>,
    pub contact: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AppStreamBundle {
    pub value: String,
    pub runtime: Option<String>,
    pub sdk: Option<String>,
    #[serde(rename = "type")]
    pub bundle_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FlatpakInfo {
    id: String,
    name: String,
    summary: String,
    description: String,
    icon: String,
    developer_name: Option<String>,
    project_license: Option<String>,
    is_free_license: Option<bool>,
    is_eol: Option<bool>,
    #[serde(rename = "iupdatesMobileFriendly")]
    is_mobile_friendly: Option<bool>,
    categories: Option<Vec<String>>,
    keywords: Option<Vec<String>>,
    screenshots: Option<Vec<AppStreamScreenshot>>,
    releases: Option<Vec<AppStreamRelease>>,
    urls: Option<AppStreamUrls>,
    bundle: Option<AppStreamBundle>,
}

impl FlatpakInfo {
    pub async fn new(app_id: &str) -> mx::Result<Self> {
        let url = match crate::core::lang::current_lang() {
            Some(lang) => format!("https://flathub.org/api/v2/appstream/{app_id}?locale={lang}"),
            None => format!("https://flathub.org/api/v2/appstream/{app_id}"),
        };
        Ok(reqwest::get(url)
            .await
            .map_err(mx::ErrorKind::HttpError)?
            .error_for_status()
            .map_err(mx::ErrorKind::HttpError)?
            .json::<Self>()
            .await
            .map_err(mx::ErrorKind::HttpError)?)
    }

    pub fn icon(&self) -> &str {
        &self.icon
    }

    pub fn keywords(&self) -> Option<&[String]> {
        self.keywords.as_deref()
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn screenshots(&self) -> Option<AppScreenshot<'_>> {
        let screenshots = self.screenshots.as_ref()?;

        let app_screenshots: Vec<SizedScreenshot> = screenshots
            .iter()
            .map(|sizes| SizedScreenshot {
                screenshot: sizes
                    .sizes
                    .iter()
                    .map(|s| Screenshot {
                        url: &s.src,
                        width: s.width.parse().unwrap_or(1920),
                        height: s.height.parse().unwrap_or(1080),
                    })
                    .collect(),
                caption: sizes.caption.as_deref().unwrap_or(""),
            })
            .collect();
        Some(AppScreenshot {
            screenshots: app_screenshots,
            default: screenshots
                .iter()
                .position(|s| s.default.unwrap_or(false))
                .unwrap_or(0),
        })
    }
}
