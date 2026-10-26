//! Interface localization (Waku's `src/i18n.rs` ported and widened).
//!
//! Translations live in `crates/orbit-pi/locales/<locale>.yml` and are
//! compiled in by the `rust_i18n::i18n!` invocation in `main.rs`. Lookups go
//! through the [`tr!`](crate::tr) / [`tr_cow!`](crate::tr_cow) macros so a
//! missing key falls back to English and, ultimately, to the key itself.
//!
//! The persisted preference is [`AppLanguage`] — either an explicit locale or
//! `System`, which resolves through the OS's preferred languages.

use serde::{Deserialize, Serialize};

/// The language preference Orbit persists. `System` resolves to one of the
/// locales Orbit deliberately ships today.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum AppLanguage {
    #[default]
    System,
    English,
    SimplifiedChinese,
    Japanese,
    Korean,
    Spanish,
    French,
    German,
    PortugueseBrazil,
    Russian,
    Italian,
}

impl AppLanguage {
    /// Every choice offered in Settings → Appearance, `System` first.
    pub const ALL: [Self; 11] = [
        Self::System,
        Self::English,
        Self::SimplifiedChinese,
        Self::Japanese,
        Self::Korean,
        Self::Spanish,
        Self::French,
        Self::German,
        Self::PortugueseBrazil,
        Self::Russian,
        Self::Italian,
    ];

    /// Explicit languages only (no `System`), in picker order. Used by the
    /// completeness tests and by callers that enumerate shipped locales.
    #[allow(dead_code)]
    pub const EXPLICIT: [Self; 10] = [
        Self::English,
        Self::SimplifiedChinese,
        Self::Japanese,
        Self::Korean,
        Self::Spanish,
        Self::French,
        Self::German,
        Self::PortugueseBrazil,
        Self::Russian,
        Self::Italian,
    ];

    /// The rust-i18n locale id this preference resolves to.
    pub fn locale(self) -> &'static str {
        match self.resolved() {
            Self::System => unreachable!("system language always resolves to a shipped locale"),
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::PortugueseBrazil => "pt-BR",
            Self::Russian => "ru",
            Self::Italian => "it",
        }
    }

    /// Explicit language names are autonyms so the selector remains
    /// understandable even when the current locale is unfamiliar. `System`
    /// is the one translated label — it always reads in the active locale.
    pub fn label(self) -> String {
        match self {
            Self::System => translate("language.system"),
            Self::English => "English".to_owned(),
            Self::SimplifiedChinese => "简体中文".to_owned(),
            Self::Japanese => "日本語".to_owned(),
            Self::Korean => "한국어".to_owned(),
            Self::Spanish => "Español".to_owned(),
            Self::French => "Français".to_owned(),
            Self::German => "Deutsch".to_owned(),
            Self::PortugueseBrazil => "Português (Brasil)".to_owned(),
            Self::Russian => "Русский".to_owned(),
            Self::Italian => "Italiano".to_owned(),
        }
    }

    /// The persisted token (also the selector's stable identity).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::PortugueseBrazil => "pt-BR",
            Self::Russian => "ru",
            Self::Italian => "it",
        }
    }

    /// Parse a persisted token. Accepts the historical `en` plus the
    /// rust-i18n locale ids; anything unknown is left for the caller to treat
    /// as a default.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().replace('_', "-").to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "en" | "en-us" | "en-gb" => Some(Self::English),
            "zh-cn" | "zh-sg" | "zh-hans" => Some(Self::SimplifiedChinese),
            "ja" | "ja-jp" => Some(Self::Japanese),
            "ko" | "ko-kr" => Some(Self::Korean),
            "es" | "es-es" | "es-mx" | "es-419" => Some(Self::Spanish),
            "fr" | "fr-fr" | "fr-ca" => Some(Self::French),
            "de" | "de-de" => Some(Self::German),
            "pt-br" | "pt" => Some(Self::PortugueseBrazil),
            "ru" | "ru-ru" => Some(Self::Russian),
            "it" | "it-it" => Some(Self::Italian),
            _ => None,
        }
    }

    /// Collapse `System` onto the concrete locale the OS asks for.
    pub fn resolved(self) -> Self {
        match self {
            Self::System => Self::from_locale_id(&system_locale()),
            explicit => explicit,
        }
    }

    /// Map an arbitrary BCP-47 tag onto a shipped locale, or `None` when
    /// Orbit does not ship that language. Used by [`Self::resolved`] to walk
    /// the OS's ordered preference list past unshipped languages. Only
    /// Simplified Chinese is enabled — Traditional tags return `None` rather
    /// than reading as the wrong script.
    pub fn shipped_locale_id(locale: &str) -> Option<Self> {
        let locale = locale.replace('_', "-").to_ascii_lowercase();
        if locale == "en" || locale.starts_with("en-") {
            Some(Self::English)
        } else if locale == "zh-cn" || locale == "zh-sg" || locale.starts_with("zh-hans") {
            Some(Self::SimplifiedChinese)
        } else if locale == "ja" || locale.starts_with("ja-") {
            Some(Self::Japanese)
        } else if locale == "ko" || locale.starts_with("ko-") {
            Some(Self::Korean)
        } else if locale == "es" || locale.starts_with("es-") {
            Some(Self::Spanish)
        } else if locale == "fr" || locale.starts_with("fr-") {
            Some(Self::French)
        } else if locale == "de" || locale.starts_with("de-") {
            Some(Self::German)
        } else if locale == "pt-br" || locale == "pt" {
            Some(Self::PortugueseBrazil)
        } else if locale == "ru" || locale.starts_with("ru-") {
            Some(Self::Russian)
        } else if locale == "it" || locale.starts_with("it-") {
            Some(Self::Italian)
        } else {
            None
        }
    }

    /// Map an arbitrary BCP-47 tag onto a shipped locale, defaulting to
    /// English. Only Simplified Chinese is enabled — Traditional tags stay
    /// English rather than reading as the wrong script.
    pub fn from_locale_id(locale: &str) -> Self {
        Self::shipped_locale_id(locale).unwrap_or(Self::English)
    }
}

/// Point the global lookup at a preference's concrete locale.
pub fn set_language(language: AppLanguage) {
    rust_i18n::set_locale(language.locale());
}

/// Translate a key in the active locale, returning an owned `String`.
///
/// Prefer the [`tr!`](crate::tr) macro at call sites; this exists so
/// non-literal keys (e.g. from a table) can still be looked up.
pub fn translate(key: &str) -> String {
    rust_i18n::t!(key).into_owned()
}

/// Translate a key in an explicit locale rather than the active one. Used by
/// tests (and any caller that must resolve a string for a locale other than
/// the one currently painted).
#[allow(dead_code)]
pub fn translate_in(locale: &str, key: &str) -> String {
    rust_i18n::t!(key, locale = locale).into_owned()
}

/// Whether the active locale reads dates in an East-Asian order
/// (`2026年2月3日`), which some call sites format by hand. Reserved for the
/// date formatters; unused until those read it.
#[allow(dead_code)]
pub fn uses_east_asian_date_format() -> bool {
    locale_uses_east_asian_date_format(&rust_i18n::locale())
}

#[allow(dead_code)]
fn locale_uses_east_asian_date_format(locale: &str) -> bool {
    matches!(locale, "zh-CN" | "ja" | "ko")
}

/// The OS's preferred UI language as a BCP-47 tag.
#[cfg(target_os = "macos")]
fn system_locale() -> String {
    use objc2_foundation::NSLocale;

    // Walk the user's ordered preference list and take the first language
    // Orbit ships. The list is ordered, so a user whose first choice is
    // unshipped (say Hindi) still gets their second (say Chinese) instead of
    // falling straight to English. Only when the list is empty do we fall
    // back to the user's current locale.
    let preferred = NSLocale::preferredLanguages();
    first_shipped(preferred.iter().map(|locale| locale.to_string()))
        .unwrap_or_else(|| NSLocale::currentLocale().localeIdentifier().to_string())
}

/// Pick the first language in an ordered preference list that Orbit ships,
/// falling back to the first non-empty entry (which resolves to English via
/// [`AppLanguage::from_locale_id`]) when none ships. `None` only for an empty
/// list.
#[cfg(target_os = "macos")]
fn first_shipped(preferred: impl IntoIterator<Item = String>) -> Option<String> {
    let mut top: Option<String> = None;
    for tag in preferred {
        if tag.is_empty() {
            continue;
        }
        if AppLanguage::shipped_locale_id(&tag).is_some() {
            return Some(tag);
        }
        top.get_or_insert(tag);
    }
    top
}

#[cfg(not(target_os = "macos"))]
fn system_locale() -> String {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en".to_owned());
    let tag = raw.split('.').next().unwrap_or("en");
    // `LANG` is usually `ll_CC`, which `shipped_locale_id` normalizes; an
    // unshipped value still resolves to English through `from_locale_id`.
    AppLanguage::shipped_locale_id(tag)
        .map(|language| language.locale().to_owned())
        .unwrap_or_else(|| tag.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_locale_ids_are_supported() {
        assert_eq!(AppLanguage::English.locale(), "en");
        assert_eq!(AppLanguage::SimplifiedChinese.locale(), "zh-CN");
        assert_eq!(AppLanguage::Japanese.locale(), "ja");
        assert_eq!(AppLanguage::Korean.locale(), "ko");
        assert_eq!(AppLanguage::Spanish.locale(), "es");
        assert_eq!(AppLanguage::French.locale(), "fr");
        assert_eq!(AppLanguage::German.locale(), "de");
        assert_eq!(AppLanguage::PortugueseBrazil.locale(), "pt-BR");
        assert_eq!(AppLanguage::Russian.locale(), "ru");
        assert_eq!(AppLanguage::Italian.locale(), "it");

        let locales = rust_i18n::available_locales!();
        assert_eq!(locales.len(), 10);
        for language in AppLanguage::EXPLICIT {
            assert!(
                locales
                    .iter()
                    .any(|locale| locale.as_ref() == language.locale()),
                "locale file missing for {}",
                language.locale()
            );
        }
    }

    #[test]
    fn language_names_are_autonyms() {
        assert_eq!(AppLanguage::English.label(), "English");
        assert_eq!(AppLanguage::SimplifiedChinese.label(), "简体中文");
        assert_eq!(AppLanguage::Japanese.label(), "日本語");
        assert_eq!(AppLanguage::Korean.label(), "한국어");
        assert_eq!(AppLanguage::German.label(), "Deutsch");
    }

    #[test]
    fn system_is_the_default_persisted_preference() {
        assert_eq!(AppLanguage::default(), AppLanguage::System);
        assert_eq!(
            serde_json::to_string(&AppLanguage::System).unwrap(),
            r#""system""#
        );
        assert!(matches!(
            AppLanguage::System.locale(),
            "en" | "zh-CN" | "ja" | "ko" | "es" | "fr" | "de" | "pt-BR" | "ru" | "it"
        ));
    }

    #[test]
    fn persisted_tokens_round_trip() {
        for language in AppLanguage::ALL {
            assert_eq!(AppLanguage::parse(language.as_str()), Some(language));
        }
        // Legacy / regional tags fold onto a shipped locale.
        assert_eq!(
            AppLanguage::parse("zh_CN"),
            Some(AppLanguage::SimplifiedChinese)
        );
        assert_eq!(
            AppLanguage::parse("pt"),
            Some(AppLanguage::PortugueseBrazil)
        );
        assert_eq!(AppLanguage::parse("fr-CA"), Some(AppLanguage::French));
    }

    #[test]
    fn system_locales_are_detected() {
        assert_eq!(AppLanguage::from_locale_id("ja_JP"), AppLanguage::Japanese);
        assert_eq!(AppLanguage::from_locale_id("ko-KR"), AppLanguage::Korean);
        assert_eq!(AppLanguage::from_locale_id("de-DE"), AppLanguage::German);
        assert_eq!(
            AppLanguage::from_locale_id("zh-Hans-CN"),
            AppLanguage::SimplifiedChinese
        );
        // Traditional Chinese is deliberately not enabled.
        assert_eq!(
            AppLanguage::from_locale_id("zh-Hant-TW"),
            AppLanguage::English
        );
        assert_eq!(AppLanguage::from_locale_id("nl-NL"), AppLanguage::English);
    }

    #[test]
    fn shipped_locale_id_only_matches_shipped_languages() {
        assert_eq!(
            AppLanguage::shipped_locale_id("en-IN"),
            Some(AppLanguage::English)
        );
        assert_eq!(
            AppLanguage::shipped_locale_id("pt-PT"),
            None,
            "only Brazilian Portuguese is shipped"
        );
        assert_eq!(AppLanguage::shipped_locale_id("hi-IN"), None);
        assert_eq!(AppLanguage::shipped_locale_id("zh-Hant-TW"), None);
    }

    /// A user whose top language Orbit does not ship should still get their
    /// next shipped preference instead of English.
    #[cfg(target_os = "macos")]
    #[test]
    fn system_preference_list_skips_unshipped_languages() {
        assert_eq!(
            first_shipped(["hi-IN".to_owned(), "zh-Hans-CN".to_owned()]),
            Some("zh-Hans-CN".to_owned())
        );
        assert_eq!(
            first_shipped(["nl-NL".to_owned(), "de-DE".to_owned()]),
            Some("de-DE".to_owned())
        );
        // English is shipped, so it wins when it is the first preference.
        assert_eq!(
            first_shipped(["en-US".to_owned(), "ja-JP".to_owned()]),
            Some("en-US".to_owned())
        );
        // Nothing ships: keep the top tag (it will resolve to English).
        assert_eq!(
            first_shipped(["hi-IN".to_owned()]),
            Some("hi-IN".to_owned())
        );
        assert_eq!(first_shipped(std::iter::empty()), None);
    }

    /// The resolved `System` default must always land on a shipped locale —
    /// this is what the app paints on a fresh install with no `ui.json`.
    #[test]
    fn system_default_resolves_to_a_shipped_locale() {
        assert!(AppLanguage::EXPLICIT.contains(&AppLanguage::System.resolved()));
    }

    #[test]
    fn east_asian_locales_use_east_asian_dates() {
        assert!(locale_uses_east_asian_date_format("ja"));
        assert!(locale_uses_east_asian_date_format("ko"));
        assert!(locale_uses_east_asian_date_format("zh-CN"));
        assert!(!locale_uses_east_asian_date_format("en"));
    }

    #[test]
    fn core_keys_resolve_in_every_locale() {
        // A canary key that must exist in every shipped locale file.
        for language in AppLanguage::EXPLICIT {
            let value = rust_i18n::t!("language.title", locale = language.locale());
            assert!(
                !value.is_empty(),
                "missing language.title for {}",
                language.locale()
            );
        }
    }

    /// Regression guard: keys wrapped before `en.yml` existed once rendered
    /// the raw key (the sidebar's “Settings” stayed English). They must now
    /// resolve to a real, non-English translation.
    #[test]
    fn previously_unregistered_sidebar_keys_translate() {
        for key in [
            "common.settings",
            "sidebar.projects",
            "status.connected",
            "status.offline",
            "composer.drop_to_attach",
        ] {
            let en = rust_i18n::t!(key, locale = "en");
            let zh = rust_i18n::t!(key, locale = "zh-CN");
            assert_ne!(en.as_ref(), key, "{key} is not defined in en.yml");
            assert_ne!(zh.as_ref(), key, "{key} is not defined in zh-CN.yml");
            assert_ne!(zh, en, "{key} still renders English in zh-CN");
        }
    }

    /// Regression guard: keys added during the localization sweep must
    /// resolve in the shipped locales instead of rendering the raw key (the
    /// usage column picker once showed `usage.col_share`).
    #[test]
    fn newly_localized_ui_keys_translate() {
        for key in [
            "usage.col_calls",
            "usage.col_avg",
            "usage.col_max",
            "usage.col_share",
            "usage.all",
            "context_meter.in_use",
            "context_meter.total",
            "access.supervised",
            "command_palette.settings",
            "model_selector.all",
            "model_selector.favorites",
            "model_selector.reasoning_balanced",
            "onboarding.platform",
            "git.pushed",
        ] {
            let en = rust_i18n::t!(key, locale = "en");
            let zh = rust_i18n::t!(key, locale = "zh-CN");
            assert_ne!(en.as_ref(), key, "{key} is not defined in en.yml");
            assert_ne!(zh.as_ref(), key, "{key} is not defined in zh-CN.yml");
            assert_ne!(zh, en, "{key} still renders English in zh-CN");
        }
    }

    /// Every key in `en.yml` must exist in every generated locale file. The
    /// generator (`scripts/gen_locales.py`) guarantees this, but a manual edit
    /// or a stale file would silently fall back to English — this catches it.
    #[test]
    fn locale_files_cover_every_english_key() {
        fn keys(src: &str) -> std::collections::BTreeSet<&str> {
            src.lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with("_version"))
                .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim()))
                .collect()
        }
        let en = keys(include_str!("../locales/en.yml"));
        for (locale, source) in [
            ("zh-CN", include_str!("../locales/zh-CN.yml")),
            ("ja", include_str!("../locales/ja.yml")),
            ("ko", include_str!("../locales/ko.yml")),
            ("es", include_str!("../locales/es.yml")),
            ("fr", include_str!("../locales/fr.yml")),
            ("de", include_str!("../locales/de.yml")),
            ("pt-BR", include_str!("../locales/pt-BR.yml")),
            ("ru", include_str!("../locales/ru.yml")),
            ("it", include_str!("../locales/it.yml")),
        ] {
            let have = keys(source);
            let missing: Vec<_> = en.difference(&have).collect();
            assert!(
                missing.is_empty(),
                "{locale} is missing {} key(s), e.g. {:?}",
                missing.len(),
                &missing[..missing.len().min(5)]
            );
        }
    }
}
