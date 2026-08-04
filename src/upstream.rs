#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
    Unknown,
}

/// Classify the upstream provider from an Authorization header value. Real
/// tools send `Authorization: Bearer sk-...`, so a leading `Bearer ` prefix is
/// stripped (case-insensitively) before the key-shape checks.
pub fn classify(auth: &str) -> Provider {
    let key = auth.trim();
    let key = key.strip_prefix("Bearer ").unwrap_or(key);
    let key = key.strip_prefix("bearer ").unwrap_or(key);
    if key.starts_with("sk-ant-") {
        Provider::Anthropic
    } else if key.starts_with("sk-") {
        Provider::OpenAI
    } else {
        Provider::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_key_shape_is_anthropic() {
        assert_eq!(classify("sk-ant-api03-abcdef"), Provider::Anthropic);
    }

    #[test]
    fn openai_key_shape_is_openai() {
        assert_eq!(classify("sk-proj-abcdef"), Provider::OpenAI);
    }

    #[test]
    fn bare_token_without_prefix_is_unknown() {
        assert_eq!(classify("somelongtoken"), Provider::Unknown);
    }

    #[test]
    fn bearer_openai_key_is_openai() {
        assert_eq!(classify("Bearer sk-proj-abcdef"), Provider::OpenAI);
    }

    #[test]
    fn bearer_anthropic_key_is_anthropic() {
        assert_eq!(classify("Bearer sk-ant-api03-abcdef"), Provider::Anthropic);
    }

    #[test]
    fn lowercase_bearer_prefix_is_stripped() {
        assert_eq!(classify("bearer sk-ant-api03-abcdef"), Provider::Anthropic);
    }

    #[test]
    fn leading_whitespace_is_trimmed() {
        assert_eq!(classify("  sk-proj-x  "), Provider::OpenAI);
    }
}
