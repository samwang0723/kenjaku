use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported locales for the search engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
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
}

impl Default for Locale {
    fn default() -> Self {
        Locale::En
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
}
