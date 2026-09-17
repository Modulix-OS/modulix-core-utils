use crate::core::{
    option::Option as mxOption,
    transaction::{
        self,
        file_lock::NixFile,
        transaction::{BuildCommand, UpdateInput},
    },
};
use crate::mx;

pub(crate) const LOCALE_FILE_PATH: &str = "locale.nix";

// --- no_transaction ---

pub fn set_locale_extra_settings_no_transaction(
    file: &mut NixFile,
    timezone: &str,
    default_locale: &str,
    lc_ctype: &str,
    lc_address: &str,
    lc_measurement: &str,
    lc_message: &str,
    lc_monetary: &str,
    lc_name: &str,
    lc_numeric: &str,
    lc_paper: &str,
    lc_telephone: &str,
    lc_time: &str,
    lc_collate: &str,
    console_keymap: &str,
) -> mx::Result<()> {
    let options = [
        ("time.timeZone", format!("\"{}\"", timezone)),
        ("i18n.defaultLocale", format!("\"{}\"", default_locale)),
        (
            "i18n.extraLocaleSettings.LC_CTYPE",
            format!("\"{}\"", lc_ctype),
        ),
        (
            "i18n.extraLocaleSettings.LC_ADDRESS",
            format!("\"{}\"", lc_address),
        ),
        (
            "i18n.extraLocaleSettings.LC_MEASUREMENT",
            format!("\"{}\"", lc_measurement),
        ),
        (
            "i18n.extraLocaleSettings.LC_MESSAGES",
            format!("\"{}\"", lc_message),
        ),
        (
            "i18n.extraLocaleSettings.LC_MONETARY",
            format!("\"{}\"", lc_monetary),
        ),
        (
            "i18n.extraLocaleSettings.LC_NAME",
            format!("\"{}\"", lc_name),
        ),
        (
            "i18n.extraLocaleSettings.LC_NUMERIC",
            format!("\"{}\"", lc_numeric),
        ),
        (
            "i18n.extraLocaleSettings.LC_PAPER",
            format!("\"{}\"", lc_paper),
        ),
        (
            "i18n.extraLocaleSettings.LC_TELEPHONE",
            format!("\"{}\"", lc_telephone),
        ),
        (
            "i18n.extraLocaleSettings.LC_TIME",
            format!("\"{}\"", lc_time),
        ),
        (
            "i18n.extraLocaleSettings.LC_COLLATE",
            format!("\"{}\"", lc_collate),
        ),
        ("console.keyMap", format!("\"{}\"", console_keymap)),
    ];

    for (key, value) in &options {
        mxOption::new(key).set(file, value)?;
    }

    Ok(())
}

pub fn set_locale_no_transaction(
    file: &mut NixFile,
    timezone: &str,
    default_locale: &str,
    console_keymap: &str,
) -> mx::Result<()> {
    set_locale_extra_settings_no_transaction(
        file,
        timezone,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        default_locale,
        console_keymap,
    )
}

/// Sets the X11 keyboard layout.
///
/// Lives next to `console.keyMap` in `locale.nix` so both keyboards are described in
/// one place. mxpkgs sets these two options with `lib.mkMxDefault` (priority 900) in
/// `modulixos/desktop/default.nix`; a plain definition here is priority 100, so it
/// wins.
pub fn set_keyboard_no_transaction(
    file: &mut NixFile,
    kb_layout: &str,
    kb_variant: &str,
) -> mx::Result<()> {
    let options = [
        ("services.xserver.xkb.layout", format!("\"{}\"", kb_layout)),
        (
            "services.xserver.xkb.variant",
            format!("\"{}\"", kb_variant),
        ),
    ];

    for (key, value) in &options {
        mxOption::new(key).set(file, value)?;
    }

    Ok(())
}

// --- transaction ---

pub fn set_locale_extra_settings(
    config_dir: &str,
    timezone: &str,
    default_locale: &str,
    lc_ctype: &str,
    lc_address: &str,
    lc_measurement: &str,
    lc_message: &str,
    lc_monetary: &str,
    lc_name: &str,
    lc_numeric: &str,
    lc_paper: &str,
    lc_telephone: &str,
    lc_time: &str,
    lc_collate: &str,
    console_keymap: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        "Set locale",
        config_dir,
        LOCALE_FILE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| {
            set_locale_extra_settings_no_transaction(
                file,
                timezone,
                default_locale,
                lc_ctype,
                lc_address,
                lc_measurement,
                lc_message,
                lc_monetary,
                lc_name,
                lc_numeric,
                lc_paper,
                lc_telephone,
                lc_time,
                lc_collate,
                console_keymap,
            )
        },
    )
}

pub fn set_locale(
    config_dir: &str,
    timezone: &str,
    default_locale: &str,
    console_keymap: &str,
) -> mx::Result<()> {
    transaction::make_transaction(
        "Set locale",
        config_dir,
        LOCALE_FILE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| set_locale_no_transaction(file, timezone, default_locale, console_keymap),
    )
}

pub fn set_keyboard(config_dir: &str, kb_layout: &str, kb_variant: &str) -> mx::Result<()> {
    transaction::make_transaction(
        "Set keyboard layout",
        config_dir,
        LOCALE_FILE_PATH,
        BuildCommand::Switch,
        UpdateInput::Keep,
        |file| set_keyboard_no_transaction(file, kb_layout, kb_variant),
    )
}
