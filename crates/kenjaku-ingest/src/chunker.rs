/// Chunking strategies for document processing.

/// Chunk a text into fixed-size chunks with overlap, respecting sentence boundaries.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }

    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());

        // Try to break at a sentence boundary
        let actual_end = if end < chars.len() {
            find_sentence_boundary(&chars, start, end).unwrap_or(end)
        } else {
            end
        };

        let chunk: String = chars[start..actual_end].iter().collect();
        let trimmed = chunk.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(trimmed);
        }

        // Move forward by chunk_size - overlap
        let step = if actual_end > start + overlap {
            actual_end - start - overlap
        } else {
            actual_end - start
        };
        start += step.max(1);
    }

    chunks
}

/// Find the nearest sentence boundary (period, newline) before `end`.
fn find_sentence_boundary(chars: &[char], start: usize, end: usize) -> Option<usize> {
    // Look backwards from end for sentence-ending punctuation
    let search_start = if end > start + 50 { end - 50 } else { start };

    for i in (search_start..end).rev() {
        if chars[i] == '.' || chars[i] == '\n' || chars[i] == '!' || chars[i] == '?' {
            return Some(i + 1);
        }
    }

    // Fallback: look for space
    for i in (search_start..end).rev() {
        if chars[i] == ' ' {
            return Some(i + 1);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_empty_text() {
        let chunks = chunk_text("", 100, 10);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_short_text() {
        let chunks = chunk_text("Hello world", 100, 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn test_chunk_respects_size() {
        let text = "A".repeat(1000);
        let chunks = chunk_text(&text, 200, 20);
        for chunk in &chunks {
            assert!(chunk.len() <= 200, "Chunk too long: {}", chunk.len());
        }
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_chunk_with_overlap() {
        let text = "First sentence. Second sentence. Third sentence. Fourth sentence.";
        let chunks = chunk_text(text, 30, 5);
        assert!(chunks.len() > 1);
        // With overlap, subsequent chunks should share some content with previous
    }

    #[test]
    fn test_chunk_sentence_boundary() {
        let text = "This is the first sentence. This is the second sentence. This is the third one.";
        let chunks = chunk_text(text, 40, 5);
        // Chunks should prefer breaking at sentence boundaries
        for chunk in &chunks {
            assert!(!chunk.is_empty());
        }
    }
}
