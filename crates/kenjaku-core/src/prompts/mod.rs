//! LLM prompt templates — externalized to `.md` files so they can be
//! edited and reviewed as prose.
//!
//! Each template is a markdown file in this directory; constants below
//! bake the contents into the binary via [`include_str!`] at compile
//! time. Zero runtime I/O, zero deployment-path concerns — but prompts
//! still travel through code review as readable diffs instead of
//! Rust-escaped multi-line strings.
//!
//! Template substitution uses simple `{{placeholder}}` syntax; call
//! sites use [`str::replace`] to fill values. See [`render`] for a
//! small helper that applies a list of `(key, value)` pairs in order.
//!
//! # Adding a new prompt
//!
//! 1. Drop `my_prompt.md` into this directory.
//! 2. Export it here: `pub const MY_PROMPT: &str = include_str!("my_prompt.md");`
//! 3. Document what `{{placeholders}}` it expects.
//! 4. Call [`render`] at the use site with the key/value pairs.
//!
//! # Why markdown
//!
//! - Prose-friendly review diffs (no `\n\` line continuations to scan past)
//! - Prompt engineers can edit without touching Rust
//! - Files can be linted, spell-checked, counted for token budget
//! - `include_str!` preserves compile-time safety — a missing file is a
//!   build error, a missing placeholder substitution shows up in tests

/// Classify a user query into one of 8 intent categories. Used by the
/// legacy (non-merged) preamble path via [`LlmProvider::generate_brief`].
///
/// Placeholders: `{{query}}`.
pub const CLASSIFY_INTENT: &str = include_str!("classify_intent.md");

/// Translate a user query to canonical English + detect source locale.
/// Used by [`LlmProvider::translate`] in legacy preamble mode.
///
/// Placeholders: `{{text}}`.
pub const TRANSLATE: &str = include_str!("translate.md");

/// Merged-preamble prompt: classify + translate + locale-detect in one
/// structured-output Gemini call. Used by
/// [`LlmProvider::preprocess_query`] when
/// `pipeline.preamble_mode = merged_preamble`.
///
/// Placeholders: `{{query}}`.
pub const PREPROCESS: &str = include_str!("preprocess.md");

/// Follow-up suggestions with cognitive diversity (vertical / horizontal /
/// temporal-or-actionable). Used by [`LlmProvider::suggest`].
///
/// Placeholders: `{{query}}`, `{{answer}}`.
pub const SUGGEST: &str = include_str!("suggest.md");

/// System instruction for the answer-generation call. Requires three
/// placeholders populated at build time by `brain::prompt::build_search_system_instruction`:
///
/// - `{{source_rules}}` — one of [`SOURCE_RULES_WITH_WEB_TOOL`] or
///   [`SOURCE_RULES_WITHOUT_WEB_TOOL`]
/// - `{{locale_display}}` — localized display name (e.g. "English", "繁體中文")
/// - `{{locale_tag}}` — BCP-47 tag (e.g. "en", "zh-TW")
pub const SYSTEM_INSTRUCTION: &str = include_str!("system_instruction.md");

/// Source-handling rules for when the provider has the `google_search`
/// grounding tool attached. Injected into [`SYSTEM_INSTRUCTION`] via
/// the `{{source_rules}}` placeholder.
pub const SOURCE_RULES_WITH_WEB_TOOL: &str = include_str!("source_rules_with_web_tool.md");

/// Source-handling rules for when no built-in web tool is attached —
/// the platform has pre-injected fresh web results as `[Source N]`
/// chunks. Injected into [`SYSTEM_INSTRUCTION`] via the
/// `{{source_rules}}` placeholder.
pub const SOURCE_RULES_WITHOUT_WEB_TOOL: &str = include_str!("source_rules_without_web_tool.md");

/// Substitute `{{key}}` placeholders in a template with their values.
///
/// **Single-pass, prompt-injection-safe.** Walks the template once
/// looking for `{{key}}` spans; substituted values are NEVER
/// re-scanned, so an untrusted value that itself contains `{{other}}`
/// cannot hijack a later substitution step. This is the defense
/// against prompt injection via user queries / LLM answers: a user
/// typing `{{answer}}` in their query is treated as literal text.
///
/// Keys are wrapped in `{{...}}` automatically — pass `"query"`, not
/// `"{{query}}"`. Whitespace inside the braces is tolerated (`{{ key }}`
/// resolves to the same as `{{key}}`).
///
/// Unknown placeholders in the template are preserved as-is (useful
/// for multi-stage rendering, e.g. the pipeline renders
/// `{{source_rules}}` first then `{{locale_display}}` in a second
/// call).
///
/// # Example
///
/// ```ignore
/// use kenjaku_core::prompts;
///
/// let rendered = prompts::render(
///     prompts::SUGGEST,
///     &[("query", "what is bitcoin"), ("answer", "Bitcoin is…")],
/// );
/// ```
pub fn render(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        match after_open.find("}}") {
            Some(end) => {
                let key = after_open[..end].trim();
                match pairs.iter().find(|(k, _)| *k == key) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        // Unknown placeholder — preserve literally so
                        // multi-stage renders can resolve it later.
                        out.push_str(&rest[start..start + end + 4]);
                    }
                }
                rest = &after_open[end + 2..];
            }
            None => {
                // Unclosed `{{` — append from the `{{` onward as
                // literal text and bail. (Prefix before `{{` was
                // already pushed above.)
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_single_placeholder() {
        let out = render("hello {{name}}", &[("name", "world")]);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn render_substitutes_multiple_placeholders() {
        let out = render(
            "q={{query}} a={{answer}}",
            &[("query", "what"), ("answer", "because")],
        );
        assert_eq!(out, "q=what a=because");
    }

    #[test]
    fn render_is_prompt_injection_safe() {
        // A user typing `{{answer}}` in their query MUST NOT cause the
        // `answer` substitution to hijack the injected text. This is
        // the core security property: substituted values are never
        // re-scanned. See the Copilot review on PR #28 for context.
        let out = render(
            "Question: {{query}}\nAnswer: {{answer}}",
            &[("query", "what is {{answer}}"), ("answer", "HIJACKED")],
        );
        // The user's literal `{{answer}}` survives verbatim; only the
        // template's own `{{answer}}` placeholder is filled.
        assert_eq!(out, "Question: what is {{answer}}\nAnswer: HIJACKED",);
    }

    #[test]
    fn render_injection_safe_across_many_pairs() {
        // Adversarial: user tries to swap every placeholder out via
        // injection. All substitutions must be scoped to the original
        // template, never to substituted content.
        let out = render(
            "A={{a}} B={{b}} C={{c}}",
            &[("a", "attack-{{b}}-{{c}}"), ("b", "B_VAL"), ("c", "C_VAL")],
        );
        // a's value is inserted literally — the {{b}} and {{c}} inside
        // it are NOT re-substituted even though (b, B_VAL) and
        // (c, C_VAL) exist in the pair list.
        assert_eq!(out, "A=attack-{{b}}-{{c}} B=B_VAL C=C_VAL");
    }

    #[test]
    fn render_leaves_unknown_placeholders_untouched() {
        // Useful for multi-stage rendering: pipeline can substitute
        // `{{source_rules}}` first, then feed the result back through
        // `render` with `{{locale_display}}` in the second pass.
        let out = render("hello {{name}} {{age}}", &[("name", "sam")]);
        assert_eq!(out, "hello sam {{age}}");
    }

    #[test]
    fn render_tolerates_whitespace_inside_braces() {
        // Tolerate `{{ key }}` for robustness — markdown linters
        // sometimes auto-format spaces inside braces.
        let out = render("{{ key }}", &[("key", "value")]);
        assert_eq!(out, "value");
    }

    #[test]
    fn render_handles_unclosed_brace_gracefully() {
        // A template with `{{` never closed should be preserved, not
        // panic. Defensive; shouldn't happen with reviewed .md files.
        let out = render("hello {{name no close", &[("name", "sam")]);
        assert_eq!(out, "hello {{name no close");
    }

    #[test]
    fn all_templates_load() {
        // Proves `include_str!` succeeded for every file — if any
        // markdown file goes missing or gets renamed, this test
        // won't even compile. That's the point.
        assert!(!CLASSIFY_INTENT.is_empty());
        assert!(!TRANSLATE.is_empty());
        assert!(!PREPROCESS.is_empty());
        assert!(!SUGGEST.is_empty());
        assert!(!SYSTEM_INSTRUCTION.is_empty());
        assert!(!SOURCE_RULES_WITH_WEB_TOOL.is_empty());
        assert!(!SOURCE_RULES_WITHOUT_WEB_TOOL.is_empty());
    }

    #[test]
    fn suggest_has_expected_placeholders() {
        assert!(SUGGEST.contains("{{query}}"));
        assert!(SUGGEST.contains("{{answer}}"));
    }

    #[test]
    fn system_instruction_has_expected_placeholders() {
        assert!(SYSTEM_INSTRUCTION.contains("{{source_rules}}"));
        assert!(SYSTEM_INSTRUCTION.contains("{{locale_display}}"));
        assert!(SYSTEM_INSTRUCTION.contains("{{locale_tag}}"));
    }

    #[test]
    fn preprocess_has_query_placeholder() {
        assert!(PREPROCESS.contains("{{query}}"));
    }

    #[test]
    fn translate_has_text_placeholder() {
        assert!(TRANSLATE.contains("{{text}}"));
    }

    #[test]
    fn classify_intent_has_query_placeholder() {
        assert!(CLASSIFY_INTENT.contains("{{query}}"));
    }
}
