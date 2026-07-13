//! Process locale detection for localizing module metadata.
//!
//! Reads the POSIX locale environment (`LC_ALL` → `LC_MESSAGES` → `LANG`) once and
//! exposes the language subtag used to select a translation overlay. English is the
//! canonical/base language of the authored JSON, so it is treated as "no overlay".

use std::sync::OnceLock;

/// Base language of the authored JSON; requesting it means "serve the base files".
pub const DEFAULT_LANG: &str = "en";

/// Extract the language subtag from a raw POSIX locale string
/// (`fr_FR.UTF-8@euro` → `fr`). Returns `None` for an empty locale, the neutral
/// `C`/`POSIX` locales, and for [`DEFAULT_LANG`] — every case where the caller
/// should serve the base English text.
fn lang_from_locale(raw: &str) -> Option<String> {
    let lang = raw
        .split(['_', '.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match lang.as_str() {
        "" | "c" | "posix" => None,
        s if s == DEFAULT_LANG => None,
        _ => Some(lang),
    }
}

/// Language subtag of the current process locale, or `None` when the base
/// (English) text should be used.
///
/// Resolution follows POSIX precedence `LC_ALL` → `LC_MESSAGES` → `LANG`, taking
/// the first non-empty value's language subtag. Computed once and cached. A `None`
/// result tells callers to skip overlay/`?locale` handling and serve the base text.
pub fn current_lang() -> Option<&'static str> {
    static LANG: OnceLock<Option<String>> = OnceLock::new();
    LANG.get_or_init(|| {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|value| !value.is_empty()))
            .and_then(|raw| lang_from_locale(&raw))
    })
    .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_subtag() {
        assert_eq!(lang_from_locale("fr_FR.UTF-8"), Some("fr".to_string()));
        assert_eq!(lang_from_locale("fr"), Some("fr".to_string()));
        assert_eq!(lang_from_locale("de_DE.UTF-8@euro"), Some("de".to_string()));
        assert_eq!(lang_from_locale("PT_br"), Some("pt".to_string()));
    }

    #[test]
    fn base_and_neutral_locales_are_none() {
        assert_eq!(lang_from_locale("en_US.UTF-8"), None);
        assert_eq!(lang_from_locale("en"), None);
        assert_eq!(lang_from_locale("C"), None);
        assert_eq!(lang_from_locale("POSIX"), None);
        assert_eq!(lang_from_locale(""), None);
    }
}
