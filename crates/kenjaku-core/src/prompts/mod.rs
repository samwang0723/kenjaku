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
/// Applied in order — earlier substitutions can produce text that
/// later substitutions replace (useful when one placeholder's value
/// itself contains further placeholders, as with `{{source_rules}}`
/// being expanded first then `{{locale_display}}` being expanded
/// inside the just-substituted block).
///
/// Keys are wrapped in `{{...}}` automatically — pass `"query"`, not
/// `"{{query}}"`.
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
    let mut out = template.to_string();
    for (key, value) in pairs {
        let placeholder = format!("{{{{{key}}}}}");
        out = out.replace(&placeholder, value);
    }
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
    fn render_applies_in_order_so_nested_placeholders_work() {
        // `{{outer}}` expands to `{{inner}}`, then `{{inner}}` expands.
        // Proves the system-instruction use case: {{source_rules}}
        // gets substituted with a block containing no locale tags,
        // then {{locale_display}} is filled — but if the source-rules
        // text itself contained `{{locale_display}}`, a second-pass
        // replace would hit it.
        let out = render(
            "X={{outer}} Y={{inner}}",
            &[("outer", "{{inner}}"), ("inner", "filled")],
        );
        assert_eq!(out, "X=filled Y=filled");
    }

    #[test]
    fn render_leaves_unknown_placeholders_untouched() {
        let out = render("hello {{name}} {{age}}", &[("name", "sam")]);
        assert_eq!(out, "hello sam {{age}}");
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
