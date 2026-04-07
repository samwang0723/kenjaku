//! Query quality guard and normalization helpers used before recording a
//! search into the trending/crowdsourcing pipeline. The goal is to keep
//! junk ("sftchhfsxbnjgfd", "aaaaaaa", 500-char pastes) out of Redis
//! entirely so it can never surface in autocomplete or top-searches.

use kenjaku_core::types::locale::Locale;

/// Max length for Latin-script queries (chars). Legitimate queries rarely
/// exceed ~100 chars; anything longer is almost certainly a paste or attack.
const MAX_LATIN_CHARS: usize = 120;

/// Max length for CJK queries (runes). CJK is information-dense, so the
/// cap is tighter than Latin.
const MAX_CJK_RUNES: usize = 60;

/// If a Latin query this long has no spaces at all, it is almost
/// certainly keyboard mashing, not a real multi-word query.
const MAX_LATIN_NO_SPACE: usize = 25;

/// For queries of this length or more, any single character appearing
/// with this frequency (as a ratio of total chars) is treated as
/// repetitive gibberish ("ccccccc", "aaaabbbb").
const REPETITION_MIN_LEN: usize = 10;
const REPETITION_MAX_RATIO: f32 = 0.4;

/// Returns `true` if the query looks like gibberish and should be
/// rejected from the trending/crowdsourcing pipeline.
///
/// Rules (apply after trimming; empty is rejected):
///
/// 1. Length caps (Latin: 120 chars, CJK: 60 runes).
/// 2. Latin queries ≥ 25 chars with no spaces → rejected.
/// 3. Any single char with > 40% frequency in queries ≥ 10 chars → rejected.
pub fn is_gibberish(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }

    let char_count = trimmed.chars().count();
    let is_cjk = contains_cjk(trimmed);

    // Rule 1: length caps
    if is_cjk {
        if char_count > MAX_CJK_RUNES {
            return true;
        }
    } else if char_count > MAX_LATIN_CHARS {
        return true;
    }

    // Rule 2: Latin queries without word boundaries
    if !is_cjk && char_count >= MAX_LATIN_NO_SPACE && !trimmed.contains(char::is_whitespace) {
        return true;
    }

    // Rule 3: single-character dominance
    if char_count >= REPETITION_MIN_LEN {
        let max_freq = max_char_frequency(trimmed);
        let ratio = max_freq as f32 / char_count as f32;
        if ratio > REPETITION_MAX_RATIO {
            return true;
        }
    }

    false
}

/// Returns `true` if the query contains at least one character in a
/// common CJK Unicode block (Hiragana, Katakana, CJK Unified Ideographs,
/// Hangul).
fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x3040..=0x309F // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0x3400..=0x4DBF // CJK Extension A
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        )
    })
}

/// Returns the frequency of the most common character (case-insensitive
/// for ASCII) in the string, ignoring whitespace.
fn max_char_frequency(s: &str) -> usize {
    use std::collections::HashMap;
    let mut counts: HashMap<char, usize> = HashMap::new();
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        let key = c.to_ascii_lowercase();
        *counts.entry(key).or_insert(0) += 1;
    }
    counts.into_values().max().unwrap_or(0)
}

/// Canonicalize a query before storing it in the trending pipeline.
///
/// - `Locale::En`: use the translator's normalized form (typo-fixed,
///   canonicalized terminology). Falls back to the raw query if the
///   normalized form is empty.
/// - Other locales: the translator auto-detects the source language and
///   outputs English, so we cannot use its result for display. Instead,
///   take the raw query and capitalize its first character so
///   "bitcoin" and "Bitcoin" don't count as two different trends.
pub fn normalize_for_trending(locale: Locale, raw: &str, normalized: &str) -> String {
    if locale == Locale::En {
        let n = normalized.trim();
        if !n.is_empty() {
            return n.to_string();
        }
        return raw.trim().to_string();
    }
    capitalize_first(raw.trim())
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_queries() {
        assert!(!is_gibberish("bitcoin price today"));
        assert!(!is_gibberish("how to stake ETH"));
        assert!(!is_gibberish("比特幣 價格"));
        assert!(!is_gibberish("暗号資産"));
    }

    #[test]
    fn rejects_empty() {
        assert!(is_gibberish(""));
        assert!(is_gibberish("   "));
    }

    #[test]
    fn rejects_overlong_latin() {
        let long = "a ".repeat(200);
        assert!(is_gibberish(&long));
    }

    #[test]
    fn rejects_overlong_cjk() {
        let long: String = "比".repeat(80);
        assert!(is_gibberish(&long));
    }

    #[test]
    fn rejects_no_space_latin() {
        assert!(is_gibberish("sftchhfsxbnjgfdqwertyuiop"));
        assert!(!is_gibberish("howdoistakeeth")); // 14 chars, under threshold
    }

    #[test]
    fn rejects_repetitive() {
        assert!(is_gibberish("ccccccccccccc"));
        assert!(is_gibberish("aaaa bbbb aaaa"));
    }

    #[test]
    fn english_uses_normalized() {
        let out = normalize_for_trending(Locale::En, "bitcon prce", "bitcoin price");
        assert_eq!(out, "bitcoin price");
    }

    #[test]
    fn english_falls_back_to_raw_when_normalized_empty() {
        let out = normalize_for_trending(Locale::En, "bitcoin", "");
        assert_eq!(out, "bitcoin");
    }

    #[test]
    fn non_english_capitalizes_first() {
        let out = normalize_for_trending(Locale::ZhTw, "比特幣 價格", "bitcoin price");
        assert_eq!(out, "比特幣 價格");
        let out = normalize_for_trending(Locale::De, "bitcoin preis", "bitcoin price");
        assert_eq!(out, "Bitcoin preis");
    }
}
