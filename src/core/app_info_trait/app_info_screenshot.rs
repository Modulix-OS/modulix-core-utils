//! Screenshot metadata, in the three levels AppStream describes it with: one
//! image, one screenshot in its available sizes, and the app's whole set.

use super::Url;

/// One image file of a screenshot.
///
/// # Fields
/// * `url` - where to download the image.
/// * `width` - its width in pixels.
/// * `height` - its height in pixels.
#[derive(Debug)]
pub struct Screenshot<'a> {
    pub url: &'a Url,
    pub width: u32,
    pub height: u32,
}

/// One screenshot, in every size the source offers.
///
/// # Fields
/// * `screenshot` - the same picture at different resolutions, for the UI to
///   pick from.
/// * `caption` - the caption to show; empty when the source has none.
#[derive(Debug)]
pub struct SizedScreenshot<'a> {
    pub screenshot: Vec<Screenshot<'a>>,
    pub caption: &'a str,
}

/// An application's whole screenshot set.
///
/// # Fields
/// * `default` - index into `screenshots` of the one to show first.
/// * `screenshots` - the screenshots, in the order the source listed them.
#[derive(Debug)]
pub struct AppScreenshot<'a> {
    pub default: usize,
    pub screenshots: Vec<SizedScreenshot<'a>>,
}
