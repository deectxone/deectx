pub mod regex;
pub mod secrets;

use crate::span::Span;

pub trait Detector: Send + Sync {
    fn detect(&self, text: &str) -> Vec<Span>;
}

pub struct DetectorChain {
    detectors: Vec<Box<dyn Detector>>,
}

impl DetectorChain {
    pub fn new(detectors: Vec<Box<dyn Detector>>) -> Self {
        Self { detectors }
    }

    pub fn detect(&self, text: &str) -> Vec<Span> {
        let mut spans: Vec<Span> = self.detectors.iter().flat_map(|d| d.detect(text)).collect();
        // longest match wins at same start; earlier start wins otherwise
        spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end - s.start)));
        let mut out: Vec<Span> = Vec::new();
        for s in spans {
            if out.last().map_or(true, |prev| s.start >= prev.end) {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Action;

    struct Fake;
    impl Detector for Fake {
        fn detect(&self, _t: &str) -> Vec<Span> {
            vec![
                Span::new(0, 5, "short", Action::Mask, "xxxxx"),
                Span::new(0, 10, "long", Action::Mask, "xxxxxxxxxx"),
                Span::new(12, 15, "other", Action::Mask, "yyy"),
            ]
        }
    }

    #[test]
    fn overlap_resolution_keeps_longest_at_same_start() {
        let chain = DetectorChain::new(vec![Box::new(Fake)]);
        let spans = chain.detect("anything");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].entity, "long");
        assert_eq!(spans[1].entity, "other");
    }
}
