use crate::masker::Masker;

const MAX_PENDING: usize = 64 * 1024;

pub struct SseRehydrator {
    pending: Vec<u8>,
    max_hold: usize,
}

impl SseRehydrator {
    pub fn new(max_hold: usize) -> Self {
        Self { pending: Vec::new(), max_hold }
    }

    pub fn push(&mut self, chunk: &[u8], session: &str, masker: &Masker) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let emit_len = self.emit_len();
        // Hard cap: a non-UTF-8 stream collapses emit_len to 0, which would
        // otherwise buffer the whole body in memory. When pending exceeds the
        // bound, flush the oldest bytes through immediately (raw rehydrate)
        // regardless of placeholder-prefix status so memory stays bounded.
        let emit_len = self.pending.len().saturating_sub(MAX_PENDING).max(emit_len);
        let tail = self.pending.split_off(emit_len);
        let emit = std::mem::take(&mut self.pending);
        self.pending = tail;
        rehydrate_bytes(&emit, session, masker)
    }

    pub fn finish(&mut self, session: &str, masker: &Masker) -> Vec<u8> {
        let tail = std::mem::take(&mut self.pending);
        rehydrate_bytes(&tail, session, masker)
    }

    fn emit_len(&self) -> usize {
        let n = self.pending.len();
        if n == 0 {
            return 0;
        }
        let mut char_end = n;
        while char_end > 0 && std::str::from_utf8(&self.pending[..char_end]).is_err() {
            char_end -= 1;
        }
        let hold_from = char_end.saturating_sub(self.max_hold);
        for k in hold_from..char_end {
            if is_placeholder_prefix(&self.pending[k..]) {
                return k;
            }
        }
        char_end
    }
}

fn is_placeholder_prefix(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes[0] != b'[' {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn rehydrate_bytes(emit: &[u8], session: &str, masker: &Masker) -> Vec<u8> {
    let text = String::from_utf8_lossy(emit);
    masker.rehydrate(session, &text).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::masker::Masker;
    use crate::span::{Action, Span};

    fn masker_with(session: &str, text: &str) -> Masker {
        let m = Masker::new();
        let spans = vec![
            Span::new(0, 19, "email", Action::Mask, "jane.doe@example.com"),
            Span::new(24, 45, "email", Action::Mask, "bob.smith@example.com"),
        ];
        m.mask_text(session, text, &spans);
        m
    }

    #[test]
    fn placeholder_split_across_chunks_is_rehydrated() {
        let m = masker_with("s1", "jane.doe@example.com and bob.smith@example.com");
        let mut r = SseRehydrator::new(64);
        let a = r.push(b"data: {\"delta\":{\"content\":\"hello [EMA", "s1", &m);
        assert!(!String::from_utf8_lossy(&a).contains("[EMA"), "prefix leaked: {:?}", a);
        let b = r.push(b"IL_1]\"}}\n\ndata: [DONE]\n\n", "s1", &m);
        let out = String::from_utf8(b).unwrap();
        assert!(out.contains("jane.doe@example.com"), "got: {out}");
        assert!(out.contains("[EMAIL_1]") == false, "placeholder leaked: {out}");
        let c = r.finish("s1", &m);
        assert!(c.is_empty());
    }

    #[test]
    fn complete_placeholder_in_one_chunk_is_emitted_immediately() {
        let m = masker_with("s1", "jane.doe@example.com and bob.smith@example.com");
        let mut r = SseRehydrator::new(64);
        let out = r.push(b"contact [EMAIL_1] please", "s1", &m);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("jane.doe@example.com"));
    }

    #[test]
    fn non_placeholder_text_flows_through_unchanged() {
        let m = Masker::new();
        let mut r = SseRehydrator::new(64);
        let out = r.push(b"just plain text, no secrets", "s1", &m);
        assert_eq!(String::from_utf8(out).unwrap(), "just plain text, no secrets");
    }

    #[test]
    fn multibyte_char_split_across_chunks_is_not_corrupted() {
        let m = Masker::new();
        let mut r = SseRehydrator::new(64);
        let full = "héllo wörld".as_bytes();
        let split = 9;
        let a = r.push(&full[..split], "s1", &m);
        let b = r.push(&full[split..], "s1", &m);
        let joined = format!("{}{}", String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
        assert!(joined.contains("wörld"), "multibyte char corrupted: {joined}");
        assert!(!joined.contains('\u{FFFD}'), "replacement char leaked: {joined}");
    }

    #[test]
    fn trailing_partial_placeholder_held_until_finish() {
        let m = masker_with("s1", "jane.doe@example.com and bob.smith@example.com");
        let mut r = SseRehydrator::new(64);
        let a = r.push(b"text ends with [EMA", "s1", &m);
        assert!(!String::from_utf8_lossy(&a).contains("[EMA"), "partial leaked: {:?}", a);
        let b = r.finish("s1", &m);
        let out = format!("{}{}", String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
        assert!(out.contains("text ends with [EMA"), "partial flushed as-is: {out}");
    }

    #[test]
    fn non_utf8_stream_pending_is_capped() {
        let m = Masker::new();
        let mut r = SseRehydrator::new(64);
        let chunk: Vec<u8> = (0..(70 * 1024)).map(|i| [0xFF, 0x00, 0xAA][i % 3]).collect();
        let emit = r.push(&chunk, "s1", &m);
        assert!(!emit.is_empty(), "overflow must flush the oldest bytes immediately");
        assert!(r.pending.len() <= MAX_PENDING, "pending grew unbounded: {} bytes", r.pending.len());
        assert_eq!(r.pending.len(), MAX_PENDING, "pending should sit exactly at the cap");
        let emit2 = r.push(&chunk, "s1", &m);
        assert!(!emit2.is_empty());
        assert!(r.pending.len() <= MAX_PENDING, "pending must stay capped across the stream");
    }
}
