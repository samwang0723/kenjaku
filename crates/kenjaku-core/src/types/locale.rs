use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported locales for the search engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "zh-TW")]
    ZhTw,
    #[serde(rename = "ja")]
    Ja,
    #[serde(rename = "ko")]
    Ko,
    #[serde(rename = "de")]
    De,
    #[serde(rename = "fr")]
    Fr,
    #[serde(rename = "es")]
    Es,
}

impl Locale {
    /// All supported locales.
    pub const ALL: &'static [Locale] = &[
        Locale::En,
        Locale::Zh,
        Locale::ZhTw,
        Locale::Ja,
        Locale::Ko,
        Locale::De,
        Locale::Fr,
        Locale::Es,
    ];

    /// Whether this locale requires translation to English for retrieval.
    pub fn needs_translation(self) -> bool {
        self != Locale::En
    }

    /// Returns the BCP-47 language tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Zh => "zh",
            Locale::ZhTw => "zh-TW",
            Locale::Ja => "ja",
            Locale::Ko => "ko",
            Locale::De => "de",
            Locale::Fr => "fr",
            Locale::Es => "es",
        }
    }

    /// Native-script display name suitable for embedding in an LLM
    /// prompt that asks the model to respond in this language.
    pub fn display_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Zh => "简体中文",
            Locale::ZhTw => "繁體中文",
            Locale::Ja => "日本語",
            Locale::Ko => "한국어",
            Locale::De => "Deutsch",
            Locale::Fr => "Français",
            Locale::Es => "Español",
        }
    }
}

/// Outcome of LLM-driven locale detection. The translator returns one of
/// these for every query — either a `Locale` we already support, or an
/// arbitrary BCP-47 tag we should fall back from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedLocale {
    /// One of the 8 supported `Locale` enum variants.
    Supported(Locale),
    /// A valid BCP-47 tag we don't support yet (e.g. `pt`, `it`, `ru`).
    /// SearchService falls back to English and emits a metric so we can
    /// prioritize which tag to add to the enum next.
    Unsupported { tag: String },
}

impl DetectedLocale {
    /// Lenient BCP-47 parser. Maps common tag variants to the `Locale`
    /// enum (`zh-tw`, `zh_TW`, `zh-Hant` → `ZhTw`); returns
    /// `Unsupported { tag }` for any tag outside the 8 supported locales.
    pub fn from_bcp47(tag: &str) -> Self {
        let trimmed = tag.trim();
        let normalized = trimmed.to_ascii_lowercase();
        let mapped: Option<Locale> = match normalized.as_str() {
            "en" | "en-us" | "en-gb" => Some(Locale::En),
            // Traditional Chinese variants — match before generic `zh`
            // so `zh-Hant` and `zh-TW` route to ZhTw.
            "zh-tw" | "zh_tw" | "zh-hant" | "zh-hant-tw" => Some(Locale::ZhTw),
            "zh" | "zh-cn" | "zh-hans" => Some(Locale::Zh),
            "ja" | "ja-jp" => Some(Locale::Ja),
            "ko" | "ko-kr" => Some(Locale::Ko),
            "de" | "de-de" => Some(Locale::De),
            "fr" | "fr-fr" => Some(Locale::Fr),
            "es" | "es-es" | "es-mx" => Some(Locale::Es),
            _ => None,
        };
        match mapped {
            Some(l) => DetectedLocale::Supported(l),
            None => DetectedLocale::Unsupported {
                tag: trimmed.to_string(),
            },
        }
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Locale {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "en" => Ok(Locale::En),
            "zh" => Ok(Locale::Zh),
            "zh-TW" | "zh_TW" | "zh-tw" => Ok(Locale::ZhTw),
            "ja" => Ok(Locale::Ja),
            "ko" => Ok(Locale::Ko),
            "de" => Ok(Locale::De),
            "fr" => Ok(Locale::Fr),
            "es" => Ok(Locale::Es),
            _ => Err(crate::error::Error::Validation(format!(
                "Unsupported locale: '{s}'. Supported: en, zh, zh-TW, ja, ko, de, fr, es"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_from_str_valid() {
        assert_eq!("en".parse::<Locale>().unwrap(), Locale::En);
        assert_eq!("zh".parse::<Locale>().unwrap(), Locale::Zh);
        assert_eq!("zh-TW".parse::<Locale>().unwrap(), Locale::ZhTw);
        assert_eq!("zh_TW".parse::<Locale>().unwrap(), Locale::ZhTw);
        assert_eq!("ja".parse::<Locale>().unwrap(), Locale::Ja);
        assert_eq!("ko".parse::<Locale>().unwrap(), Locale::Ko);
        assert_eq!("de".parse::<Locale>().unwrap(), Locale::De);
        assert_eq!("fr".parse::<Locale>().unwrap(), Locale::Fr);
        assert_eq!("es".parse::<Locale>().unwrap(), Locale::Es);
    }

    #[test]
    fn test_locale_from_str_invalid() {
        assert!("pt".parse::<Locale>().is_err());
        assert!("".parse::<Locale>().is_err());
        assert!("EN".parse::<Locale>().is_err());
    }

    #[test]
    fn test_locale_display() {
        assert_eq!(Locale::En.to_string(), "en");
        assert_eq!(Locale::ZhTw.to_string(), "zh-TW");
        assert_eq!(Locale::Ja.to_string(), "ja");
    }

    #[test]
    fn test_locale_needs_translation() {
        assert!(!Locale::En.needs_translation());
        assert!(Locale::Zh.needs_translation());
        assert!(Locale::ZhTw.needs_translation());
        assert!(Locale::Ja.needs_translation());
        assert!(Locale::Ko.needs_translation());
        assert!(Locale::De.needs_translation());
        assert!(Locale::Fr.needs_translation());
        assert!(Locale::Es.needs_translation());
    }

    #[test]
    fn test_locale_serde_roundtrip() {
        let locale = Locale::ZhTw;
        let json = serde_json::to_string(&locale).unwrap();
        assert_eq!(json, "\"zh-TW\"");
        let parsed: Locale = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Locale::ZhTw);
    }

    #[test]
    fn test_all_locales_count() {
        assert_eq!(Locale::ALL.len(), 8);
    }

    #[test]
    fn test_display_name() {
        assert_eq!(Locale::En.display_name(), "English");
        assert_eq!(Locale::ZhTw.display_name(), "繁體中文");
        assert_eq!(Locale::Ja.display_name(), "日本語");
    }

    #[test]
    fn test_detected_locale_supported() {
        assert_eq!(
            DetectedLocale::from_bcp47("en"),
            DetectedLocale::Supported(Locale::En)
        );
        assert_eq!(
            DetectedLocale::from_bcp47("zh-TW"),
            DetectedLocale::Supported(Locale::ZhTw)
        );
        assert_eq!(
            DetectedLocale::from_bcp47("zh_tw"),
            DetectedLocale::Supported(Locale::ZhTw)
        );
        assert_eq!(
            DetectedLocale::from_bcp47("zh-Hant"),
            DetectedLocale::Supported(Locale::ZhTw)
        );
        assert_eq!(
            DetectedLocale::from_bcp47("zh-Hans"),
            DetectedLocale::Supported(Locale::Zh)
        );
        assert_eq!(
            DetectedLocale::from_bcp47("ja-JP"),
            DetectedLocale::Supported(Locale::Ja)
        );
    }

    #[test]
    fn test_detected_locale_unsupported() {
        match DetectedLocale::from_bcp47("pt") {
            DetectedLocale::Unsupported { tag } => assert_eq!(tag, "pt"),
            _ => panic!("expected Unsupported"),
        }
        match DetectedLocale::from_bcp47("it") {
            DetectedLocale::Unsupported { tag } => assert_eq!(tag, "it"),
            _ => panic!("expected Unsupported"),
        }
    }
}
