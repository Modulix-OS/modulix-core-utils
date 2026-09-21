//! Sets the system's timezone, locales and keyboard layouts in the
//! configuration's `locale.nix`.

use crate::core::{
    option::Option as mxOption,
    transaction::{
        self,
        file_lock::NixFile,
        transaction::{BuildCommand, UpdateInput},
    },
};
use crate::mx;

/// Configuration file, relative to the config directory, holding the timezone,
/// the locales and both keyboard layouts.
pub(crate) const LOCALE_FILE_PATH: &str = "locale.nix";

/// Sets the timezone, the default locale, every `LC_*` category and the
/// console keymap, in an already-open `locale.nix`.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `timezone` - tz database name for `time.timeZone` (e.g. `Europe/Paris`).
/// * `default_locale` - locale for `i18n.defaultLocale` (e.g. `fr_FR.UTF-8`).
/// * `lc_ctype` - value of `LC_CTYPE` (character classification).
/// * `lc_address` - value of `LC_ADDRESS` (postal address format).
/// * `lc_measurement` - value of `LC_MEASUREMENT` (unit system).
/// * `lc_message` - value of `LC_MESSAGES` (interface language).
/// * `lc_monetary` - value of `LC_MONETARY` (currency format).
/// * `lc_name` - value of `LC_NAME` (personal name format).
/// * `lc_numeric` - value of `LC_NUMERIC` (number format).
/// * `lc_paper` - value of `LC_PAPER` (paper size).
/// * `lc_telephone` - value of `LC_TELEPHONE` (phone number format).
/// * `lc_time` - value of `LC_TIME` (date and time format).
/// * `lc_collate` - value of `LC_COLLATE` (sort order).
/// * `console_keymap` - keymap for `console.keyMap` (e.g. `fr`), which is the
///   TTY layout; the graphical one is set by [`set_keyboard_no_transaction`].
///
/// # Post-conditions
/// Each option is assigned unconditionally, overwriting what the file held.
/// Values are quoted here, so pass them unquoted; none is validated against
/// the locales the system actually generates.
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

/// Sets the timezone, the locale and the console keymap in an already-open
/// `locale.nix`, the simple case where one locale covers every category.
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `timezone` - tz database name for `time.timeZone`.
/// * `default_locale` - locale used both for `i18n.defaultLocale` and for
///   every `LC_*` category.
/// * `console_keymap` - keymap for `console.keyMap`.
///
/// # Post-conditions
/// As in [`set_locale_extra_settings_no_transaction`], which this delegates to
/// with `default_locale` repeated for all categories.
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
///
/// # Parameters
/// * `file` - the open configuration file to edit.
/// * `kb_layout` - value of `services.xserver.xkb.layout` (e.g. `fr`).
/// * `kb_variant` - value of `services.xserver.xkb.variant` (e.g. `azerty`);
///   pass an empty string for the layout's default variant.
///
/// # Post-conditions
/// Both options are assigned unconditionally. The console keymap is a separate
/// setting, handled by [`set_locale_no_transaction`].
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

/// Sets the timezone, every locale category and the console keymap, then
/// rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `timezone`, `default_locale`, `lc_*`, `console_keymap` - as in
///   [`set_locale_extra_settings_no_transaction`].
///
/// # Post-conditions
/// On success the settings are part of the active generation; on error the
/// configuration is rolled back. Blocks for the whole `nixos-rebuild switch`.
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

/// Sets the timezone, the locale and the console keymap, then rebuilds the
/// system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `timezone`, `default_locale`, `console_keymap` - as in
///   [`set_locale_no_transaction`].
///
/// # Post-conditions
/// As in [`set_locale_extra_settings`].
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

/// Sets the graphical keyboard layout and rebuilds the system.
///
/// # Parameters
/// * `config_dir` - configuration repository to edit.
/// * `kb_layout`, `kb_variant` - as in [`set_keyboard_no_transaction`].
///
/// # Post-conditions
/// As in [`set_locale_extra_settings`].
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
