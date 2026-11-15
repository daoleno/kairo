//! The window's language. Its text comes from `locales/` through NSH's `t!`
//! (`t!("settings.language.title")`), which follows the person's languages,
//! or the one Settings picks (`set_locale`). What Kairo says outside the
//! window (the tray, notifications, the engine's messages, the platform's
//! toolbar and sidebar) uses `kairo_core::tr!` over the same files, in the
//! language the window tells it ([`shown`]).

use nsh_core::element::{global, locale, locale_settings};
use nsh_core::i18n::Translations;
use nsh_core::prelude::*;

/// The catalog, for a root that is not built by `nsh` (the desktop binary).
pub fn translations() -> Translations {
    Translations::from_map(kairo_core::i18n::catalog())
}

/// The catalog language the window's text is in (`en`, `zh`), as NSH picks
/// it: the first of the person's languages it has, else the locale's.
/// Kairo's text outside the window follows it before anything that reads it
/// runs again, so a page built anew reads both in the same language.
pub fn shown() -> Memo<String> {
    computed(move || {
        let tag = global::<Translations>().with(|t| {
            t.pick(locale_settings().languages.iter())
                .unwrap_or_else(locale)
        });
        kairo_core::i18n::set_language(tag.as_str());
        kairo_core::i18n::language()
    })
}

/// The key for `n` of something: `<key>.one` for 1, else `<key>.other`
/// (English counts so; Chinese has the same text in both).
pub fn plural(key: &str, n: impl Into<u64>) -> String {
    if n.into() == 1 {
        format!("{key}.one")
    } else {
        format!("{key}.other")
    }
}
