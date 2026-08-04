#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
    Unknown,
}

/// Classify the upstream provider from an Authorization header value.
pub fn classify(auth: &str) -> Provider {
    let key = auth.trim();
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
    fn bare_bearer_is_unknown() {
        assert_eq!(classify("Bearer somelongtoken"), Provider::Unknown);
    }

    #[test]
    fn leading_whitespace_is_trimmed() {
        assert_eq!(classify("  sk-proj-x  "), Provider::OpenAI);
    }
}
