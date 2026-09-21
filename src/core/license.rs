//! Normalises license metadata (nixpkgs `meta.license`, Flathub
//! `project_license`) into the SPDX-ish string GNOME Software's
//! `gs_app_set_license()` expects.

use serde::Deserialize;

/// AppStream license ref standing for "unfree, exact license unknown",
/// recognised by `as_license_is_free_license()`.
pub const LICENSE_PROPRIETARY: &str = "LicenseRef-proprietary";

/// AppStream license ref standing for "free, exact license unknown".
pub const LICENSE_FREE: &str = "LicenseRef-free";

/// The three shapes `meta.license` takes in nixpkgs: a bare string (legacy
/// attributes), one `lib.licenses.*` attrset, or a list of either.
///
/// # Variants
/// * `Str` - the legacy spelling, an SPDX id or `"unfree"`/`"unfree-…"`.
/// * `One` - a single `lib.licenses.*` entry.
/// * `Many` - several licenses, each itself any of these shapes.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RawLicense {
    Str(String),
    One(LicenseObj),
    Many(Vec<RawLicense>),
}

/// A `lib.licenses.*` entry. Every field is optional — `free` is only spelled
/// out for unfree licenses, and a handful of entries carry no `spdxId`.
///
/// # Fields
/// * `spdx_id` - the SPDX id (`spdxId`), absent for a few nixpkgs entries.
/// * `free` - whether the license is free; `None` means free, since nixpkgs
///   only spells the field out to mark an unfree license.
#[derive(Debug, Deserialize)]
pub struct LicenseObj {
    #[serde(rename = "spdxId")]
    spdx_id: Option<String>,
    free: Option<bool>,
}

/// One license of an expression, after the nixpkgs shapes have been flattened.
///
/// # Variants
/// * `Proprietary` - an unfree license, whatever its id.
/// * `Spdx` - a license with a usable SPDX id, carried as payload.
/// * `FreeUnknown` - known free (or unmarked, which nixpkgs means as free) but
///   no SPDX id.
enum Term {
    Proprietary,
    Spdx(String),
    FreeUnknown,
}

/// Flattens a `meta.license` value into individual terms.
///
/// # Parameters
/// * `raw` - the license metadata, of any of the three nixpkgs shapes.
/// * `out` - accumulator the terms are appended to, in traversal order;
///   pre-existing entries are kept.
///
/// # Post-conditions
/// Nothing is appended for an empty string or an empty list, so `out` can come
/// back unchanged. A legacy `RawLicense::Str` value (`license = "unfree";` /
/// `"unfree-redistributable"`) becomes [`Term::Proprietary`] when it starts with
/// `"unfree"` (case-insensitive), and [`Term::Spdx`] otherwise.
fn terms(raw: &RawLicense, out: &mut Vec<Term>) {
    match raw {
        RawLicense::Str(s) => {
            let s = s.trim();
            if s.is_empty() {
                return;
            }
            out.push(if s.to_ascii_lowercase().starts_with("unfree") {
                Term::Proprietary
            } else {
                Term::Spdx(s.to_string())
            });
        }
        RawLicense::One(o) => {
            if o.free == Some(false) {
                out.push(Term::Proprietary);
                return;
            }
            match o
                .spdx_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(id) => out.push(Term::Spdx(id.to_string())),
                None => out.push(Term::FreeUnknown),
            }
        }
        RawLicense::Many(list) => {
            for item in list {
                terms(item, out);
            }
        }
    }
}

/// SPDX expression for `raw`, or `None` when it carries nothing usable.
///
/// A single unfree term taints the whole expression (that is what GNOME
/// Software's free/proprietary badge means for the user), and a term without
/// an SPDX id degrades the *whole* expression to [`LICENSE_FREE`] rather than
/// emitting a partial, misleading `AND` chain.
///
/// # Parameters
/// * `raw` - the `meta.license` value as deserialised from `nix eval`.
///
/// # Returns
/// [`LICENSE_PROPRIETARY`] if any term is unfree; the terms joined with ` AND `
/// when every one of them has an SPDX id; [`LICENSE_FREE`] when at least one is
/// free without an id; `None` when `raw` yields no term at all.
pub fn normalize(raw: &RawLicense) -> Option<String> {
    let mut parsed = Vec::new();
    terms(raw, &mut parsed);
    if parsed.is_empty() {
        return None;
    }
    if parsed.iter().any(|t| matches!(t, Term::Proprietary)) {
        return Some(LICENSE_PROPRIETARY.to_string());
    }
    let ids: Vec<&str> = parsed
        .iter()
        .filter_map(|t| match t {
            Term::Spdx(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    if ids.len() == parsed.len() {
        Some(ids.join(" AND "))
    } else {
        Some(LICENSE_FREE.to_string())
    }
}

/// Same, for the Flathub AppStream payload: `project_license` is already an
/// SPDX expression when present; `is_free_license` is the only signal left
/// otherwise.
///
/// # Parameters
/// * `project_license` - the component's `project_license`, if the payload
///   carried one.
/// * `is_free` - the payload's `is_free_license` flag, used only as a fallback.
///
/// # Returns
/// `project_license` trimmed when it is non-empty; else [`LICENSE_FREE`] or
/// [`LICENSE_PROPRIETARY`] according to `is_free`; else `None`.
pub fn from_flathub(project_license: Option<&str>, is_free: Option<bool>) -> Option<String> {
    if let Some(license) = project_license.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(license.to_string());
    }
    match is_free {
        Some(true) => Some(LICENSE_FREE.to_string()),
        Some(false) => Some(LICENSE_PROPRIETARY.to_string()),
        None => None,
    }
}

#[cfg(all(test, feature = "serde-json"))]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<String> {
        let raw: Option<RawLicense> = serde_json::from_str(json).unwrap();
        normalize(&raw?)
    }

    #[test]
    fn attrset_uses_spdx_id() {
        assert_eq!(
            parse(r#"{"spdxId":"MPL-2.0","fullName":"Mozilla Public License 2.0","free":true}"#),
            Some("MPL-2.0".to_string())
        );
    }

    #[test]
    fn unfree_attrset_is_proprietary() {
        assert_eq!(
            parse(r#"{"fullName":"Unfree redistributable","free":false}"#),
            Some(LICENSE_PROPRIETARY.to_string())
        );
    }

    #[test]
    fn list_joins_spdx_ids() {
        assert_eq!(
            parse(r#"[{"spdxId":"MIT","free":true},{"spdxId":"Apache-2.0","free":true}]"#),
            Some("MIT AND Apache-2.0".to_string())
        );
    }

    #[test]
    fn one_unfree_taints_the_list() {
        assert_eq!(
            parse(r#"[{"spdxId":"MIT","free":true},{"fullName":"EULA","free":false}]"#),
            Some(LICENSE_PROPRIETARY.to_string())
        );
    }

    #[test]
    fn missing_spdx_id_degrades_to_free_ref() {
        assert_eq!(
            parse(r#"[{"spdxId":"MIT"},{"fullName":"Homebrew license"}]"#),
            Some(LICENSE_FREE.to_string())
        );
    }

    #[test]
    fn legacy_string_forms() {
        assert_eq!(parse(r#""GPL-3.0-only""#), Some("GPL-3.0-only".to_string()));
        assert_eq!(parse(r#""unfree""#), Some(LICENSE_PROPRIETARY.to_string()));
        assert_eq!(parse(r#""""#), None);
    }

    #[test]
    fn empty_and_null_yield_nothing() {
        assert_eq!(parse("[]"), None);
        assert_eq!(parse("null"), None);
    }

    #[test]
    fn flathub_prefers_project_license() {
        assert_eq!(
            from_flathub(Some("GPL-3.0-or-later"), Some(true)),
            Some("GPL-3.0-or-later".to_string())
        );
        assert_eq!(
            from_flathub(Some("  "), Some(false)),
            Some(LICENSE_PROPRIETARY.to_string())
        );
        assert_eq!(
            from_flathub(None, Some(true)),
            Some(LICENSE_FREE.to_string())
        );
        assert_eq!(from_flathub(None, None), None);
    }
}
