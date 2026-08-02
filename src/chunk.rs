#[derive(Debug)]
pub struct Chunk {
    pub start: usize,
    pub text: String,
}

pub fn chunk_text(text: &str, max_chars: usize, overlap_chars: usize) -> Vec<Chunk> {
    assert!(max_chars > 0, "chunk size must be positive");
    assert!(
        overlap_chars < max_chars,
        "overlap must be smaller than chunk size"
    );
    let len = text.len();
    if len <= max_chars {
        return vec![Chunk {
            start: 0,
            text: text.to_string(),
        }];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < len {
        let mut end = (start + max_chars).min(len);
        end = char_boundary_before(text, end);
        if end < len {
            if let Some(rel) = text[start..end].rfind(char::is_whitespace) {
                if rel > 0 {
                    end = start + rel;
                }
            }
        }
        out.push(Chunk {
            start,
            text: text[start..end].to_string(),
        });
        if end >= len {
            break;
        }
        let next = end.saturating_sub(overlap_chars);
        let next = char_boundary_before(text, next);
        // progress is guaranteed: either next > start (overlap) or we jump to
        // end (no overlap) — never rewind to or before the previous start.
        start = if next > start { next } else { end };
    }
    out
}

fn char_boundary_before(text: &str, mut idx: usize) -> usize {
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_a_single_chunk() {
        let chunks = chunk_text("hello world", 50, 5);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[0].text, "hello world");
    }

    #[test]
    fn chunks_cover_the_text_without_gaps() {
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let chunks = chunk_text(text, 15, 4);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start, 0);
        let mut covered_until = 0;
        for c in &chunks {
            assert!(
                c.start + c.text.len() >= covered_until,
                "gap in coverage at byte {}",
                c.start
            );
            assert!(
                text[c.start..].starts_with(&c.text[..c.text.len().min(3)]),
                "chunk text must align with original at byte {}",
                c.start
            );
            covered_until = c.start + c.text.len();
        }
        let last = chunks.last().unwrap();
        assert_eq!(
            last.start + last.text.len(),
            text.len(),
            "last chunk must reach the end"
        );
    }

    #[test]
    fn boundary_token_appears_in_both_adjacent_chunks() {
        let text = "alpha beta gamma";
        let chunks = chunk_text(text, 11, 5);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunks, got {:?}",
            chunks
        );
        assert!(
            chunks[0].text.contains("beta"),
            "left chunk must keep the boundary token: {:?}",
            chunks[0].text
        );
        assert!(
            chunks[1].text.contains("beta"),
            "overlap must re-scan the boundary token: {:?}",
            chunks[1].text
        );
        assert!(chunks[0].text.contains("alpha"));
        assert!(chunks[1].text.contains("gamma"));
    }

    #[test]
    fn multibyte_text_never_splits_mid_char() {
        let text = "héllo wörld süß ✨✨✨🎉🎉🎉 this is a long enough sentence to force splitting across boundaries";
        for c in chunk_text(text, 12, 4) {
            assert!(
                c.text.is_char_boundary(c.text.len()),
                "chunk ends mid-char: {c:?}"
            );
        }
    }

    #[test]
    fn overlap_must_be_smaller_than_chunk() {
        let result = std::panic::catch_unwind(|| chunk_text("abc def ghi", 5, 10));
        assert!(result.is_err(), "overlap >= max_chars must panic");
    }
}
