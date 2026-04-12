//! Prompt text builders for the search pipeline.
//!
//! These functions produce the exact system instruction and user-turn strings
//! that the LLM sees. They are intentionally decoupled from any provider's
//! wire format — they return plain `String`s that the `ConversationAssembler`
//! wraps into `Message` values and the `LlmProvider` converts to its native
//! shape.
//!
//! **IMPORTANT**: Do NOT change the text of these templates without a staging
//! canary. Prompt wording is load-bearing — a one-word change can affect
//! refusal rates (see commit `cab2292`).

use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::search::RetrievedChunk;

/// Build the context string from retrieved chunks.
///
/// Produces the `[Source N]` block that gets embedded in the user turn.
pub fn build_context(chunks: &[RetrievedChunk]) -> String {
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            format!(
                "[Source {}] {}\n{}\n",
                i + 1,
                chunk.title,
                chunk.original_content,
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n")
}

/// Build the user-turn prompt. Includes an explicit language
/// reminder because in-context multi-turn priors in another language
/// can override the systemInstruction — we want the model to see the
/// target language inside the current user turn as well.
///
/// When retrieval returned no chunks, the `Internal context:` block is
/// omitted entirely rather than emitted empty — an empty block is a
/// negative cue that nudges the model toward refusal, whereas omitting
/// it leaves the model free to reach for `google_search`.
///
pub fn build_search_prompt(query: &str, context: &str, answer_locale: Locale) -> String {
    let display = answer_locale.display_name();
    let tag = answer_locale.as_str();
    let context_block = if context.trim().is_empty() {
        String::new()
    } else {
        format!("Internal context:\n{context}\n\n")
    };
    format!(
        "{context_block}\
         Question: {query}\n\n\
         Respond in {display} (`{tag}`). If earlier turns were in a different language, ignore that — answer this question in {display}.\n\n\
         Answer:"
    )
}

/// Build the per-request system instruction for the answer call.
/// Pins the answer language, sets source-handling rules, and keeps
/// wording generic so we don't trip Gemini preview models into
/// interpreting literal tool names as client-side function calls.
///
/// Two variants, selected by `has_builtin_web_tool`:
/// - **true** — the `google_search` grounding tool is attached to
///   this request. The model can reach for live web facts itself.
///   Prompt language encourages it to do so for real-time questions
///   and forbids the "I cannot access real-time data" refusal.
/// - **false** — no tool is attached; a separate `WebSearchProvider`
///   (Brave) has already pre-injected fresh web results as
///   `[Source N]` chunks in the user turn. Prompt language tells
///   the model those chunks ARE its real-time data and that it must
///   synthesize from them without deferring the user elsewhere.
///
/// Returns the raw text string. The caller wraps it in
/// `Message::system_text(...)` or passes it to the LLM provider's
/// wire format converter.
///
pub fn build_search_system_instruction(
    answer_locale: Locale,
    has_builtin_web_tool: bool,
) -> String {
    let display = answer_locale.display_name();
    let tag = answer_locale.as_str();
    let source_rules = if has_builtin_web_tool {
        "Your inputs, in priority order:\n\
         1. The numbered `[Source N]` entries in the current user turn. These come from the product's own document corpus. Prefer them when they answer the question, and cite with `[Source N]` markers.\n\
         2. The built-in web search capability attached to this request. For real-time questions — markets, prices, news, weather, sports scores, live events, anything mentioning \"today\", \"now\", \"current\", \"latest\", \"this week\" — you MUST use the web search to retrieve fresh facts, then synthesize a direct answer. Web sources surface separately in the response; do not invent `[Source N]` markers for them.\n\
         3. Your own training knowledge, used only as a last-resort fallback for timeless factual questions when both above are insufficient. Only this case may disclose a training cut-off.\n\
         \n\
         How to answer:\n\
         - NEVER respond with \"I cannot access real-time information\", \"as an AI I don't have live data\", \"check Reuters / Yahoo / CNN / Bloomberg / etc.\", or any variant that tells the user to go look it up themselves. You have a web search tool — use it. Refusals are forbidden for real-time questions.\n\
         - Include concrete numbers, dates, names, and timestamps when the retrieved sources carry them.\n\
         - Do not refuse because internal retrieval is sparse. Reach for web search instead."
    } else {
        "Your only inputs are:\n\
         1. The numbered `[Source N]` entries in the current user turn. These are authoritative. They may include product documentation, knowledge-base articles, and/or fresh web results that the platform has pre-fetched for you — you do NOT need to distinguish between them. Treat every `[Source N]` as trustworthy context supplied by the platform for this specific question.\n\
         2. Your own training knowledge, used only as a fallback when the `[Source N]` entries do not cover the question.\n\
         \n\
         How to answer:\n\
         - If `[Source N]` entries are present, synthesize a direct answer from them and cite with `[Source N]` markers. Include concrete numbers, dates, names, and timestamps when the sources carry them.\n\
         - If no `[Source N]` entries are present, answer from your training knowledge. Only in this case may you briefly note a training cut-off.\n\
         - NEVER respond with \"I cannot access real-time information\", \"as an AI I don't have live data\", \"check Reuters / Yahoo / CNN / Bloomberg / etc.\", or any variant that tells the user to go look it up themselves. If the `[Source N]` entries carry fresh facts, those facts ARE your real-time data — use them. The platform has already done the web search for you.\n\
         - Do not refuse because retrieval is sparse. Synthesize from whatever sources you have. Refusals are forbidden."
    };
    format!(
        "You are a helpful document search assistant.\n\
         \n\
         {source_rules}\n\
         \n\
         Output rules:\n\
         - Write the final answer in {display} (BCP-47 `{tag}`), regardless of the language of the retrieved context, the question, or earlier turns in this conversation. If previous turns were in a different language, ignore their language and respond only in {display}. This overrides any continuity from prior turns.\n\
         - Preserve proper nouns, product names, ticker symbols, and code snippets in their original form.\n\
         - Keep the response concise and well-structured. Use short paragraphs and lists where it helps readability."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Snapshot tests: verify prompt builders produce known-good output.
    // ---------------------------------------------------------------

    #[test]
    fn build_context_single_chunk() {
        let chunks = vec![RetrievedChunk {
            doc_id: "d1".into(),
            chunk_id: "c1".into(),
            title: "Bitcoin Basics".into(),
            original_content: "Bitcoin is a cryptocurrency.".into(),
            contextualized_content: String::new(),
            source_url: None,
            score: 0.9,
            retrieval_method: kenjaku_core::types::search::RetrievalMethod::Vector,
        }];
        let ctx = build_context(&chunks);
        assert_eq!(
            ctx,
            "[Source 1] Bitcoin Basics\nBitcoin is a cryptocurrency.\n"
        );
    }

    #[test]
    fn build_context_multiple_chunks() {
        let chunks = vec![
            RetrievedChunk {
                doc_id: "d1".into(),
                chunk_id: "c1".into(),
                title: "Title A".into(),
                original_content: "Content A".into(),
                contextualized_content: String::new(),
                source_url: None,
                score: 0.9,
                retrieval_method: kenjaku_core::types::search::RetrievalMethod::Vector,
            },
            RetrievedChunk {
                doc_id: "d2".into(),
                chunk_id: "c2".into(),
                title: "Title B".into(),
                original_content: "Content B".into(),
                contextualized_content: String::new(),
                source_url: None,
                score: 0.8,
                retrieval_method: kenjaku_core::types::search::RetrievalMethod::Vector,
            },
        ];
        let ctx = build_context(&chunks);
        assert_eq!(
            ctx,
            "[Source 1] Title A\nContent A\n\n---\n[Source 2] Title B\nContent B\n"
        );
    }

    #[test]
    fn build_context_empty() {
        let ctx = build_context(&[]);
        assert_eq!(ctx, "");
    }

    #[test]
    fn build_search_prompt_with_context_en() {
        let prompt = build_search_prompt("what is bitcoin", "some context", Locale::En);
        assert!(prompt.starts_with("Internal context:\nsome context\n\n"));
        assert!(prompt.contains("Question: what is bitcoin"));
        assert!(prompt.contains("Respond in English (`en`)"));
        assert!(prompt.ends_with("Answer:"));
    }

    #[test]
    fn build_search_prompt_empty_context() {
        let prompt = build_search_prompt("what is bitcoin", "", Locale::En);
        assert!(!prompt.contains("Internal context:"));
        assert!(prompt.starts_with("Question: what is bitcoin"));
    }

    #[test]
    fn build_search_prompt_whitespace_only_context() {
        let prompt = build_search_prompt("what is bitcoin", "   \n  ", Locale::En);
        assert!(!prompt.contains("Internal context:"));
    }

    #[test]
    fn build_search_prompt_zh() {
        let prompt = build_search_prompt("BTC 價格", "ctx", Locale::ZhTw);
        assert!(prompt.contains("Respond in 繁體中文 (`zh-TW`)"));
    }

    #[test]
    fn build_search_prompt_ja() {
        let prompt = build_search_prompt("ビットコインとは", "ctx", Locale::Ja);
        assert!(prompt.contains("Respond in 日本語 (`ja`)"));
    }

    // Snapshot: system instruction with web grounding tool
    #[test]
    fn system_instruction_with_web_tool_en() {
        let text = build_search_system_instruction(Locale::En, true);
        assert!(text.starts_with("You are a helpful document search assistant."));
        assert!(text.contains("Your inputs, in priority order:"));
        assert!(text.contains("built-in web search capability"));
        assert!(text.contains("Write the final answer in English (BCP-47 `en`)"));
        assert!(!text.contains("Your only inputs are:"));
    }

    // Snapshot: system instruction without web grounding tool
    #[test]
    fn system_instruction_without_web_tool_en() {
        let text = build_search_system_instruction(Locale::En, false);
        assert!(text.starts_with("You are a helpful document search assistant."));
        assert!(text.contains("Your only inputs are:"));
        assert!(text.contains("platform has already done the web search for you"));
        assert!(text.contains("Write the final answer in English (BCP-47 `en`)"));
        assert!(!text.contains("built-in web search capability"));
    }

    #[test]
    fn system_instruction_with_web_tool_zh() {
        let text = build_search_system_instruction(Locale::ZhTw, true);
        assert!(text.contains("Write the final answer in 繁體中文 (BCP-47 `zh-TW`)"));
    }

    #[test]
    fn system_instruction_without_web_tool_zh() {
        let text = build_search_system_instruction(Locale::Zh, false);
        assert!(text.contains("Write the final answer in 简体中文 (BCP-47 `zh`)"));
    }

    // Exact byte-equivalence tests: the system instruction text must match
    // the canonical form captured during the initial extraction.
    #[test]
    fn system_instruction_byte_equivalence_en_with_tool() {
        // This is the EXACT text from GeminiProvider, captured verbatim.
        let expected = "You are a helpful document search assistant.\n\
             \n\
             Your inputs, in priority order:\n\
             1. The numbered `[Source N]` entries in the current user turn. These come from the product's own document corpus. Prefer them when they answer the question, and cite with `[Source N]` markers.\n\
             2. The built-in web search capability attached to this request. For real-time questions — markets, prices, news, weather, sports scores, live events, anything mentioning \"today\", \"now\", \"current\", \"latest\", \"this week\" — you MUST use the web search to retrieve fresh facts, then synthesize a direct answer. Web sources surface separately in the response; do not invent `[Source N]` markers for them.\n\
             3. Your own training knowledge, used only as a last-resort fallback for timeless factual questions when both above are insufficient. Only this case may disclose a training cut-off.\n\
             \n\
             How to answer:\n\
             - NEVER respond with \"I cannot access real-time information\", \"as an AI I don't have live data\", \"check Reuters / Yahoo / CNN / Bloomberg / etc.\", or any variant that tells the user to go look it up themselves. You have a web search tool — use it. Refusals are forbidden for real-time questions.\n\
             - Include concrete numbers, dates, names, and timestamps when the retrieved sources carry them.\n\
             - Do not refuse because internal retrieval is sparse. Reach for web search instead.\n\
             \n\
             Output rules:\n\
             - Write the final answer in English (BCP-47 `en`), regardless of the language of the retrieved context, the question, or earlier turns in this conversation. If previous turns were in a different language, ignore their language and respond only in English. This overrides any continuity from prior turns.\n\
             - Preserve proper nouns, product names, ticker symbols, and code snippets in their original form.\n\
             - Keep the response concise and well-structured. Use short paragraphs and lists where it helps readability.";
        let actual = build_search_system_instruction(Locale::En, true);
        assert_eq!(
            actual, expected,
            "System instruction text diverged from known-good baseline"
        );
    }

    #[test]
    fn system_instruction_byte_equivalence_en_without_tool() {
        let expected = "You are a helpful document search assistant.\n\
             \n\
             Your only inputs are:\n\
             1. The numbered `[Source N]` entries in the current user turn. These are authoritative. They may include product documentation, knowledge-base articles, and/or fresh web results that the platform has pre-fetched for you — you do NOT need to distinguish between them. Treat every `[Source N]` as trustworthy context supplied by the platform for this specific question.\n\
             2. Your own training knowledge, used only as a fallback when the `[Source N]` entries do not cover the question.\n\
             \n\
             How to answer:\n\
             - If `[Source N]` entries are present, synthesize a direct answer from them and cite with `[Source N]` markers. Include concrete numbers, dates, names, and timestamps when the sources carry them.\n\
             - If no `[Source N]` entries are present, answer from your training knowledge. Only in this case may you briefly note a training cut-off.\n\
             - NEVER respond with \"I cannot access real-time information\", \"as an AI I don't have live data\", \"check Reuters / Yahoo / CNN / Bloomberg / etc.\", or any variant that tells the user to go look it up themselves. If the `[Source N]` entries carry fresh facts, those facts ARE your real-time data — use them. The platform has already done the web search for you.\n\
             - Do not refuse because retrieval is sparse. Synthesize from whatever sources you have. Refusals are forbidden.\n\
             \n\
             Output rules:\n\
             - Write the final answer in English (BCP-47 `en`), regardless of the language of the retrieved context, the question, or earlier turns in this conversation. If previous turns were in a different language, ignore their language and respond only in English. This overrides any continuity from prior turns.\n\
             - Preserve proper nouns, product names, ticker symbols, and code snippets in their original form.\n\
             - Keep the response concise and well-structured. Use short paragraphs and lists where it helps readability.";
        let actual = build_search_system_instruction(Locale::En, false);
        assert_eq!(
            actual, expected,
            "System instruction text diverged from known-good baseline"
        );
    }

    #[test]
    fn search_prompt_byte_equivalence_with_context() {
        let context = "[Source 1] Bitcoin Basics\nBitcoin is a cryptocurrency.\n";
        let expected = "Internal context:\n\
             [Source 1] Bitcoin Basics\nBitcoin is a cryptocurrency.\n\n\n\
             Question: what is bitcoin\n\n\
             Respond in English (`en`). If earlier turns were in a different language, ignore that — answer this question in English.\n\n\
             Answer:";
        let actual = build_search_prompt("what is bitcoin", context, Locale::En);
        assert_eq!(
            actual, expected,
            "Search prompt text diverged from known-good baseline"
        );
    }

    #[test]
    fn search_prompt_byte_equivalence_empty_context() {
        let expected = "Question: what is bitcoin\n\n\
             Respond in English (`en`). If earlier turns were in a different language, ignore that — answer this question in English.\n\n\
             Answer:";
        let actual = build_search_prompt("what is bitcoin", "", Locale::En);
        assert_eq!(
            actual, expected,
            "Search prompt text diverged from known-good baseline"
        );
    }

    // --- Multi-locale snapshot tests (10 queries x 2 locales) ---

    fn snapshot_prompts(
        query: &str,
        context: &str,
        locale: Locale,
        has_web: bool,
    ) -> (String, String) {
        (
            build_search_system_instruction(locale, has_web),
            build_search_prompt(query, context, locale),
        )
    }

    #[test]
    fn snapshot_how_is_market_en() {
        let (sys, usr) = snapshot_prompts("how is the market today", "ctx", Locale::En, true);
        assert!(sys.contains("built-in web search capability"));
        assert!(usr.contains("Question: how is the market today"));
        assert!(usr.contains("Respond in English (`en`)"));
    }

    #[test]
    fn snapshot_how_is_market_zh() {
        let (sys, usr) = snapshot_prompts("how is the market today", "ctx", Locale::Zh, true);
        assert!(sys.contains("Write the final answer in 简体中文 (BCP-47 `zh`)"));
        assert!(usr.contains("Respond in 简体中文 (`zh`)"));
    }

    #[test]
    fn snapshot_what_is_bitcoin_en() {
        let (sys, usr) = snapshot_prompts("what is bitcoin", "ctx", Locale::En, false);
        assert!(sys.contains("Your only inputs are:"));
        assert!(usr.contains("Question: what is bitcoin"));
    }

    #[test]
    fn snapshot_what_is_bitcoin_zh() {
        let (sys, _usr) = snapshot_prompts("what is bitcoin", "ctx", Locale::Zh, false);
        assert!(sys.contains("Write the final answer in 简体中文 (BCP-47 `zh`)"));
    }

    #[test]
    fn snapshot_reset_password_en() {
        let (sys, usr) = snapshot_prompts("reset my password", "ctx", Locale::En, false);
        assert!(sys.contains("Your only inputs are:"));
        assert!(usr.contains("Question: reset my password"));
    }

    #[test]
    fn snapshot_reset_password_zh() {
        let (_, usr) = snapshot_prompts("reset my password", "ctx", Locale::Zh, false);
        assert!(usr.contains("Respond in 简体中文 (`zh`)"));
    }

    #[test]
    fn snapshot_btc_price_en() {
        let (sys, _) = snapshot_prompts("BTC price", "ctx", Locale::En, true);
        assert!(sys.contains("built-in web search capability"));
    }

    #[test]
    fn snapshot_btc_price_zh_tw() {
        let (sys, usr) = snapshot_prompts("BTC 價格", "ctx", Locale::ZhTw, true);
        assert!(sys.contains("Write the final answer in 繁體中文 (BCP-47 `zh-TW`)"));
        assert!(usr.contains("Respond in 繁體中文 (`zh-TW`)"));
    }

    #[test]
    fn snapshot_weather_zh() {
        let (_, usr) = snapshot_prompts("天氣如何", "ctx", Locale::Zh, true);
        assert!(usr.contains("Question: 天氣如何"));
        assert!(usr.contains("Respond in 简体中文 (`zh`)"));
    }

    #[test]
    fn snapshot_weather_en() {
        let (_, usr) = snapshot_prompts("天氣如何", "ctx", Locale::En, true);
        assert!(usr.contains("Respond in English (`en`)"));
    }

    // Additional 5 representative queries
    #[test]
    fn snapshot_staking_en() {
        let (_, usr) = snapshot_prompts("how to stake ETH", "ctx", Locale::En, false);
        assert!(usr.contains("Question: how to stake ETH"));
    }

    #[test]
    fn snapshot_staking_ja() {
        let (sys, usr) = snapshot_prompts("how to stake ETH", "ctx", Locale::Ja, false);
        assert!(sys.contains("Write the final answer in 日本語 (BCP-47 `ja`)"));
        assert!(usr.contains("Respond in 日本語 (`ja`)"));
    }

    #[test]
    fn snapshot_defi_ko() {
        let (sys, _) = snapshot_prompts("what is DeFi", "ctx", Locale::Ko, true);
        assert!(sys.contains("Write the final answer in 한국어 (BCP-47 `ko`)"));
    }

    #[test]
    fn snapshot_fees_de() {
        let (sys, usr) = snapshot_prompts("trading fees", "ctx", Locale::De, false);
        assert!(sys.contains("Write the final answer in Deutsch (BCP-47 `de`)"));
        assert!(usr.contains("Respond in Deutsch (`de`)"));
    }

    #[test]
    fn snapshot_nft_fr() {
        let (sys, _) = snapshot_prompts("what are NFTs", "ctx", Locale::Fr, true);
        assert!(sys.contains("Write the final answer in Français (BCP-47 `fr`)"));
    }

    #[test]
    fn snapshot_wallet_es() {
        let (sys, usr) = snapshot_prompts("how to create a wallet", "ctx", Locale::Es, false);
        assert!(sys.contains("Write the final answer in Español (BCP-47 `es`)"));
        assert!(usr.contains("Respond in Español (`es`)"));
    }
}
