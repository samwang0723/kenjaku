use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};

use kenjaku_core::config::LlmConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::assets::{Asset, AssetType};
use kenjaku_core::types::intent::{Intent, IntentClassification};
use kenjaku_core::types::locale::{DetectedLocale, Locale};
use kenjaku_core::types::message::{ContentPart, Message, Role};
use kenjaku_core::types::preprocess::{PreprocessWire, QueryPreprocessing};
use kenjaku_core::types::search::{
    LlmResponse, LlmSource, LlmUsage, StreamChunk, StreamChunkType, TranslationResult,
};
use kenjaku_core::types::suggestion::ClusterQuestions;
use std::collections::HashMap;

/// Gemini LLM provider implementation.
///
/// **Merged-JSON mode (current)**: `generate` / `generate_stream` ask
/// Gemini for a `{message, assets, suggestions}` structured-output
/// response via `responseSchema`. Gemini rejects attaching built-in
/// tools (including `google_search`) alongside `responseSchema`, so
/// no tool is attached on these calls. The platform's `WebSearchProvider`
/// (Brave) handles real-time freshness by pre-injecting synthetic
/// `[Source N]` chunks into the user turn before this provider runs.
pub struct GeminiProvider {
    client: Client,
    config: LlmConfig,
    base_url: String,
}

impl GeminiProvider {
    /// Construct a Gemini provider.
    ///
    /// Post-merged-JSON: no `use_google_search_tool` flag — the search
    /// path never attaches Gemini's built-in tools because
    /// `responseSchema` (which we rely on for the merged answer shape)
    /// is incompatible with them. Web freshness is supplied upstream
    /// by `WebSearchProvider` (Brave).
    pub fn new(config: LlmConfig) -> Self {
        let base_url = config.base_url.clone();
        Self {
            client: Client::new(),
            base_url,
            config,
        }
    }

    /// Returns the ALL-CAPS service tier for the `serviceTier` request field.
    fn service_tier_value(&self) -> Option<String> {
        Some(self.config.service_tier.as_api_value().to_string())
    }

    /// Estimate cost in USD for a given token usage, factoring in model
    /// base pricing and the configured service tier multiplier.
    ///
    /// Pricing per 1M tokens (standard tier):
    /// - gemini-3.1-pro*:         $2.00 input,  $12.00 output (<=200k ctx)
    /// - gemini-2.5-flash*:       $0.30 input,   $2.50 output
    /// - gemini-2.5-flash-lite*:  $0.10 input,   $0.40 output
    /// - gemini-3.1-flash-lite*:  $0.10 input,   $0.40 output
    /// - fallback (unknown):      $0.30 input,   $2.50 output (flash rate)
    fn estimate_cost(&self, prompt_tokens: u32, completion_tokens: u32) -> Option<f64> {
        let (input_per_m, output_per_m) = cost_rates_for_model(&self.config.model);
        let tier_mult = self.config.service_tier.cost_multiplier();
        let cost = tier_mult
            * ((prompt_tokens as f64 / 1_000_000.0) * input_per_m
                + (completion_tokens as f64 / 1_000_000.0) * output_per_m);

        Some((cost * 1_000_000.0).round() / 1_000_000.0) // round to 6 decimal places
    }

    /// The JSON schema the merged-generate call asks Gemini to emit.
    ///
    /// Extracted so the sync and streaming code paths share the exact
    /// same schema. Shape:
    /// ```json
    /// {
    ///   "message":     "<string>",
    ///   "assets":      [{"symbol": "...", "type": "stock" | "crypto"}, ...],
    ///   "suggestions": ["...", "...", "..."]
    /// }
    /// ```
    fn merged_response_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "OBJECT",
            "properties": {
                "message": {"type": "STRING"},
                "assets": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "symbol": {"type": "STRING"},
                            "type": {"type": "STRING", "enum": ["stock", "crypto"]}
                        },
                        "required": ["symbol", "type"]
                    }
                },
                "suggestions": {"type": "ARRAY", "items": {"type": "STRING"}}
            },
            "required": ["message", "suggestions"],
            "propertyOrdering": ["message", "assets", "suggestions"]
        })
    }

    /// Convert LLM-agnostic `Message` values to Gemini wire format.
    ///
    /// Returns `(system_instruction, contents)`:
    /// - If the first message has `Role::System`, it becomes the
    ///   `systemInstruction` field; otherwise `None`.
    /// - All remaining messages map to `GeminiContent` entries with
    ///   `role` set to `"user"` or `"model"` (Gemini's term for
    ///   assistant). System messages after the first are treated as
    ///   user messages (Gemini only supports one system instruction).
    fn messages_to_wire(messages: &[Message]) -> (Option<GeminiContent>, Vec<GeminiContent>) {
        if messages.is_empty() {
            return (None, Vec::new());
        }

        let (system, body) = if messages[0].role == Role::System {
            let sys = Self::message_to_gemini(&messages[0], "system");
            (Some(sys), &messages[1..])
        } else {
            (None, messages)
        };

        let contents = body
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User | Role::System => "user",
                    Role::Assistant => "model",
                };
                Self::message_to_gemini(m, role)
            })
            .collect();

        (system, contents)
    }

    /// Convert a single `Message` into a `GeminiContent` with the given role.
    fn message_to_gemini(msg: &Message, role: &str) -> GeminiContent {
        let parts = msg
            .parts
            .iter()
            .map(|p| match p {
                ContentPart::Text(s) => GeminiPart::text(s.clone()),
            })
            .collect();
        GeminiContent {
            parts,
            role: Some(role.to_string()),
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    #[instrument(skip(self, messages), fields(model = %self.config.model, msg_count = messages.len()))]
    async fn generate(&self, messages: &[Message]) -> Result<LlmResponse> {
        let (system_instruction, contents) = Self::messages_to_wire(messages);

        // Merged-JSON mode: structured output via responseSchema.
        // `tools` must be `None` — Gemini rejects tools + responseSchema
        // together. Brave provides web freshness via pre-injected chunks.
        let request = GeminiRequest {
            contents,
            system_instruction,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(self.config.max_tokens),
                temperature: Some(self.config.temperature),
                response_mime_type: Some("application/json".to_string()),
                response_schema: Some(Self::merged_response_schema()),
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Gemini response: {e}")))?;

        let raw_text = result
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        // Parse the structured payload. On any failure, fall back to
        // `{answer: raw_text, assets: [], suggestions: []}` so the
        // pipeline still produces something (graceful degradation).
        let (answer, assets, suggestions) = parse_merged_response(&raw_text);

        // Extract grounding sources if the provider ever attaches them
        // to non-tool responses (currently always empty in merged mode).
        let sources = result
            .candidates
            .first()
            .and_then(|c| c.grounding_metadata.as_ref())
            .map(|meta| {
                meta.grounding_chunks
                    .iter()
                    .filter_map(|chunk| {
                        chunk.web.as_ref().map(|w| LlmSource {
                            title: w.title.clone().unwrap_or_default(),
                            url: w.uri.clone().unwrap_or_default(),
                            snippet: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = result.usage_metadata.map(|u| {
            let prompt = u.prompt_token_count.unwrap_or(0);
            let completion = u.candidates_token_count.unwrap_or(0);
            LlmUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: u
                    .total_token_count
                    .unwrap_or(prompt.saturating_add(completion)),
                cost_usd: self.estimate_cost(prompt, completion),
            }
        });

        Ok(LlmResponse {
            answer,
            sources,
            model: self.config.model.clone(),
            usage,
            assets,
            suggestions,
        })
    }

    /// Fast stateless single-shot completion — no tools, no history, no
    /// system instruction, capped tokens. Used by the intent classifier
    /// and similar utility calls. Keeps intent classification at ~1s
    /// instead of the ~5s that would apply if google_search was attached.
    ///
    /// Cap at 400 tokens: 256 was too tight for CJK→English translation
    /// where a ~500-char input can produce ~280 output tokens. 400 leaves
    /// headroom without undermining the latency characteristic.
    #[instrument(skip(self, prompt), fields(model = %self.config.model))]
    async fn generate_brief(&self, prompt: &str) -> Result<LlmResponse> {
        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::text(prompt.to_string())],
                role: Some("user".to_string()),
            }],
            system_instruction: None,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(400),
                temperature: Some(0.0),
                response_mime_type: None,
                response_schema: None,
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini brief request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Gemini brief response: {e}")))?;

        let answer = result
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let usage = result.usage_metadata.map(|u| {
            let prompt = u.prompt_token_count.unwrap_or(0);
            let completion = u.candidates_token_count.unwrap_or(0);
            LlmUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: u
                    .total_token_count
                    .unwrap_or(prompt.saturating_add(completion)),
                cost_usd: self.estimate_cost(prompt, completion),
            }
        });

        Ok(LlmResponse {
            answer,
            sources: Vec::new(),
            model: self.config.model.clone(),
            usage,
            assets: Vec::new(),
            suggestions: Vec::new(),
        })
    }

    #[instrument(skip(self, messages), fields(model = %self.config.model, msg_count = messages.len()))]
    async fn generate_stream(
        &self,
        messages: &[Message],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let (system_instruction, contents) = Self::messages_to_wire(messages);

        // Merged-JSON streaming: same responseSchema contract as
        // `generate`, but each SSE event carries a partial slice of
        // the JSON output. A stateful parser unpacks the `message`
        // field into delta chars as they arrive, and buffers the tail
        // so we can parse `assets` + `suggestions` once the stream
        // finishes.
        let request = GeminiRequest {
            contents,
            system_instruction,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(self.config.max_tokens),
                temperature: Some(self.config.temperature),
                response_mime_type: Some("application/json".to_string()),
                response_schema: Some(Self::merged_response_schema()),
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini streaming request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        info!(content_type = %content_type, "Gemini stream response opened");

        // Use eventsource-stream — handles SSE framing (CRLF/LF, multi-line data,
        // event/id/retry fields, partial chunks) properly. We just consume the
        // parsed events and convert each `data` payload to a StreamChunk.
        use eventsource_stream::Eventsource;
        use futures::StreamExt;

        // Capture model name + pricing context so the async closure can
        // assemble `LlmUsage` without holding `&self`.
        let model = self.config.model.clone();
        let tier_mult = self.config.service_tier.cost_multiplier();
        let pricing = cost_rates_for_model(&model);

        // Shared merged-JSON streaming parser: threads through each
        // event so `message` chars can be emitted as deltas and the
        // raw accumulated JSON can be final-parsed for assets +
        // suggestions on the terminal event.
        //
        // Wrap in `Arc<Mutex<...>>` because `filter_map`'s closure is
        // `FnMut` but the inner `async move` block we return needs to
        // capture the parser across await points — a bare `&mut` won't
        // satisfy both requirements. The mutex is uncontended (the
        // stream is single-consumer by construction).
        let parser = std::sync::Arc::new(std::sync::Mutex::new(MergedJsonStreamParser::new()));

        let event_stream = response.bytes_stream().eventsource();
        let stream = event_stream.filter_map(move |event_result| {
            let model = model.clone();
            let pricing = pricing;
            let parser = parser.clone();
            async move {
                match event_result {
                    Ok(event) => {
                        tracing::info!(
                            event_len = event.data.len(),
                            raw = %event.data,
                            "Gemini SSE event (full)"
                        );
                        if event.data.trim() == "[DONE]" {
                            // Final [DONE] marker (rare — Gemini usually
                            // signals via `finishReason` on the last
                            // content event). Finalize the parser and
                            // emit any assets/suggestions we buffered.
                            let (assets, suggestions) =
                                parser.lock().expect("parser mutex").finalize();
                            return Some(Ok(StreamChunk {
                                delta: String::new(),
                                chunk_type: StreamChunkType::Answer,
                                finished: true,
                                grounding: None,
                                usage: None,
                                assets: Some(assets),
                                suggestions: Some(suggestions),
                            }));
                        }

                        let response: GeminiResponse = match serde_json::from_str(&event.data) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(
                                    parse_error = %e,
                                    sample = %event.data.chars().take(400).collect::<String>(),
                                    "Failed to parse Gemini SSE event — dropping"
                                );
                                return None;
                            }
                        };

                        let text = response
                            .candidates
                            .first()
                            .map(|c| {
                                c.content
                                    .parts
                                    .iter()
                                    .map(|p| p.text_str())
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default();

                        let finish_reason = response
                            .candidates
                            .first()
                            .and_then(|c| c.finish_reason.clone());
                        let finished = finish_reason.is_some();
                        if let Some(reason) = finish_reason.as_ref() {
                            info!(
                                finish_reason = %reason,
                                text_len = text.len(),
                                "Gemini stream final event"
                            );
                        }

                        // Feed the raw JSON slice through the parser.
                        // Produces (delta_to_emit, _parser_state).
                        let (delta, _state) = parser.lock().expect("parser mutex").push(&text);

                        // Extract grounding sources — still honored
                        // even in merged-JSON mode for future-proofing.
                        let grounding: Option<Vec<LlmSource>> = response
                            .candidates
                            .first()
                            .and_then(|c| c.grounding_metadata.as_ref())
                            .map(|meta| {
                                meta.grounding_chunks
                                    .iter()
                                    .filter_map(|gc| {
                                        gc.web.as_ref().and_then(|w| {
                                            w.uri.as_ref().map(|uri| LlmSource {
                                                title: w.title.clone().unwrap_or_default(),
                                                url: uri.clone(),
                                                snippet: None,
                                            })
                                        })
                                    })
                                    .collect()
                            })
                            .filter(|v: &Vec<LlmSource>| !v.is_empty());
                        if let Some(g) = grounding.as_ref() {
                            info!(
                                grounding_count = g.len(),
                                finished, "Captured Gemini grounding sources from stream event"
                            );
                        }

                        // Harvest usageMetadata from the event (typically only
                        // present on the finish event). Each SSE chunk may
                        // independently carry it; we forward as-is, and the
                        // consumer takes the last non-None.
                        let (input_per_m, output_per_m) = pricing;
                        let usage = response.usage_metadata.as_ref().map(|u| {
                            let prompt = u.prompt_token_count.unwrap_or(0);
                            let completion = u.candidates_token_count.unwrap_or(0);
                            let cost = tier_mult
                                * ((prompt as f64 / 1_000_000.0) * input_per_m
                                    + (completion as f64 / 1_000_000.0) * output_per_m);
                            let cost_rounded = (cost * 1_000_000.0).round() / 1_000_000.0;
                            LlmUsage {
                                prompt_tokens: prompt,
                                completion_tokens: completion,
                                total_tokens: u
                                    .total_token_count
                                    .unwrap_or(prompt.saturating_add(completion)),
                                cost_usd: Some(cost_rounded),
                            }
                        });
                        if let Some(u) = usage.as_ref() {
                            info!(
                                prompt_tokens = u.prompt_tokens,
                                completion_tokens = u.completion_tokens,
                                cost_usd = ?u.cost_usd,
                                model = %model,
                                "Captured Gemini usage from stream event"
                            );
                        }

                        // On the terminal event, finalize the parser
                        // to recover assets + suggestions from the
                        // fully buffered JSON.
                        let (assets, suggestions) = if finished {
                            let (a, s) = parser.lock().expect("parser mutex").finalize();
                            (Some(a), Some(s))
                        } else {
                            (None, None)
                        };

                        if delta.is_empty() && !finished && grounding.is_none() && usage.is_none() {
                            return None;
                        }

                        Some(Ok(StreamChunk {
                            delta,
                            chunk_type: StreamChunkType::Answer,
                            finished,
                            grounding,
                            usage,
                            assets,
                            suggestions,
                        }))
                    }
                    Err(e) => Some(Err(Error::Llm(format!("SSE parse error: {e}")))),
                }
            }
        });

        Ok(Box::pin(stream))
    }

    #[instrument(skip(self))]
    async fn translate(&self, text: &str) -> Result<(TranslationResult, Option<LlmUsage>)> {
        let prompt =
            kenjaku_core::prompts::render(kenjaku_core::prompts::TRANSLATE, &[("text", text)]);

        let schema = serde_json::json!({
            "type": "OBJECT",
            "properties": {
                "detected_locale": {
                    "type": "STRING",
                    "description": "BCP-47 source language tag, e.g. en, zh, zh-TW, ja, ko, de, fr, es, pt, it, ru."
                },
                "normalized_query": {
                    "type": "STRING",
                    "description": "Query rewritten in canonical English."
                }
            },
            "required": ["detected_locale", "normalized_query"]
        });

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::text(prompt)],
                role: Some("user".to_string()),
            }],
            system_instruction: None,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(256),
                temperature: Some(0.0),
                response_mime_type: Some("application/json".to_string()),
                response_schema: Some(schema),
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Translation(format!("Gemini translation failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Translation(format!(
                "Gemini returned {status}: {body}"
            )));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Translation(format!("Failed to parse translation: {e}")))?;

        let raw_text = result
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let usage = result.usage_metadata.as_ref().map(|u| {
            let prompt = u.prompt_token_count.unwrap_or(0);
            let completion = u.candidates_token_count.unwrap_or(0);
            LlmUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: u
                    .total_token_count
                    .unwrap_or(prompt.saturating_add(completion)),
                cost_usd: self.estimate_cost(prompt, completion),
            }
        });

        // Parse the structured output. On any failure, fall back to the
        // raw user text + en — never block search.
        match serde_json::from_str::<TranslatorJson>(&raw_text) {
            Ok(parsed) => {
                let detected = DetectedLocale::from_bcp47(&parsed.detected_locale);
                let normalized = if parsed.normalized_query.trim().is_empty() {
                    text.to_string()
                } else {
                    parsed.normalized_query
                };
                Ok((
                    TranslationResult {
                        normalized,
                        detected_locale: detected,
                    },
                    usage,
                ))
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw = %raw_text,
                    "Translator returned malformed JSON; falling back to raw text + en"
                );
                Ok((
                    TranslationResult {
                        normalized: text.to_string(),
                        detected_locale: DetectedLocale::Supported(Locale::En),
                    },
                    usage,
                ))
            }
        }
    }

    /// **Two-call pipeline (Phase A)** — merged preamble.
    ///
    /// Issues a single Gemini call with `responseSchema` structured
    /// output. The schema enforces an enum on `intent` so the model
    /// can only return one of the 8 valid categories; the parser falls
    /// back to a safe (Unknown, En, raw_query) tuple on any
    /// schema-violation or HTTP failure.
    ///
    /// Smoke-tested against this exact prompt + schema across 30
    /// queries spanning 8 locales — 100% compliance on JSON validity,
    /// field completeness, intent enum, and locale match. See
    /// `scripts/smoke/two_call_pipeline_smoke.py`.
    #[instrument(skip(self))]
    async fn preprocess_query(
        &self,
        query: &str,
    ) -> Result<(QueryPreprocessing, Option<LlmUsage>)> {
        let prompt =
            kenjaku_core::prompts::render(kenjaku_core::prompts::PREPROCESS, &[("query", query)]);

        let schema = serde_json::json!({
            "type": "OBJECT",
            "properties": {
                "intent": {
                    "type": "STRING",
                    "enum": [
                        "factual", "navigational", "how_to", "comparison",
                        "troubleshooting", "exploratory", "conversational", "unknown"
                    ],
                    "description": "Single intent category from the fixed list."
                },
                "detected_locale": {
                    "type": "STRING",
                    "description": "BCP-47 source language tag."
                },
                "normalized_query": {
                    "type": "STRING",
                    "description": "Query rewritten in canonical English."
                }
            },
            "required": ["intent", "detected_locale", "normalized_query"],
            "propertyOrdering": ["intent", "detected_locale", "normalized_query"]
        });

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::text(prompt)],
                role: Some("user".to_string()),
            }],
            system_instruction: None,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(300),
                temperature: Some(0.0),
                response_mime_type: Some("application/json".to_string()),
                response_schema: Some(schema),
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini preprocess failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse preprocess: {e}")))?;

        let raw_text = result
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let usage = result.usage_metadata.as_ref().map(|u| {
            let prompt = u.prompt_token_count.unwrap_or(0);
            let completion = u.candidates_token_count.unwrap_or(0);
            LlmUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: u
                    .total_token_count
                    .unwrap_or(prompt.saturating_add(completion)),
                cost_usd: self.estimate_cost(prompt, completion),
            }
        });

        // Parse the structured output. On any failure (Gemini returned
        // junk, schema violation, network truncation, etc.), fall back
        // to a safe (Unknown, En, raw_query) tuple. Never block search.
        match serde_json::from_str::<PreprocessWire>(&raw_text) {
            Ok(wire) => {
                let intent = Intent::from_raw(&wire.intent);
                let confidence = if intent == Intent::Unknown { 0.0 } else { 0.85 };
                let detected = DetectedLocale::from_bcp47(&wire.detected_locale);
                let normalized = if wire.normalized_query.trim().is_empty() {
                    query.to_string()
                } else {
                    wire.normalized_query
                };
                Ok((
                    QueryPreprocessing {
                        intent: IntentClassification { intent, confidence },
                        translation: TranslationResult {
                            normalized,
                            detected_locale: detected,
                        },
                    },
                    usage,
                ))
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw = %raw_text,
                    "Preprocess returned malformed JSON; falling back to (Unknown, En, raw)"
                );
                Ok((
                    QueryPreprocessing {
                        intent: IntentClassification {
                            intent: Intent::Unknown,
                            confidence: 0.0,
                        },
                        translation: TranslationResult {
                            normalized: query.to_string(),
                            // Emit `Unsupported { tag: "" }` so
                            // `resolve_translation` records
                            // `FallbackEn` provenance (not
                            // `LlmDetected`) — matches the parallel
                            // path's behavior and the Brain trait
                            // default impl.
                            detected_locale: DetectedLocale::Unsupported { tag: String::new() },
                        },
                    },
                    usage,
                ))
            }
        }
    }

    #[instrument(skip(self, excerpt))]
    async fn generate_cluster_questions(&self, excerpt: &str) -> Result<ClusterQuestions> {
        let prompt = format!(
            "You generate search suggestion questions for a document search engine, for a\n\
             cluster of related document excerpts.\n\
             \n\
             Excerpts:\n\
             {excerpt}\n\
             \n\
             Task:\n\
             1. Produce a 3-5 word English topic label for this cluster.\n\
             2. For EACH of the 8 locales below, produce 3-5 natural questions a curious\n\
                reader would plausibly ask and that the documents in this cluster could\n\
                answer.\n\
             \n\
             Locales: en (English), zh (\u{7b80}\u{4f53}\u{4e2d}\u{6587}), zh-TW (\u{7e41}\u{9ad4}\u{4e2d}\u{6587}), ja (\u{65e5}\u{672c}\u{8a9e}),\n\
                      ko (\u{d55c}\u{ad6d}\u{c5b4}), de (Deutsch), fr (Fran\u{e7}ais), es (Espa\u{f1}ol)\n\
             \n\
             Rules (apply to ALL locales):\n\
             - Native, natural phrasing - not translated-sounding.\n\
             - Preserve product names, tickers, and proper nouns verbatim across locales.\n\
             - No financial advice, no price predictions, no \"should I buy / sell\" framing.\n\
             - No yes/no questions - prefer \"what / how / why / when\".\n\
             - Keep each question under 80 characters.\n\
             \n\
             Output strict JSON matching the response schema."
        );

        let locale_prop = serde_json::json!({
            "type": "ARRAY",
            "items": { "type": "STRING" }
        });
        let schema = serde_json::json!({
            "type": "OBJECT",
            "properties": {
                "label": { "type": "STRING" },
                "questions": {
                    "type": "OBJECT",
                    "properties": {
                        "en":    locale_prop,
                        "zh":    locale_prop,
                        "zh-TW": locale_prop,
                        "ja":    locale_prop,
                        "ko":    locale_prop,
                        "de":    locale_prop,
                        "fr":    locale_prop,
                        "es":    locale_prop,
                    },
                    "required": ["en", "zh", "zh-TW", "ja", "ko", "de", "fr", "es"]
                }
            },
            "required": ["label", "questions"]
        });

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::text(prompt)],
                role: Some("user".to_string()),
            }],
            system_instruction: None,
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(2048),
                temperature: Some(0.4),
                response_mime_type: Some("application/json".to_string()),
                response_schema: Some(schema),
            }),
            service_tier: self.service_tier_value(),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "Gemini cluster-questions request failed");
                Error::Llm(format!("gemini cluster-questions transport: {e}"))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                %status,
                body = %body,
                "Gemini cluster-questions returned non-success"
            );
            return Err(Error::Llm(format!(
                "gemini cluster-questions http {status}: {body}"
            )));
        }

        let result: GeminiResponse = response.json().await.map_err(|e| {
            tracing::warn!(error = %e, "Gemini cluster-questions parse failed");
            Error::Llm(format!("gemini cluster-questions envelope parse: {e}"))
        })?;

        let raw_text = result
            .candidates
            .first()
            .map(|c| {
                c.content
                    .parts
                    .iter()
                    .map(|p| p.text_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        match serde_json::from_str::<ClusterQuestionsJson>(&raw_text) {
            Ok(parsed) => Ok(parsed.into_domain()),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw = %raw_text,
                    "Cluster-questions inner JSON malformed; treating as empty payload"
                );
                Ok(ClusterQuestionsJson::default().into_domain())
            }
        }
    }
}

/// Wire-shape mirror for the merged `generate` JSON output (non-stream).
///
/// Gemini with `responseSchema` emits exactly this JSON. `assets` is
/// optional so the parser tolerates the model omitting the array on
/// queries with no asset mentions.
#[derive(Deserialize, Default)]
struct MergedResponseJson {
    #[serde(default)]
    message: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
    #[serde(default)]
    suggestions: Vec<String>,
}

#[derive(Deserialize, Default)]
struct AssetJson {
    #[serde(default)]
    symbol: String,
    #[serde(rename = "type", default)]
    asset_type: String,
}

impl AssetJson {
    /// Convert wire representation into a typed `Asset`. Returns
    /// `None` when the type string is off-list (`stock` / `crypto`)
    /// or the symbol is blank — caller drops the entry in both cases.
    fn into_domain(self) -> Option<Asset> {
        if self.symbol.trim().is_empty() {
            return None;
        }
        AssetType::from_raw(&self.asset_type).map(|t| Asset {
            symbol: self.symbol.trim().to_string(),
            asset_type: t,
        })
    }
}

/// Parse the merged `generate` response text into `(answer, assets,
/// suggestions)`. Graceful on failure — returns the raw text as the
/// answer with empty `assets`/`suggestions` so the pipeline never
/// blocks on a malformed model output.
fn parse_merged_response(raw_text: &str) -> (String, Vec<Asset>, Vec<String>) {
    match serde_json::from_str::<MergedResponseJson>(raw_text) {
        Ok(parsed) => {
            let assets = parsed
                .assets
                .into_iter()
                .filter_map(AssetJson::into_domain)
                .collect();
            (parsed.message, assets, parsed.suggestions)
        }
        Err(e) => {
            warn!(
                error = %e,
                raw = %raw_text.chars().take(400).collect::<String>(),
                "Merged generate response returned malformed JSON; using raw text as answer"
            );
            (raw_text.to_string(), Vec::new(), Vec::new())
        }
    }
}

/// Stateful parser for streaming merged-JSON responses.
///
/// Gemini's streaming path delivers partial slices of the
/// `{"message": "...", "assets": [...], "suggestions": [...]}` JSON.
/// This parser:
/// 1. Extracts the `message` string character-by-character so we can
///    emit SSE `delta` events to clients in real time.
/// 2. Buffers the full JSON text so we can `serde_json::from_str` it
///    once the stream finishes, recovering the typed `assets` +
///    `suggestions` arrays for the terminal SSE event.
///
/// State machine:
/// - [`ParseState::Seeking`] — looking for the `"message":"` opener.
///   Anything before it (including `{` and whitespace) is buffered
///   but produces no delta.
/// - [`ParseState::Inside`] — inside the message string. Unescaped
///   characters are appended to the delta buffer; escape sequences
///   (`\n`, `\"`, `\\`, `\/`, `\r`, `\t`) are decoded. Closing `"`
///   transitions to `AfterMessage`.
/// - [`ParseState::AfterClose`] — message closed; we simply buffer bytes
///   until the stream ends, then parse the full accumulated JSON.
///
/// Boundary handling:
/// - A trailing `\` (lone backslash at the end of a chunk) is carried
///   over; the next chunk's first char decides the escape meaning.
/// - Partial UTF-8 sequences at chunk boundaries are buffered in the
///   outer byte stream upstream of this parser (SSE framing already
///   delivers well-formed text), so we operate on `&str` here.
///
/// NOTE: `\uXXXX` unicode escapes are forwarded verbatim into the
/// delta buffer for now. They're rare in English / CJK / European
/// LLM output (Gemini emits native UTF-8 instead). The final
/// `finalize()` call re-parses the whole buffer through
/// `serde_json`, so suggestions + assets still decode these escapes
/// correctly; only the streamed deltas are literal.
struct MergedJsonStreamParser {
    /// Everything we've received so far — fed straight into
    /// `serde_json::from_str` at `finalize()`.
    buffer: String,
    /// Where we are in the state machine.
    state: ParseState,
    /// Set when the previous chunk ended on a lone backslash inside
    /// the message string — the NEXT character decides the escape.
    pending_escape: bool,
    /// Number of bytes of `buffer` we've already scanned. Ensures we
    /// never double-process a byte across `push` calls.
    scan_pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    /// Looking for the `"message":"` opener; bytes buffered but no
    /// delta emitted yet.
    Seeking,
    /// Inside the message string — decoding chars into deltas.
    Inside,
    /// Message string closed; buffering the rest for final parse.
    AfterClose,
}

impl MergedJsonStreamParser {
    fn new() -> Self {
        Self {
            buffer: String::with_capacity(1024),
            state: ParseState::Seeking,
            pending_escape: false,
            scan_pos: 0,
        }
    }

    /// Feed a new chunk of JSON text through the parser. Returns the
    /// decoded delta chars for the `message` field (may be empty).
    /// The second tuple element is currently always `false` but kept
    /// as a future extension slot for terminal signalling.
    fn push(&mut self, text: &str) -> (String, bool) {
        if text.is_empty() {
            return (String::new(), false);
        }
        self.buffer.push_str(text);
        let mut delta = String::new();

        // Scan forward from where we left off. We walk byte-by-byte
        // but only decode when inside the message string; `{`, `"`,
        // `:` and whitespace are all ASCII so byte scanning is safe.
        let bytes = self.buffer.as_bytes();
        while self.scan_pos < bytes.len() {
            match self.state {
                ParseState::Seeking => {
                    // Look for the sentinel `"message":"` (ignoring
                    // inter-token whitespace). Gemini's structured
                    // output always emits the schema's declared
                    // properties in propertyOrdering order; `message`
                    // is first so we only need to find the first
                    // `"message"` key, then consume whitespace and
                    // the colon to reach the opening `"` of its value.
                    if let Some(pos) = find_message_value_start(&self.buffer, self.scan_pos) {
                        self.scan_pos = pos;
                        self.state = ParseState::Inside;
                    } else {
                        // Not enough data yet — leave scan_pos so the
                        // NEXT push re-scans from here. (We could
                        // advance to len-10ish to avoid rework, but
                        // `message` appears once per response so the
                        // overhead is trivial.)
                        break;
                    }
                }
                ParseState::Inside => {
                    // Consume one scalar unit (char or escape) from
                    // `self.scan_pos`. Use a temporary slice so the
                    // borrow checker lets us mutate `self.scan_pos`.
                    let rest = &self.buffer[self.scan_pos..];
                    match consume_message_char(rest, self.pending_escape) {
                        CharStep::Emit(c, advance, pending) => {
                            delta.push(c);
                            self.scan_pos += advance;
                            self.pending_escape = pending;
                        }
                        CharStep::Skip(advance, pending) => {
                            self.scan_pos += advance;
                            self.pending_escape = pending;
                        }
                        CharStep::Closed(advance) => {
                            // Unescaped `"` — end of message string.
                            self.scan_pos += advance;
                            self.state = ParseState::AfterClose;
                            self.pending_escape = false;
                        }
                        CharStep::NeedMore => {
                            // Chunk ends mid-escape / mid-char.
                            // Remember the pending state if relevant.
                            break;
                        }
                    }
                }
                ParseState::AfterClose => {
                    // Fast-forward: we don't need to scan any more
                    // byte-by-byte. Just buffer the rest and finalize
                    // at end of stream.
                    self.scan_pos = bytes.len();
                }
            }
        }

        (delta, false)
    }

    /// End-of-stream: parse the full buffered JSON and return
    /// `(assets, suggestions)`. Never errors — malformed input
    /// degrades to `(vec![], vec![])` with a warning log.
    fn finalize(&mut self) -> (Vec<Asset>, Vec<String>) {
        // Quick path: empty buffer → nothing to parse.
        if self.buffer.trim().is_empty() {
            return (Vec::new(), Vec::new());
        }

        match serde_json::from_str::<MergedResponseJson>(&self.buffer) {
            Ok(parsed) => {
                let assets = parsed
                    .assets
                    .into_iter()
                    .filter_map(AssetJson::into_domain)
                    .collect();
                (assets, parsed.suggestions)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    sample = %self.buffer.chars().take(400).collect::<String>(),
                    "Stream finalize: merged JSON malformed; emitting empty assets + suggestions"
                );
                (Vec::new(), Vec::new())
            }
        }
    }
}

/// Locate the byte offset AFTER the opening `"` of `message`'s value,
/// starting scan from `from`. Returns `None` when the opening pattern
/// isn't fully present yet.
///
/// Looks for the first `"message"` key followed by optional whitespace,
/// a `:`, optional whitespace, and an opening `"`. Tolerant to byte
/// positions pointing mid-string: we scan forward until we hit the
/// `"message"` literal or exhaust the buffer.
fn find_message_value_start(buffer: &str, from: usize) -> Option<usize> {
    // Anchor on the literal `"message"`. Gemini schema with
    // `propertyOrdering: ["message", "assets", "suggestions"]` always
    // emits `message` first — but we don't depend on that ordering
    // either way; we just find the key by name.
    const KEY: &[u8] = b"\"message\"";
    let buf = buffer.as_bytes();
    if from >= buf.len() {
        return None;
    }

    let mut i = from;
    while i + KEY.len() <= buf.len() {
        if &buf[i..i + KEY.len()] == KEY {
            // Found `"message"` — now consume whitespace, expect `:`,
            // more whitespace, then the opening `"`.
            let mut j = i + KEY.len();
            while j < buf.len() && (buf[j] == b' ' || buf[j] == b'\t' || buf[j] == b'\n') {
                j += 1;
            }
            if j >= buf.len() {
                return None;
            }
            if buf[j] != b':' {
                // Not actually the field key — could be `"message"`
                // appearing inside another string literal. Skip past
                // and keep searching.
                i += 1;
                continue;
            }
            j += 1;
            while j < buf.len() && (buf[j] == b' ' || buf[j] == b'\t' || buf[j] == b'\n') {
                j += 1;
            }
            if j >= buf.len() {
                return None;
            }
            if buf[j] != b'"' {
                // Schema violation (e.g. `"message": null`). Treat as
                // no message to emit; downstream finalize will still
                // try to decode assets/suggestions.
                return None;
            }
            return Some(j + 1);
        }
        i += 1;
    }
    None
}

/// Outcome of decoding one scalar unit inside the message string.
enum CharStep {
    /// Emitted character; advance `n` bytes; `pending_escape` is the
    /// new pending-escape state (carried across chunks when `true`).
    Emit(char, usize, bool),
    /// Consumed bytes without emitting. Used when a chunk ends on a
    /// lone `\` — we advance past it and set `pending_escape=true`
    /// so the next chunk's first char completes the escape.
    Skip(usize, bool),
    /// Unescaped `"` — end of the message string. Advance `n` bytes.
    Closed(usize),
    /// Insufficient data (lone trailing `\` or partial char); bail
    /// until the next chunk arrives.
    NeedMore,
}

/// Decode one scalar unit from `rest`, which is the message-string
/// slice starting at the current scan position. `pending_escape` is
/// `true` iff the previous chunk ended on a lone `\`.
fn consume_message_char(rest: &str, pending_escape: bool) -> CharStep {
    if rest.is_empty() {
        return CharStep::NeedMore;
    }

    if pending_escape {
        // Previous chunk ended on `\`; this char completes the escape.
        let mut it = rest.chars();
        let c = match it.next() {
            Some(c) => c,
            None => return CharStep::NeedMore,
        };
        let out = match c {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{0008}',
            'f' => '\u{000C}',
            'u' => {
                // `\uXXXX` — forward verbatim per design note.
                // Emit the backslash + 'u' then advance only 1 byte;
                // the remaining hex digits are emitted by subsequent
                // calls as plain chars. The final `serde_json::from_str`
                // still handles them correctly for assets/suggestions.
                return CharStep::Emit('\\', c.len_utf8(), false);
            }
            other => other, // invalid escape — pass through
        };
        return CharStep::Emit(out, c.len_utf8(), false);
    }

    let b = rest.as_bytes()[0];
    if b == b'\\' {
        // Start of an escape — need the NEXT char to decode.
        if rest.len() == 1 {
            // Lone trailing backslash at end of chunk. Advance past
            // the `\` and signal `pending_escape` so the next push's
            // first char is interpreted as the escape continuation.
            return CharStep::Skip(1, true);
        }
        // Peek at char after `\`.
        let after = &rest[1..];
        let mut it = after.chars();
        let c = match it.next() {
            Some(c) => c,
            None => return CharStep::NeedMore,
        };
        let out = match c {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{0008}',
            'f' => '\u{000C}',
            'u' => {
                // Forward the literal backslash — see above.
                return CharStep::Emit('\\', 1, false);
            }
            other => other,
        };
        return CharStep::Emit(out, 1 + c.len_utf8(), false);
    }

    if b == b'"' {
        return CharStep::Closed(1);
    }

    // Regular UTF-8 char — take exactly one `char`.
    let mut it = rest.chars();
    match it.next() {
        Some(c) => CharStep::Emit(c, c.len_utf8(), false),
        None => CharStep::NeedMore,
    }
}

/// Wire-shape mirror for the cluster-questions response. Gemini emits the
/// JSON we asked for via responseSchema. Locale keys arrive as strings
/// (`"en"`, `"zh-TW"`, ...) which we map to the typed `Locale` enum.
///
/// NOTE: this struct intentionally does NOT use `#[serde(rename_all =
/// "camelCase")]` because the field names here come from OUR prompt /
/// schema, not from Gemini's API envelope. The camelCase trap applies
/// only to Gemini's wrapper types (`GeminiResponse`, `GeminiCandidate`,
/// etc.) which are unchanged.
#[derive(Deserialize, Default)]
struct ClusterQuestionsJson {
    #[serde(default)]
    label: String,
    #[serde(default)]
    questions: HashMap<String, Vec<String>>,
}

impl ClusterQuestionsJson {
    fn into_domain(self) -> ClusterQuestions {
        let mut typed: HashMap<Locale, Vec<String>> = HashMap::new();
        for (tag, qs) in self.questions {
            if let Ok(locale) = tag.parse::<Locale>() {
                typed.insert(locale, qs);
            }
        }
        ClusterQuestions {
            label: self.label,
            questions: typed,
        }
    }
}

// --- Gemini API types ---

/// Structured-output payload returned by the translator. The model is
/// instructed (via `responseMimeType` + `responseSchema`) to emit this
/// shape verbatim, so a plain serde parse is reliable.
#[derive(Deserialize)]
struct TranslatorJson {
    detected_locale: String,
    normalized_query: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    /// Pinned per-request system prompt (e.g. answer-language constraint).
    /// Serialized as `systemInstruction` per the v1beta REST spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    /// Inference tier: STANDARD, FLEX, or PRIORITY. Controls latency/cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    /// Normal text delta. Gemini 3 may also emit a `thoughtSignature`
    /// field on text parts for "thinking" traces — we capture it so
    /// deserialization doesn't fail, but we don't use it.
    Text {
        text: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "thoughtSignature"
        )]
        thought_signature: Option<String>,
    },
    /// Gemini 3 streams the model's decision to call the built-in
    /// `google_search` tool as a `functionCall` part BEFORE the grounded
    /// text arrives. We need to accept and ignore it so the event parses;
    /// the grounded answer text shows up in a later event as a plain
    /// Text part.
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: serde_json::Value,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "thoughtSignature"
        )]
        thought_signature: Option<String>,
    },
    /// Catch-all for any future part type we don't know about. Keeps
    /// the stream parser resilient.
    Other(serde_json::Value),
}

impl GeminiPart {
    /// Borrow the text content if this part is a text delta; otherwise
    /// return an empty str (for `functionCall`, `Other`, etc.).
    fn text_str(&self) -> &str {
        match self {
            GeminiPart::Text { text, .. } => text,
            _ => "",
        }
    }

    /// Convenience for building a user/model text part without having
    /// to spell out `thought_signature: None` at every call site.
    fn text(text: String) -> Self {
        GeminiPart::Text {
            text,
            thought_signature: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    #[serde(skip_serializing_if = "Option::is_none")]
    google_search: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Set to `application/json` to force structured output. Used by the
    /// translator; left `None` for the answer call (incompatible with
    /// `google_search` tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_schema: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    grounding_metadata: Option<GroundingMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroundingMetadata {
    #[serde(default)]
    grounding_chunks: Vec<GroundingChunk>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroundingChunk {
    #[serde(default)]
    web: Option<WebChunk>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebChunk {
    uri: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

/// Per-1M-token pricing rates for a given Gemini model, as `(input, output)`.
///
/// Factored out of `GeminiProvider::estimate_cost` so the streaming path
/// (which assembles `LlmUsage` inside an `async move` that can't hold
/// `&self`) can reuse the same table. Mirrors the pricing in the
/// `estimate_cost` docstring.
fn cost_rates_for_model(model: &str) -> (f64, f64) {
    if model.contains("pro") {
        (2.00_f64, 12.00_f64)
    } else if model.contains("flash-lite") || model.contains("flash_lite") {
        (0.10, 0.40)
    } else {
        // flash + unknown models default to flash pricing
        (0.30, 2.50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_to_wire_empty() {
        let (sys, contents) = GeminiProvider::messages_to_wire(&[]);
        assert!(sys.is_none());
        assert!(contents.is_empty());
    }

    #[test]
    fn messages_to_wire_system_plus_user() {
        let messages = vec![
            Message::system_text("You are a helpful assistant."),
            Message::user_text("Hello"),
        ];
        let (sys, contents) = GeminiProvider::messages_to_wire(&messages);
        assert!(sys.is_some());
        let sys = sys.unwrap();
        assert_eq!(sys.role, Some("system".to_string()));
        assert_eq!(sys.parts.len(), 1);
        assert_eq!(sys.parts[0].text_str(), "You are a helpful assistant.");

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, Some("user".to_string()));
        assert_eq!(contents[0].parts[0].text_str(), "Hello");
    }

    #[test]
    fn messages_to_wire_with_history() {
        let messages = vec![
            Message::system_text("sys"),
            Message::user_text("q1"),
            Message::assistant_text("a1"),
            Message::user_text("q2"),
        ];
        let (sys, contents) = GeminiProvider::messages_to_wire(&messages);
        assert!(sys.is_some());
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0].role, Some("user".to_string()));
        assert_eq!(contents[0].parts[0].text_str(), "q1");
        assert_eq!(contents[1].role, Some("model".to_string()));
        assert_eq!(contents[1].parts[0].text_str(), "a1");
        assert_eq!(contents[2].role, Some("user".to_string()));
        assert_eq!(contents[2].parts[0].text_str(), "q2");
    }

    #[test]
    fn messages_to_wire_no_system() {
        let messages = vec![Message::user_text("just a user message")];
        let (sys, contents) = GeminiProvider::messages_to_wire(&messages);
        assert!(sys.is_none());
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, Some("user".to_string()));
    }

    #[test]
    fn cluster_questions_json_parses_all_locales() {
        let raw = r#"{
            "label": "Staking Basics",
            "questions": {
                "en":    ["What is staking?", "How does staking work?"],
                "zh":    ["什么是质押？", "质押如何工作？"],
                "zh-TW": ["什麼是質押？", "質押如何運作？"],
                "ja":    ["ステーキングとは？", "ステーキングの仕組みは？"],
                "ko":    ["스테이킹이란?", "스테이킹은 어떻게 작동합니까?"],
                "de":    ["Was ist Staking?", "Wie funktioniert Staking?"],
                "fr":    ["Qu'est-ce que le staking?", "Comment fonctionne le staking?"],
                "es":    ["¿Qué es el staking?", "¿Cómo funciona el staking?"]
            }
        }"#;
        let parsed: ClusterQuestionsJson = serde_json::from_str(raw).unwrap();
        let domain = parsed.into_domain();
        assert_eq!(domain.label, "Staking Basics");
        assert_eq!(domain.questions.len(), 8);
        for locale in Locale::ALL {
            let qs = domain
                .questions
                .get(locale)
                .unwrap_or_else(|| panic!("missing locale {locale}"));
            assert!(!qs.is_empty(), "locale {locale} has no questions");
        }
    }

    #[test]
    fn cluster_questions_json_drops_unknown_locales() {
        let raw = r#"{
            "label": "Test",
            "questions": {
                "en": ["q1"],
                "pt": ["q2"]
            }
        }"#;
        let parsed: ClusterQuestionsJson = serde_json::from_str(raw).unwrap();
        let domain = parsed.into_domain();
        assert_eq!(domain.questions.len(), 1);
        assert!(domain.questions.contains_key(&Locale::En));
    }

    #[test]
    fn cluster_questions_json_handles_missing_fields() {
        let raw = r#"{}"#;
        let parsed: ClusterQuestionsJson = serde_json::from_str(raw).unwrap();
        let domain = parsed.into_domain();
        assert_eq!(domain.label, "");
        assert!(domain.questions.is_empty());
    }

    // ---- Usage capture tests ----------------------------------------------

    /// Per-1M-token rates table: confirm the pro/flash/flash-lite mapping
    /// stays stable. If anyone edits the pricing, this test forces an
    /// explicit update instead of a silent drift.
    #[test]
    fn cost_rates_for_known_model_families() {
        assert_eq!(cost_rates_for_model("gemini-3.1-pro"), (2.00, 12.00));
        assert_eq!(cost_rates_for_model("gemini-2.5-flash"), (0.30, 2.50));
        assert_eq!(cost_rates_for_model("gemini-2.5-flash-lite"), (0.10, 0.40));
        // Unknown models default to flash pricing.
        assert_eq!(cost_rates_for_model("unknown-model"), (0.30, 2.50));
    }

    /// `GeminiResponse` must deserialize the `usageMetadata` block so
    /// translate/suggest/generate can build their `LlmUsage` from it.
    /// This test locks the camelCase mapping that previously caused
    /// silent empty `Option`s when missed.
    #[test]
    fn gemini_response_parses_usage_metadata() {
        let raw = r#"{
            "candidates": [{
                "content": { "parts": [{ "text": "hi" }] }
            }],
            "usageMetadata": {
                "promptTokenCount": 150,
                "candidatesTokenCount": 50,
                "totalTokenCount": 200
            }
        }"#;
        let parsed: GeminiResponse = serde_json::from_str(raw).unwrap();
        let usage = parsed.usage_metadata.expect("usageMetadata must parse");
        assert_eq!(usage.prompt_token_count, Some(150));
        assert_eq!(usage.candidates_token_count, Some(50));
        assert_eq!(usage.total_token_count, Some(200));
    }

    /// When the server omits `usageMetadata` (not all endpoints report
    /// it), the parse must still succeed and yield `None` instead of
    /// failing hard.
    #[test]
    fn gemini_response_handles_missing_usage_metadata() {
        let raw = r#"{
            "candidates": [{
                "content": { "parts": [{ "text": "hi" }] }
            }]
        }"#;
        let parsed: GeminiResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.usage_metadata.is_none());
    }

    /// Gemini sometimes returns `promptTokenCount` + `candidatesTokenCount`
    /// but omits `totalTokenCount`. Previously we defaulted the total to
    /// `0`, which under-reported usage and produced an obviously
    /// inconsistent `LlmUsage` (prompt+completion non-zero, total=0).
    /// Fall back to `prompt.saturating_add(completion)` so downstream
    /// usage accounting stays coherent even when the provider skips
    /// the total field.
    #[test]
    fn gemini_response_fills_total_tokens_when_omitted() {
        let raw = r#"{
            "candidates": [{
                "content": { "parts": [{ "text": "hi" }] }
            }],
            "usageMetadata": {
                "promptTokenCount": 120,
                "candidatesTokenCount": 30
            }
        }"#;
        let parsed: GeminiResponse = serde_json::from_str(raw).unwrap();
        let u = parsed.usage_metadata.expect("usageMetadata must parse");
        assert_eq!(u.prompt_token_count, Some(120));
        assert_eq!(u.candidates_token_count, Some(30));
        assert!(u.total_token_count.is_none());

        // Simulate the production fallback — every call site in this
        // file now mirrors this expression.
        let prompt = u.prompt_token_count.unwrap_or(0);
        let completion = u.candidates_token_count.unwrap_or(0);
        let total = u
            .total_token_count
            .unwrap_or(prompt.saturating_add(completion));
        assert_eq!(total, 150, "total must fall back to prompt+completion");
    }

    /// The cost math must factor in the service-tier multiplier AND
    /// the per-model base pricing. Standard flash: 150 in + 50 out =
    /// (150/1M * 0.30) + (50/1M * 2.50) = 0.000170. Flex cuts that to
    /// 0.000085 (50% multiplier), Priority raises it to 0.0002975
    /// (1.75x multiplier).
    #[test]
    fn estimate_cost_reflects_service_tier_and_model() {
        use kenjaku_core::config::{LlmConfig, ServiceTier};

        fn provider_with(model: &str, tier: ServiceTier) -> GeminiProvider {
            GeminiProvider::new(LlmConfig {
                provider: "gemini".into(),
                model: model.into(),
                api_key: "test".into(),
                max_tokens: 1024,
                temperature: 0.0,
                service_tier: tier,
                base_url: "http://localhost".into(),
            })
        }

        let flash_standard = provider_with("gemini-2.5-flash", ServiceTier::Standard);
        let cost = flash_standard.estimate_cost(150, 50).unwrap();
        assert!(
            (cost - 0.000170).abs() < 1e-9,
            "flash+standard cost math changed: got {cost}"
        );

        let flash_flex = provider_with("gemini-2.5-flash", ServiceTier::Flex);
        let flex_cost = flash_flex.estimate_cost(150, 50).unwrap();
        assert!(
            (flex_cost - 0.000085).abs() < 1e-9,
            "flex multiplier should halve cost: got {flex_cost}"
        );

        let pro_priority = provider_with("gemini-3.1-pro", ServiceTier::Priority);
        // pro pricing: 150/1M * 2.00 + 50/1M * 12.00 = 0.00090
        // priority 1.75x = 0.001575
        let pro_cost = pro_priority.estimate_cost(150, 50).unwrap();
        assert!(
            (pro_cost - 0.001575).abs() < 1e-9,
            "pro+priority cost math changed: got {pro_cost}"
        );
    }

    // ---- Merged JSON parsing (non-stream path) ----------------------------

    /// Happy-path parse: typical response with message + assets + suggestions
    /// all populated. Ensures the serde mapping on `MergedResponseJson`
    /// matches the prompt's `{message, assets, suggestions}` contract.
    #[test]
    fn parse_merged_response_happy_path() {
        let raw = r#"{
            "message": "Apple rose 2% today.",
            "assets": [
                {"symbol": "AAPL", "type": "stock"},
                {"symbol": "BTC",  "type": "crypto"}
            ],
            "suggestions": [
                "What is Apple's revenue?",
                "How does AAPL compare to MSFT?",
                "Why did Apple rise today?"
            ]
        }"#;
        let (answer, assets, suggestions) = parse_merged_response(raw);
        assert_eq!(answer, "Apple rose 2% today.");
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].symbol, "AAPL");
        assert_eq!(assets[0].asset_type, AssetType::Stock);
        assert_eq!(assets[1].symbol, "BTC");
        assert_eq!(assets[1].asset_type, AssetType::Crypto);
        assert_eq!(suggestions.len(), 3);
    }

    /// Missing `assets` field is legal under the schema (only `message`
    /// and `suggestions` are `required`). Parser must tolerate it and
    /// yield an empty assets vec.
    #[test]
    fn parse_merged_response_missing_assets_field() {
        let raw = r#"{
            "message": "Hello world.",
            "suggestions": ["Q1", "Q2", "Q3"]
        }"#;
        let (answer, assets, suggestions) = parse_merged_response(raw);
        assert_eq!(answer, "Hello world.");
        assert!(assets.is_empty());
        assert_eq!(suggestions.len(), 3);
    }

    /// Invalid asset types (e.g. `"bond"`) must be filtered out;
    /// valid entries in the same array must survive.
    #[test]
    fn parse_merged_response_filters_invalid_asset_types() {
        let raw = r#"{
            "message": "Mixed assets.",
            "assets": [
                {"symbol": "AAPL", "type": "stock"},
                {"symbol": "US10Y", "type": "bond"},
                {"symbol": "ETH",  "type": "crypto"}
            ],
            "suggestions": []
        }"#;
        let (_, assets, _) = parse_merged_response(raw);
        assert_eq!(assets.len(), 2, "bond must be filtered out");
        assert_eq!(assets[0].symbol, "AAPL");
        assert_eq!(assets[1].symbol, "ETH");
    }

    /// Malformed JSON input falls back to `(raw_text, [], [])` so the
    /// pipeline still produces SOMETHING — graceful degradation per
    /// the design contract.
    #[test]
    fn parse_merged_response_falls_back_on_malformed_json() {
        let raw = "not json at all, just a plain string";
        let (answer, assets, suggestions) = parse_merged_response(raw);
        assert_eq!(answer, raw);
        assert!(assets.is_empty());
        assert!(suggestions.is_empty());
    }

    /// Empty symbol → drop. Protects against Gemini emitting
    /// `{"symbol": "", "type": "stock"}` from a degenerate answer.
    #[test]
    fn parse_merged_response_drops_empty_symbol() {
        let raw = r#"{
            "message": "ok",
            "assets": [{"symbol": "", "type": "stock"}],
            "suggestions": []
        }"#;
        let (_, assets, _) = parse_merged_response(raw);
        assert!(assets.is_empty());
    }

    // ---- MergedJsonStreamParser tests -------------------------------------

    /// Sanity: a full single-chunk JSON produces the expected delta,
    /// assets, and suggestions on finalize.
    #[test]
    fn stream_parser_happy_path_one_chunk() {
        let raw = r#"{"message":"Hello world","assets":[{"symbol":"AAPL","type":"stock"}],"suggestions":["A","B","C"]}"#;
        let mut parser = MergedJsonStreamParser::new();
        let (delta, _) = parser.push(raw);
        assert_eq!(delta, "Hello world");
        let (assets, suggestions) = parser.finalize();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].symbol, "AAPL");
        assert_eq!(suggestions, vec!["A", "B", "C"]);
    }

    /// Chunk boundary lands in the middle of the `message` value.
    /// Both halves concatenated must equal the decoded string.
    #[test]
    fn stream_parser_split_mid_message() {
        let mut parser = MergedJsonStreamParser::new();
        let (d1, _) = parser.push(r#"{"message":"Hello "#);
        let (d2, _) = parser.push(r#"world","assets":[],"suggestions":["s1","s2","s3"]}"#);
        assert_eq!(format!("{d1}{d2}"), "Hello world");
        let (_, suggestions) = parser.finalize();
        assert_eq!(suggestions.len(), 3);
    }

    /// Chunk ends on a lone `\` that begins an escape; next chunk
    /// completes it as `\n`.
    #[test]
    fn stream_parser_split_mid_escape() {
        let mut parser = MergedJsonStreamParser::new();
        let (d1, _) = parser.push(r#"{"message":"line1\"#);
        let (d2, _) = parser.push(r#"nline2","assets":[],"suggestions":[]}"#);
        assert_eq!(format!("{d1}{d2}"), "line1\nline2");
    }

    /// Split inside a multi-byte UTF-8 character. The outer SSE framing
    /// guarantees well-formed `&str` at this level, so we simulate the
    /// realistic case where the CJK char arrives inside a single push.
    /// This test also guards against future regressions if someone
    /// converts the parser to operate on raw bytes.
    #[test]
    fn stream_parser_handles_cjk_in_message() {
        let mut parser = MergedJsonStreamParser::new();
        let (delta, _) = parser.push(r#"{"message":"測試","assets":[],"suggestions":[]}"#);
        assert_eq!(delta, "測試");
    }

    /// Escape coverage: `\n`, `\t`, `\"`, `\\`, `\/` must all decode
    /// to their canonical single chars in the delta output.
    #[test]
    fn stream_parser_decodes_escapes() {
        let raw = r#"{"message":"a\nb\tc\"d\\e\/f","assets":[],"suggestions":[]}"#;
        let mut parser = MergedJsonStreamParser::new();
        let (delta, _) = parser.push(raw);
        assert_eq!(delta, "a\nb\tc\"d\\e/f");
    }

    /// Empty arrays: `assets` absent is tolerated; empty
    /// `suggestions` array parses to empty Vec.
    #[test]
    fn stream_parser_empty_arrays() {
        let raw = r#"{"message":"ok","assets":[],"suggestions":[]}"#;
        let mut parser = MergedJsonStreamParser::new();
        parser.push(raw);
        let (assets, suggestions) = parser.finalize();
        assert!(assets.is_empty());
        assert!(suggestions.is_empty());
    }

    /// Malformed / truncated stream: if the stream ends before the
    /// closing brace, finalize degrades to `(vec![], vec![])` rather
    /// than panic. The message delta emitted so far is still returned
    /// by the preceding `push` calls.
    #[test]
    fn stream_parser_truncated_stream_degrades() {
        let mut parser = MergedJsonStreamParser::new();
        // Partial — no closing `"` on message, no closing `}`.
        let (delta, _) = parser.push(r#"{"message":"hello"#);
        assert_eq!(delta, "hello");
        let (assets, suggestions) = parser.finalize();
        assert!(assets.is_empty());
        assert!(suggestions.is_empty());
    }

    /// Assets with invalid `type` (e.g. `"bond"`) must drop from the
    /// finalized output but valid ones survive.
    #[test]
    fn stream_parser_filters_invalid_asset_types() {
        let raw = r#"{"message":"x","assets":[{"symbol":"AAPL","type":"stock"},{"symbol":"US10Y","type":"bond"}],"suggestions":[]}"#;
        let mut parser = MergedJsonStreamParser::new();
        parser.push(raw);
        let (assets, _) = parser.finalize();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].symbol, "AAPL");
    }

    /// The pre-`message` preamble (`{"message":`) is not emitted as
    /// delta — only the value's chars are. Guards against a regression
    /// where the parser forgets to skip the JSON structural tokens.
    #[test]
    fn stream_parser_does_not_emit_preamble() {
        let mut parser = MergedJsonStreamParser::new();
        let (delta, _) = parser.push(r#"{"message":""#);
        assert_eq!(delta, "", "preamble bytes must produce no delta");
        let (delta2, _) = parser.push(r#"hi","assets":[],"suggestions":[]}"#);
        assert_eq!(delta2, "hi");
    }

    /// Stream that splits one byte at a time. Stress-tests boundary
    /// handling — no partial escape should leak into the delta, and
    /// the concatenated delta must equal the full decoded message.
    #[test]
    fn stream_parser_byte_by_byte_fuzz() {
        let raw = r#"{"message":"hello \"world\"","assets":[],"suggestions":["q"]}"#;
        let mut parser = MergedJsonStreamParser::new();
        let mut all_delta = String::new();
        for b in raw.chars() {
            let (d, _) = parser.push(&b.to_string());
            all_delta.push_str(&d);
        }
        assert_eq!(all_delta, "hello \"world\"");
        let (_, suggestions) = parser.finalize();
        assert_eq!(suggestions, vec!["q"]);
    }
}
