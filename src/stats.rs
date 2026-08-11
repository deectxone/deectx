use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Live, process-scoped counters. Cheap atomic increments; never read from disk.
#[derive(Default)]
pub struct LiveStats {
    requests: AtomicU64,
    masked: AtomicU64,
    redacted: AtomicU64,
    alerts: AtomicU64,
    errors: AtomicU64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StatsSnapshot {
    pub requests: u64,
    pub masked: u64,
    pub redacted: u64,
    pub alerts: u64,
    pub errors: u64,
}

impl LiveStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_event(&self, action: &str, alert: bool) {
        if action == "redact" {
            self.redacted.fetch_add(1, Ordering::Relaxed);
        } else {
            self.masked.fetch_add(1, Ordering::Relaxed);
        }
        if alert {
            self.alerts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A masking failure (panic or serialization error) was recovered from by
    /// forwarding the request unmasked rather than blocking or corrupting it.
    /// Tracked so `deectx status`/`audit` surface it instead of it going
    /// unnoticed.
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            masked: self.masked.load(Ordering::Relaxed),
            redacted: self.redacted.load(Ordering::Relaxed),
            alerts: self.alerts.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_is_zero() {
        let s = LiveStats::new();
        assert_eq!(
            s.snapshot(),
            StatsSnapshot {
                requests: 0,
                masked: 0,
                redacted: 0,
                alerts: 0,
                errors: 0
            }
        );
    }

    #[test]
    fn records_requests_and_mask_events() {
        let s = LiveStats::new();
        s.record_request();
        s.record_event("mask", false);
        s.record_event("redact", true);
        let snap = s.snapshot();
        assert_eq!(snap.requests, 1);
        assert_eq!(snap.masked, 1);
        assert_eq!(snap.redacted, 1);
        assert_eq!(snap.alerts, 1);
    }

    #[test]
    fn record_error_tracks_recovered_masking_failures() {
        let s = LiveStats::new();
        s.record_request();
        s.record_error();
        s.record_error();
        let snap = s.snapshot();
        assert_eq!(snap.requests, 1);
        assert_eq!(snap.errors, 2, "each recovered masking failure must count");
        assert_eq!(snap.masked, 0);
        assert_eq!(snap.redacted, 0);
    }

    #[test]
    fn serializes_to_json() {
        let s = LiveStats::new();
        s.record_request();
        let json = serde_json::to_string(&s.snapshot()).unwrap();
        assert!(json.contains("\"requests\":1"));
    }
}
