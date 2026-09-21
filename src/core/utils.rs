//! Conversions between a raw value and its Nix string literal form.

use crate::mx;

/// Quotes `value` as a Nix double-quoted string literal.
///
/// # Parameters
/// * `value` - the raw text to quote; it is inserted as-is, so it must not
///   contain a character needing a Nix escape.
///
/// # Returns
/// `"<value>"`, ready to be written into a configuration file.
pub fn value_to_string_nix(value: &str) -> String {
    String::from("\"") + value + "\""
}

/// Quotes `value` as a Nix indented-string literal, for text spanning several
/// lines.
///
/// # Parameters
/// * `value` - the raw text to quote, inserted as-is.
///
/// # Returns
/// `'''<value>'''`.
pub fn value_to_block_string_nix(value: &str) -> String {
    String::from("'''") + value + "'''"
}

/// Unquotes a Nix string literal, inverse of [`value_to_string_nix`] and
/// [`value_to_block_string_nix`].
///
/// # Parameters
/// * `str_nix` - a literal delimited by `"` or by `'''`.
///
/// # Returns
/// The contents without the delimiters, borrowed from `str_nix`; escapes are
/// not interpreted.
///
/// # Errors
/// [`mx::ErrorKind::InvalidNixString`] when `str_nix` carries no delimiter, or
/// carries an opening one without its matching closing one.
pub fn string_nix_to_value(str_nix: &str) -> mx::Result<&str> {
    match str_nix.strip_prefix('"') {
        Some(s) => match s.strip_suffix('"') {
            Some(s) => Ok(s),
            None => Err(mx::ErrorKind::InvalidNixString),
        },
        None => match str_nix.strip_prefix("'''") {
            Some(s) => s.strip_suffix("'''").ok_or(mx::ErrorKind::InvalidNixString),
            None => Err(mx::ErrorKind::InvalidNixString),
        },
    }
}
