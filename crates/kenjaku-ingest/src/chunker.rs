//! Token-aware chunking using tiktoken (cl100k_base — shared by text-embedding-3-*).
//!
//! Best practice for RAG with OpenAI `text-embedding-3-small`:
//! - 500-800 tokens per chunk (balanced precision/context)
//! - 10-20% overlap (preserves continuity across chunks)
//! - Respect sentence boundaries when possible
//!
//! See Anthropic's Contextual Retrieval guide and OpenAI's cookbook.

use tiktoken_rs::{CoreBPE, cl100k_base};

/// Chunk text into token-bounded segments with overlap, respecting sentence boundaries.
///
/// - `chunk_size` and `overlap` are in **tokens**, not characters.
/// - Uses the `cl100k_base` tokenizer (same as text-embedding-3-small/large).
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![];
    }

    let bpe = match cl100k_base() {
        Ok(bpe) => bpe,
        Err(_) => {
            // Fallback to character-based chunking if tokenizer init fails.
            return chunk_by_chars(text, chunk_size * 4, overlap * 4);
        }
    };

    let tokens = bpe.encode_with_special_tokens(text);
    if tokens.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < tokens.len() {
        let end = (start + chunk_size).min(tokens.len());
        let slice = &tokens[start..end];

        let decoded = match bpe.decode(slice.to_vec()) {
            Ok(s) => s,
            Err(_) => {
                start += step;
                continue;
            }
        };

        // Prefer to end at a sentence boundary within the last ~15% of the chunk.
        let trimmed = adjust_to_sentence_boundary(&decoded, end < tokens.len(), &bpe);
        let text_out = trimmed.trim().to_string();

        if !text_out.is_empty() {
            chunks.push(text_out);
        }

        if end >= tokens.len() {
            break;
        }
        start += step;
    }

    chunks
}

/// Try to trim the decoded chunk at the last sentence boundary, if it's within
/// the last 15% of the text. This avoids cutting mid-sentence.
fn adjust_to_sentence_boundary(text: &str, has_more: bool, _bpe: &CoreBPE) -> String {
    if !has_more {
        return text.to_string();
    }

    let min_keep = (text.len() as f32 * 0.85) as usize;
    let chars: Vec<char> = text.chars().collect();

    for i in (min_keep..chars.len()).rev() {
        let c = chars[i];
        if c == '.' || c == '\n' || c == '!' || c == '?' {
            let end = i + 1;
            let slice: String = chars[..end].iter().collect();
            return slice;
        }
    }

    text.to_string()
}

/// Character-based fallback chunker (used if tokenizer init fails).
fn chunk_by_chars(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let slice: String = chars[start..end].iter().collect();
        let trimmed = slice.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(trimmed);
        }
        if end >= chars.len() {
            break;
        }
        start += step;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_empty_text() {
        assert!(chunk_text("", 100, 10).is_empty());
        assert!(chunk_text("   \n  ", 100, 10).is_empty());
    }

    #[test]
    fn test_chunk_short_text() {
        let chunks = chunk_text("Hello world", 100, 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn test_chunk_respects_token_budget() {
        // Build a long text that should definitely exceed 50 tokens.
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(100);
        let chunks = chunk_text(&text, 50, 5);
        assert!(
            chunks.len() > 1,
            "expected multiple chunks, got {}",
            chunks.len()
        );

        // Verify each chunk is within the budget (give small slack for decoding).
        let bpe = cl100k_base().unwrap();
        for chunk in &chunks {
            let tokens = bpe.encode_with_special_tokens(chunk);
            assert!(
                tokens.len() <= 55,
                "chunk has {} tokens, expected <= 55",
                tokens.len()
            );
        }
    }

    #[test]
    fn test_chunk_prefers_sentence_boundaries() {
        let text = "First sentence here. Second sentence there. Third sentence everywhere. Fourth sentence nowhere.".repeat(20);
        let chunks = chunk_text(&text, 30, 3);
        assert!(chunks.len() > 1);
        // Most chunks (except maybe last) should end with sentence punctuation
        // when there's room.
        let ending_with_period = chunks
            .iter()
            .filter(|c| c.ends_with('.') || c.ends_with('!') || c.ends_with('?'))
            .count();
        assert!(
            ending_with_period >= chunks.len() / 2,
            "Expected most chunks to end at sentence boundaries"
        );
    }
}
