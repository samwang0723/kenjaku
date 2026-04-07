use std::io::Read;
use std::path::{Path, PathBuf};

use kenjaku_core::types::search::DocumentType;

const ALLOWED_EXTENSIONS: &[&str] = &["md", "markdown", "txt", "html", "htm"];

/// Read file content using `std::io::Read` after path validation.
/// Canonicalizes path, checks it is a regular file, validates the extension
/// against `ALLOWED_EXTENSIONS`, then reads via buffered I/O.
fn read_validated_file(path: &Path) -> anyhow::Result<(String, String)> {
    let canonical = path
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to resolve path {}: {e}", path.display()))?;

    if !canonical.is_file() {
        return Err(anyhow::anyhow!(
            "Path is not a regular file: {}",
            canonical.display()
        ));
    }

    let extension = canonical
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(anyhow::anyhow!("File type not in allowlist: {extension}"));
    }

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
    let mut file = std::fs::File::open(&canonical)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    Ok((content, extension))
}

/// Parse a document file into plain text based on its extension.
/// The path is canonicalized, validated against an extension allowlist,
/// and read through `read_validated_file` before any processing.
pub fn parse_file(path: &Path) -> anyhow::Result<(String, DocumentType)> {
    let (content, extension) = read_validated_file(path)?;

    match extension.as_str() {
        "md" | "markdown" => {
            let text = parse_markdown(&content);
            Ok((text, DocumentType::Markdown))
        }
        "txt" => Ok((content, DocumentType::PlainText)),
        "html" | "htm" => {
            let text = crate::crawler::extract_text_from_html(&content);
            Ok((text, DocumentType::Html))
        }
        _ => Err(anyhow::anyhow!("Unsupported file type: {extension}")),
    }
}

/// Convert markdown to plain text.
pub fn parse_markdown(content: &str) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};

    let parser = Parser::new(content);
    let mut text = String::new();

    for event in parser {
        match event {
            Event::Text(t) => text.push_str(&t),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::End(TagEnd::Paragraph) => text.push_str("\n\n"),
            Event::End(TagEnd::Heading(_)) => text.push_str("\n\n"),
            Event::Code(code) => text.push_str(&code),
            _ => {}
        }
    }

    text
}

/// Extract the title from a document.
///
/// Tries (in order): markdown `# H1`, markdown `## H2`, a leading bold line
/// (`**Title**`), and finally the filename stem prettified from slug form.
pub fn extract_title(content: &str, path: &Path) -> String {
    for line in content.lines().take(40) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return rest.trim().to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            return rest.trim().to_string();
        }
        if let Some(rest) = trimmed
            .strip_prefix("**")
            .and_then(|s| s.strip_suffix("**"))
            && !rest.is_empty()
            && rest.len() <= 120
        {
            return rest.trim().to_string();
        }
    }

    // Fallback to filename stem, prettified so "260617-our-company" becomes
    // "Our Company" instead of a slug leaking into search results.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    prettify_slug_for_title(stem)
}

/// Local slug prettifier for the ingest crate (mirrors
/// `kenjaku_service::quality::prettify_title` but without the cross-crate
/// dep — ingest does not depend on the service layer).
fn prettify_slug_for_title(stem: &str) -> String {
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        return "Untitled".to_string();
    }

    // Strip a leading all-digit prefix ("260617-foo" → "foo").
    let stripped = match trimmed.find(['-', '_']) {
        Some(idx) if idx > 0 && trimmed[..idx].chars().all(|c| c.is_ascii_digit()) => {
            &trimmed[idx + 1..]
        }
        _ => trimmed,
    };

    let words: Vec<String> = stripped
        .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect();

    if words.is_empty() {
        "Untitled".to_string()
    } else {
        words.join(" ")
    }
}

/// Validate and canonicalize a directory path for traversal.
fn validate_directory(dir: &Path) -> anyhow::Result<PathBuf> {
    let canonical = dir
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to resolve directory {}: {e}", dir.display()))?;

    if !canonical.is_dir() {
        return Err(anyhow::anyhow!(
            "Path is not a directory: {}",
            canonical.display()
        ));
    }

    Ok(canonical)
}

/// Discover all supported files in a directory recursively.
/// The directory is canonicalized first, and all discovered file paths
/// are verified to remain under the canonical root.
pub fn discover_files(dir: &Path) -> Vec<PathBuf> {
    let canonical_root = match validate_directory(dir) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    discover_files_inner(&canonical_root, &canonical_root)
}

fn discover_files_inner(dir: &Path, root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let supported_extensions = ["md", "markdown", "txt", "html", "htm"];

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();

            // Verify path stays within the root directory
            if let Ok(canonical) = path.canonicalize() {
                if !canonical.starts_with(root) {
                    continue; // Skip symlinks escaping root
                }

                if canonical.is_dir() {
                    files.extend(discover_files_inner(&canonical, root));
                } else if let Some(ext) = canonical.extension().and_then(|e| e.to_str())
                    && supported_extensions.contains(&ext.to_lowercase().as_str())
                {
                    files.push(canonical);
                }
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_markdown() {
        let md = "# Title\n\nThis is a **paragraph**.\n\n- Item 1\n- Item 2\n";
        let text = parse_markdown(md);
        assert!(text.contains("Title"));
        assert!(text.contains("paragraph"));
        assert!(text.contains("Item 1"));
    }

    #[test]
    fn test_extract_title_from_heading() {
        let content = "# My Document\n\nSome content here.";
        let path = Path::new("/tmp/test.md");
        let title = extract_title(content, path);
        assert_eq!(title, "My Document");
    }

    #[test]
    fn test_extract_title_fallback_to_filename() {
        let content = "No heading here, just text.";
        let path = Path::new("/tmp/my-document.md");
        let title = extract_title(content, path);
        assert_eq!(title, "My Document");
    }

    #[test]
    fn test_extract_title_strips_numeric_prefix() {
        let content = "no heading";
        let path = Path::new("/tmp/260617-our-company.md");
        let title = extract_title(content, path);
        assert_eq!(title, "Our Company");
    }

    #[test]
    fn test_extract_title_from_h2_fallback() {
        let content = "Intro paragraph\n\n## Section Title\n\nmore";
        let path = Path::new("/tmp/x.md");
        assert_eq!(extract_title(content, path), "Section Title");
    }

    #[test]
    fn test_discover_files() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("test.md");
        let txt_path = dir.path().join("test.txt");
        let jpg_path = dir.path().join("image.jpg");

        std::fs::File::create(&md_path)
            .unwrap()
            .write_all(b"# Test")
            .unwrap();
        std::fs::File::create(&txt_path)
            .unwrap()
            .write_all(b"text")
            .unwrap();
        std::fs::File::create(&jpg_path)
            .unwrap()
            .write_all(b"")
            .unwrap();

        let files = discover_files(dir.path());
        assert_eq!(files.len(), 2); // md + txt, not jpg
    }

    #[test]
    fn test_parse_txt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "Hello, world!").unwrap();

        let (text, doc_type) = parse_file(&path).unwrap();
        assert_eq!(text, "Hello, world!");
        assert_eq!(doc_type, DocumentType::PlainText);
    }

    #[test]
    fn test_safe_read_rejects_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.env");
        std::fs::write(&path, "SECRET=abc").unwrap();

        let result = read_validated_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("allowlist"));
    }
}
