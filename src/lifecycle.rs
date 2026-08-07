use crate::setup::Tool;

/// Abstraction over OS process operations so the lifecycle logic is testable
/// without spawning or killing anything.
pub trait ProcessManager {
    /// Spawn `<current_exe> serve` detached; return the child pid.
    fn spawn_serve(&self) -> anyhow::Result<u32>;
    /// True if a process with `pid` is currently alive.
    fn is_alive(&self, pid: u32) -> bool;
    /// Terminate `pid`.
    fn kill(&self, pid: u32) -> anyhow::Result<()>;
    /// True if `addr` (host:port) is already bound by some process.
    fn port_in_use(&self, addr: &str) -> bool;
}

/// A snapshot of deeCtx's state for the status dashboard.
#[derive(Debug, Clone)]
pub struct StatusReport {
    pub running: bool,
    pub listen: Option<String>,
    pub running_version: Option<String>,
    pub current_version: String,
    pub tools: Vec<(Tool, bool)>,
    pub warnings: Vec<String>,
}

/// Render the status dashboard. Pure: no I/O, so it is unit-testable.
pub fn render_status(r: &StatusReport) -> String {
    let mut out = String::new();
    if r.running {
        out.push_str("deeCtx — ACTIVE ✓ masking\n");
        let listen = r.listen.clone().unwrap_or_default();
        let ver = r.running_version.clone().unwrap_or_default();
        out.push_str(&format!("  proxy    running · {listen} · v{ver}\n"));
    } else {
        out.push_str("deeCtx — OFF (tools talk directly to the API)\n");
        out.push_str("  proxy    not running\n");
    }
    let tools: Vec<String> = r
        .tools
        .iter()
        .map(|(t, ok)| format!("{t:?} {}", if *ok { "✓" } else { "✗" }))
        .collect();
    out.push_str(&format!("  tools    {}\n", tools.join("   ")));
    for w in &r.warnings {
        out.push_str(&format!("  ⚠ {w}\n"));
    }
    out.push_str(if r.running {
        "\nNext: you're protected. `deectx stop` to turn off.\n"
    } else {
        "\nNext: `deectx start` to protect your tools.\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_active_mentions_stop() {
        let r = StatusReport {
            running: true,
            listen: Some("127.0.0.1:8787".into()),
            running_version: Some("0.2.0".into()),
            current_version: "0.2.0".into(),
            tools: vec![(Tool::ClaudeCode, true), (Tool::Codex, false)],
            warnings: vec![],
        };
        let s = render_status(&r);
        assert!(s.contains("ACTIVE"));
        assert!(s.contains("127.0.0.1:8787"));
        assert!(s.contains("deectx stop"));
        assert!(s.contains("ClaudeCode ✓"));
        assert!(s.contains("Codex ✗"));
    }

    #[test]
    fn render_off_mentions_start() {
        let r = StatusReport {
            running: false,
            listen: None,
            running_version: None,
            current_version: "0.2.0".into(),
            tools: vec![],
            warnings: vec!["port 8787 in use by another app".into()],
        };
        let s = render_status(&r);
        assert!(s.contains("OFF"));
        assert!(s.contains("deectx start"));
        assert!(s.contains("⚠ port 8787 in use by another app"));
    }
}
