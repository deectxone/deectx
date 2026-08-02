#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action { #[default] Mask, Redact }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub entity: String,
    pub action: Action,
    pub text: String,
    pub alert: bool,
}

impl Span {
    pub fn new(start: usize, end: usize, entity: &str, action: Action, text: &str) -> Self {
        Self { start, end, entity: entity.into(), action, text: text.into(), alert: false }
    }
}
