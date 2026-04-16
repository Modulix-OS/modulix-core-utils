use crate::mx;

pub fn value_to_string_nix(value: &str) -> String {
    String::from("\"") + value + "\""
}

pub fn value_to_block_string_nix(value: &str) -> String {
    String::from("'''") + value + "'''"
}

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
