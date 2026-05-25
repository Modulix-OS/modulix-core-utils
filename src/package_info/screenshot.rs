use super::Url;

#[derive(Debug)]
pub struct Screenshot<'a> {
    pub url: &'a Url,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct SizedScreenshot<'a> {
    pub screenshot: Vec<Screenshot<'a>>,
    pub caption: &'a str,
}

#[derive(Debug)]
pub struct AppSreenshot<'a> {
    pub default: usize,
    pub screenshots: Vec<SizedScreenshot<'a>>,
}
