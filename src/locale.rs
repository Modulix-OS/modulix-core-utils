use crate::core::{
    option::Option as mxOption,
    transaction::transaction::{BuildCommand, Transaction},
};
use crate::mx;

const LOCALE_FILE_PATH: &str = "locale.nix";

pub fn set_locale_extra_settings(
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
    let mut transaction = Transaction::new("Set locale", BuildCommand::Switch)?;

    transaction.add_file(LOCALE_FILE_PATH)?;
    transaction.begin()?;

    let file = match transaction.get_file(LOCALE_FILE_PATH) {
        Ok(f) => f,
        Err(e) => {
            transaction.rollback()?;
            return Err(e);
        }
    };

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
        match mxOption::new(key).set(file, value) {
            Ok(()) => (),
            Err(e) => {
                transaction.rollback()?;
                return Err(e);
            }
        };
    }

    transaction.commit()?;
    Ok(())
}

pub fn set_locale(timezone: &str, default_locale: &str, console_keymap: &str) -> mx::Result<()> {
    set_locale_extra_settings(
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
