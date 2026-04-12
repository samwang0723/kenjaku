use kenjaku_core::types::search::{GroundingInfo, RetrievalMethod, RetrievedChunk};
use kenjaku_core::types::tool::{ToolOutput, ToolOutputMap};

/// Merge tool outputs into a flat list of `RetrievedChunk`s for LLM consumption.
///
/// - `ToolOutput::Chunks` pass through as-is.
/// - `ToolOutput::WebHits` are converted to synthetic `RetrievedChunk`s with
///   `RetrievalMethod::Web`, mirroring the old `fetch_web_chunks` logic.
/// - `ToolOutput::Structured` and `ToolOutput::Empty` are skipped.
pub fn merge_tool_outputs(outputs: &ToolOutputMap) -> Vec<RetrievedChunk> {
    let mut chunks = Vec::new();

    for (_, output) in outputs.iter() {
        match output {
            ToolOutput::Chunks {
                chunks: tool_chunks,
                ..
            } => {
                chunks.extend(tool_chunks.iter().cloned());
            }
            ToolOutput::WebHits { hits, .. } => {
                for (i, hit) in hits.iter().enumerate() {
                    let snippet = hit.snippet.clone().unwrap_or_default();
                    chunks.push(RetrievedChunk {
                        doc_id: format!("web-{i}"),
                        chunk_id: format!("web-{i}"),
                        title: hit.title.clone(),
                        original_content: snippet.clone(),
                        contextualized_content: snippet,
                        source_url: Some(hit.url.clone()),
                        score: 0.0,
                        retrieval_method: RetrievalMethod::Web,
                    });
                }
            }
            ToolOutput::Structured { .. } | ToolOutput::Empty => {
                // Structured facts and empty outputs don't contribute chunks.
            }
        }
    }

    chunks
}

/// Build `GroundingInfo` from tool outputs. Checks whether any web
/// tool contributed results and captures the provider name.
pub fn grounding_from_outputs(outputs: &ToolOutputMap) -> GroundingInfo {
    let mut info = GroundingInfo::default();
    for (_, output) in outputs.iter() {
        if let ToolOutput::WebHits { hits, provider } = output
            && !hits.is_empty()
        {
            info.web_search_used = true;
            info.web_search_provider = Some(provider.clone());
            info.web_search_count = hits.len();
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::search::LlmSource;
    use kenjaku_core::types::tool::ToolId;

    fn make_chunk(id: &str) -> RetrievedChunk {
        RetrievedChunk {
            doc_id: id.to_string(),
            chunk_id: id.to_string(),
            title: format!("Title {id}"),
            original_content: "content".to_string(),
            contextualized_content: "content".to_string(),
            source_url: Some(format!("https://example.com/{id}")),
            score: 0.9,
            retrieval_method: RetrievalMethod::Vector,
        }
    }

    fn make_web_hit(title: &str, url: &str) -> LlmSource {
        LlmSource {
            title: title.to_string(),
            url: url.to_string(),
            snippet: Some("snippet text".to_string()),
        }
    }

    #[test]
    fn merge_tool_outputs_preserves_chunks() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(
            ToolId("rag".into()),
            ToolOutput::Chunks {
                chunks: vec![make_chunk("c1"), make_chunk("c2")],
                provider: "rag".into(),
            },
        );
        let merged = merge_tool_outputs(&outputs);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].retrieval_method == RetrievalMethod::Vector);
    }

    #[test]
    fn merge_tool_outputs_converts_web_hits() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(
            ToolId("brave".into()),
            ToolOutput::WebHits {
                hits: vec![
                    make_web_hit("Result 1", "https://a.com"),
                    make_web_hit("Result 2", "https://b.com"),
                ],
                provider: "brave".into(),
            },
        );
        let merged = merge_tool_outputs(&outputs);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].retrieval_method, RetrievalMethod::Web);
    }

    #[test]
    fn merge_tool_outputs_mixed_chunks_and_web() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(
            ToolId("rag".into()),
            ToolOutput::Chunks {
                chunks: vec![make_chunk("c1")],
                provider: "rag".into(),
            },
        );
        outputs.insert(
            ToolId("brave".into()),
            ToolOutput::WebHits {
                hits: vec![make_web_hit("Web", "https://web.com")],
                provider: "brave".into(),
            },
        );
        let merged = merge_tool_outputs(&outputs);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_tool_outputs_ignores_empty_and_structured() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(ToolId("e".into()), ToolOutput::Empty);
        outputs.insert(
            ToolId("s".into()),
            ToolOutput::Structured {
                facts: serde_json::json!({"key": "val"}),
                provider: "test".into(),
            },
        );
        let merged = merge_tool_outputs(&outputs);
        assert!(merged.is_empty());
    }

    #[test]
    fn grounding_from_outputs_detects_web() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(
            ToolId("rag".into()),
            ToolOutput::Chunks {
                chunks: vec![],
                provider: "rag".into(),
            },
        );
        outputs.insert(
            ToolId("brave".into()),
            ToolOutput::WebHits {
                hits: vec![make_web_hit("R", "https://r.com")],
                provider: "brave".into(),
            },
        );
        let info = grounding_from_outputs(&outputs);
        assert!(info.web_search_used);
        assert_eq!(info.web_search_provider, Some("brave".to_string()));
        assert_eq!(info.web_search_count, 1);
    }

    #[test]
    fn grounding_from_outputs_empty_when_no_web() {
        let mut outputs = ToolOutputMap::new();
        outputs.insert(
            ToolId("rag".into()),
            ToolOutput::Chunks {
                chunks: vec![make_chunk("c1")],
                provider: "rag".into(),
            },
        );
        let info = grounding_from_outputs(&outputs);
        assert!(!info.web_search_used);
        assert!(info.web_search_provider.is_none());
        assert_eq!(info.web_search_count, 0);
    }
}
