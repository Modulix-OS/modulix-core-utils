//! The Flathub AppStream payload: the metadata nixpkgs does not carry (long
//! description, screenshots, keywords, license), fetched per application id.

use crate::mx;

use super::app_info_screenshot::{AppScreenshot, Screenshot, SizedScreenshot};
use serde::{Deserialize, Serialize};

/// One screenshot of the payload.
///
/// # Fields
/// * `sizes` - the same picture in the resolutions Flathub generated.
/// * `caption` - its caption, absent when the app declares none.
/// * `default` - whether this is the app's showcase screenshot.
#[derive(Debug, Deserialize, Serialize)]
struct AppStreamScreenshot {
    pub sizes: Vec<AppStreamScreenshotSize>,
    pub caption: Option<String>,
    pub default: Option<bool>,
}

/// One rendition of a screenshot.
///
/// # Fields
/// * `src` - URL of the image.
/// * `width` - width in pixels, as a string in the payload.
/// * `height` - height in pixels, as a string in the payload.
/// * `scale` - HiDPI scale factor this rendition is meant for.
#[derive(Debug, Deserialize, Serialize)]
struct AppStreamScreenshotSize {
    pub src: String,
    pub width: String,
    pub height: String,
    pub scale: String,
}

/// One release entry of the payload's changelog.
///
/// # Fields
/// * `version` - the version string.
/// * `timestamp` - release date as a Unix timestamp, in string form.
/// * `description` - the changelog text.
/// * `release_type` - `stable`, `development`, … as the payload's `type`.
/// * `urgency` - how urgently the release should be applied.
#[derive(Debug, Deserialize, Serialize)]
struct AppStreamRelease {
    pub version: String,
    pub timestamp: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub release_type: Option<String>,
    pub urgency: Option<String>,
}

/// The links an app declares, each absent when it declares none.
///
/// # Fields
/// * `homepage` - the project's home page.
/// * `bugtracker` - where to report a bug.
/// * `help` - user documentation.
/// * `donation` - how to support the project.
/// * `translate` - where to help translate it.
/// * `faq` - frequently asked questions.
/// * `contact` - how to reach the authors.
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

/// How the app is packaged on Flathub - kept for completeness of the payload,
/// since Modulix installs from nixpkgs rather than from the bundle.
///
/// # Fields
/// * `value` - the bundle reference.
/// * `runtime` - runtime it needs.
/// * `sdk` - SDK it was built against.
/// * `bundle_type` - bundle kind, as the payload's `type`.
#[derive(Debug, Deserialize, Serialize)]
struct AppStreamBundle {
    pub value: String,
    pub runtime: Option<String>,
    pub sdk: Option<String>,
    #[serde(rename = "type")]
    pub bundle_type: Option<String>,
}

/// The Flathub AppStream payload of one application.
///
/// # Fields
/// * `id` - AppStream component id the payload was fetched for.
/// * `name` - application name.
/// * `summary` - one-line description.
/// * `description` - long description, as HTML.
/// * `icon` - URL of the application icon.
/// * `developer_name` - who publishes it.
/// * `project_license` - SPDX expression, when declared.
/// * `is_free_license` - Flathub's own free/unfree verdict, used when
///   `project_license` is missing.
/// * `is_eol` - whether the app is marked end-of-life.
/// * `is_mobile_friendly` - whether it works on a phone form factor (the
///   payload's misspelled `iupdatesMobileFriendly`).
/// * `categories` - AppStream categories.
/// * `keywords` - extra search terms.
/// * `screenshots` - screenshots, each in several sizes.
/// * `releases` - changelog entries, newest first.
/// * `urls` - the app's links.
/// * `bundle` - Flatpak packaging details.
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
    /// Fetches an application's AppStream payload from Flathub.
    ///
    /// # Parameters
    /// * `app_id` - AppStream component id, used as-is in the request path.
    ///
    /// # Returns
    /// The payload, localised to the process locale when
    /// `crate::core::lang::current_lang` reports one.
    ///
    /// # Pre-conditions
    /// Needs network access to `flathub.org`; the call goes over the crate's
    /// shared HTTP client and its timeouts.
    ///
    /// # Errors
    /// [`mx::ErrorKind::HttpError`] when the request fails, the response status
    /// is an error - which includes an id Flathub does not know - or the body
    /// does not deserialise; [`mx::ErrorKind::RequestSenderError`] when the
    /// shared client cannot be built.
    pub async fn new(app_id: &str) -> mx::Result<Self> {
        let url = match crate::core::lang::current_lang() {
            Some(lang) => format!("https://flathub.org/api/v2/appstream/{app_id}?locale={lang}"),
            None => format!("https://flathub.org/api/v2/appstream/{app_id}"),
        };
        Ok(crate::core::http_client::client()?
            .get(url)
            .send()
            .await
            .map_err(mx::ErrorKind::HttpError)?
            .error_for_status()
            .map_err(mx::ErrorKind::HttpError)?
            .json::<Self>()
            .await
            .map_err(mx::ErrorKind::HttpError)?)
    }

    /// SPDX expression for the app, from the Flathub AppStream payload.
    ///
    /// # Returns
    /// The declared `project_license` when there is one, else
    /// [`crate::core::license::LICENSE_FREE`] or
    /// [`crate::core::license::LICENSE_PROPRIETARY`] from Flathub's own verdict,
    /// else `None`.
    pub fn license(&self) -> Option<String> {
        crate::core::license::from_flathub(self.project_license.as_deref(), self.is_free_license)
    }

    /// URL of the application icon.
    ///
    /// # Returns
    /// The URL, borrowed from `self`; empty when the payload carried none.
    pub fn icon(&self) -> &str {
        &self.icon
    }

    /// Extra search terms the app declares.
    ///
    /// # Returns
    /// The keywords, or `None` when the payload carried none.
    pub fn keywords(&self) -> Option<&[String]> {
        self.keywords.as_deref()
    }

    /// The long description.
    ///
    /// # Returns
    /// The description as HTML, borrowed from `self`; a consumer that needs
    /// plain text has to strip the markup itself.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The app's screenshots, in this crate's own shape.
    ///
    /// # Returns
    /// The screenshot set, borrowing the URLs and captions from `self`, or
    /// `None` when the payload carried no screenshot. A size whose dimensions do
    /// not parse falls back to 1920×1080, a missing caption to the empty string,
    /// and a set without a default screenshot to the first one.
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
