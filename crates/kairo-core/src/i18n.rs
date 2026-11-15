//! Kairo's own words in the language the person reads: the app's `locales/`
//! files, which NSH's `t!` reads in the window, read here with rust-i18n for
//! what Kairo says outside it (the tray, notifications, the engine's
//! messages). English is the source; a key without a translation shows in
//! English.
//!
//! The window decides the language (the system's, or the one picked in
//! Settings) and tells it here ([`set_language`]); until it does, the
//! process starts in the setting's or the environment's ([`start`]).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::LazyLock;

use tokio::sync::watch;

use crate::settings::Language;

/// The catalog's language of the text shown now (`en`, `zh`), for whoever
/// says something outside the window; it changes when the window's does.
static SHOWN: LazyLock<watch::Sender<String>> = LazyLock::new(|| watch::Sender::new("en".into()));

/// The catalog language `tag` reads in: itself or the first shorter tag of
/// it the catalog has (`zh-Hans-CN`, `zh-Hans`, `zh`), else English.
pub fn catalog_language(tag: &str) -> String {
    let have = crate::_rust_i18n_available_locales();
    let mut next = Some(tag.trim());
    while let Some(t) = next {
        if let Some(l) = have.iter().find(|l| l.eq_ignore_ascii_case(t)) {
            return l.to_string();
        }
        next = t.rfind('-').map(|i| &t[..i]);
    }
    "en".into()
}

/// Kairo's text outside the window follows `tag` (a BCP 47 tag, `zh-Hans-CN`).
pub fn set_language(tag: &str) {
    let language = catalog_language(tag);
    rust_i18n::set_locale(&language);
    SHOWN.send_if_modified(|now| {
        let changed = *now != language;
        *now = language;
        changed
    });
}

/// The catalog language Kairo's text outside the window is in.
pub fn language() -> String {
    SHOWN.borrow().clone()
}

/// Changes of [`language`] (the tray labels itself again).
pub fn watch() -> watch::Receiver<String> {
    SHOWN.subscribe()
}

/// The language a process starts in, before a window says: the one picked in
/// Settings, else the environment's (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`,
/// `LANG`), else English.
pub fn start(language: Language) {
    let tag = language
        .tag()
        .map(str::to_owned)
        .or_else(environment_language)
        .unwrap_or_else(|| "en".into());
    set_language(&tag);
}

/// The first language the POSIX environment asks for, as a tag (`zh_CN.UTF-8`
/// is `zh-CN`); none for `C` and `POSIX`.
pub fn environment_language() -> Option<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|var| std::env::var(var).ok())
        .find_map(|v| posix_tag(v.split(':').next().unwrap_or_default()))
}

fn posix_tag(value: &str) -> Option<String> {
    let tag = value.split(['.', '@']).next().unwrap_or_default().trim();
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        return None;
    }
    Some(tag.replace('_', "-"))
}

/// The text of `key` in [`language`], `args` in its `%{name}`s: what
/// [`tr!`](crate::tr) calls.
pub fn translate(key: &str, args: &[(&str, &dyn Display)]) -> String {
    text_in(&rust_i18n::locale(), key, args)
}

/// The text of `key` in the catalog language `locale`, `args` in its
/// `%{name}`s; `<locale>.<key>` where no language has it.
pub fn text_in(locale: &str, key: &str, args: &[(&str, &dyn Display)]) -> String {
    let text = crate::_rust_i18n_translate(locale, key);
    if args.is_empty() {
        return text.into_owned();
    }
    let names: Vec<&str> = args.iter().map(|(n, _)| *n).collect();
    let values: Vec<String> = args.iter().map(|(_, v)| v.to_string()).collect();
    rust_i18n::replace_patterns(&text, &names, &values)
}

/// Every text by language and key: what the window's root is given
/// (`Root::translations`), so NSH's `t!` reads the same catalog.
pub fn catalog() -> HashMap<String, HashMap<String, String>> {
    let backend = crate::_rust_i18n_backend();
    crate::_rust_i18n_available_locales()
        .into_iter()
        .map(|locale| {
            let texts = backend
                .messages_for_locale(&locale)
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            (locale.into_owned(), texts)
        })
        .collect()
}

/// Kairo's text of a key in the language outside the window, as `t!` is in
/// it: `tr!("tray.quit")`, `tr!("notice.saved", name = file)` (`%{name}`).
#[macro_export]
macro_rules! tr {
    ($key:expr $(,)?) => {
        $crate::i18n::translate($key, &[])
    };
    ($key:expr $(, $name:ident = $value:expr)+ $(,)?) => {
        $crate::i18n::translate(
            $key,
            &[$((stringify!($name), &$value as &dyn ::core::fmt::Display)),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_read_in_the_catalog_language_they_fall_to() {
        assert_eq!(catalog_language("zh-Hans-CN"), "zh");
        assert_eq!(catalog_language("zh-CN"), "zh");
        assert_eq!(catalog_language("zh"), "zh");
        assert_eq!(catalog_language("en-GB"), "en");
        assert_eq!(catalog_language("de-CH"), "en");
        assert_eq!(catalog_language(""), "en");
    }

    #[test]
    fn posix_values_are_tags() {
        assert_eq!(posix_tag("zh_CN.UTF-8").as_deref(), Some("zh-CN"));
        assert_eq!(posix_tag("en_US@euro").as_deref(), Some("en-US"));
        assert_eq!(posix_tag("C"), None);
        assert_eq!(posix_tag("POSIX"), None);
        assert_eq!(posix_tag(""), None);
    }

    #[test]
    fn both_languages_have_the_language_names() {
        assert_eq!(text_in("en", "settings.language.system", &[]), "System");
        assert_eq!(text_in("zh", "settings.language.system", &[]), "跟随系统");
        assert_eq!(text_in("zh-Hans", "settings.language.title", &[]), "语言");
    }
}
