#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Mask, Redact }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub entity: String,
    pub action: Action,
    pub text: String,
}

impl Span {
    pub fn new(start: usize, end: usize, entity: &str, action: Action, text: &str) -> Self {
        Self { start, end, entity: entity.into(), action, text: text.into() }
    }
}
