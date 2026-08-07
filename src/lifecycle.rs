use crate::home::Pidfile;
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

const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// Kill a running proxy recorded in the pidfile (if its pid is alive), then
/// clear the pidfile. Safe when nothing is running.
fn stop_running_proxy<P: ProcessManager>(pm: &P) {
    if let Some(pf) = Pidfile::read() {
        if pm.is_alive(pf.pid) {
            let _ = pm.kill(pf.pid);
        }
        Pidfile::clear();
    }
}

/// Ensure `<home>/config.toml` exists; write an empty file (all defaults) if not.
fn ensure_config() -> anyhow::Result<()> {
    crate::home::ensure_home()?;
    let path = crate::home::config_path();
    if !path.exists() {
        std::fs::write(&path, "# deeCtx config — see ARCHITECTURE.md §11\n")?;
    }
    Ok(())
}

/// Wire every installed, non-locked tool to the proxy. Returns per-tool wired
/// state for the status report; a single tool failing never aborts the rest.
fn wire_tools() -> Vec<(Tool, bool)> {
    let mut out = Vec::new();
    for (tool, path) in crate::setup::discover() {
        if crate::setup::is_locked(tool, &path) {
            out.push((tool, false));
            continue;
        }
        let ok = crate::setup::patch_config(tool, &path).is_ok();
        out.push((tool, ok));
    }
    out
}

/// Turn deeCtx ON. Idempotent: replaces any running/stale proxy with the
/// current binary, wires tools, installs autostart, and starts serving.
pub fn start<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    ensure_config()?;
    stop_running_proxy(pm);
    let _tools = wire_tools();
    let _ = crate::setup::install_daemon();
    let _pid = pm.spawn_serve()?;
    status(pm)
}

/// Turn deeCtx OFF: restore tool configs (direct to API), stop the proxy, and
/// remove the login autostart so it stays off until `start`.
pub fn stop<P: ProcessManager>(pm: &P) -> anyhow::Result<()> {
    let _ = crate::setup::unwrap();
    stop_running_proxy(pm);
    let _ = crate::setup::uninstall_daemon();
    Ok(())
}

/// Full teardown: stop, then optionally delete config + ledger. Never removes
/// the binary.
pub fn uninstall<P: ProcessManager>(pm: &P, delete_data: bool) -> anyhow::Result<()> {
    stop(pm)?;
    if delete_data {
        let _ = std::fs::remove_file(crate::home::config_path());
        let _ = std::fs::remove_file(crate::home::ledger_path());
    }
    Ok(())
}

/// Build the current status snapshot from the pidfile, process liveness, tool
/// wiring, and version comparison.
pub fn status<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let pf = Pidfile::read();
    let running = pf.as_ref().map(|p| pm.is_alive(p.pid)).unwrap_or(false);

    let mut tools = Vec::new();
    for (tool, path) in crate::setup::discover() {
        let wired = std::fs::read_to_string(&path)
            .map(|c| crate::setup::wired(tool, &c))
            .unwrap_or(false);
        tools.push((tool, wired));
    }

    let mut warnings = Vec::new();
    if let Some(pf) = &pf {
        if running && pf.version != current_version {
            warnings.push(format!(
                "update installed (running v{}, have v{current_version}) — run `deectx start` to apply",
                pf.version
            ));
        }
    }
    if !running && pm.port_in_use(DEFAULT_LISTEN) {
        warnings.push(format!("port {DEFAULT_LISTEN} in use by another app"));
    }

    Ok(StatusReport {
        running,
        listen: pf.as_ref().map(|p| p.listen.clone()),
        running_version: pf.as_ref().map(|p| p.version.clone()),
        current_version,
        tools,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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

    #[derive(Default)]
    struct FakePm {
        alive: RefCell<Vec<u32>>,
        killed: RefCell<Vec<u32>>,
        spawned: RefCell<bool>,
    }
    impl ProcessManager for FakePm {
        fn spawn_serve(&self) -> anyhow::Result<u32> {
            *self.spawned.borrow_mut() = true;
            Ok(4242)
        }
        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains(&pid)
        }
        fn kill(&self, pid: u32) -> anyhow::Result<()> {
            self.killed.borrow_mut().push(pid);
            Ok(())
        }
        fn port_in_use(&self, _addr: &str) -> bool {
            false
        }
    }

    /// Point DEECTX_HOME and the profile at a fresh temp dir so setup::discover
    /// (which reads USERPROFILE/HOME) finds no real tool configs. Caller must
    /// hold `crate::home::ENV_LOCK`.
    fn isolated_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deectx_life_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DEECTX_HOME", &dir);
        std::env::set_var("USERPROFILE", &dir);
        std::env::set_var("HOME", &dir);
        // Redirect the autostart-daemon target (Windows uses APPDATA) into the
        // temp dir so start()/stop() never touch the real Startup folder.
        std::env::set_var("APPDATA", dir.join("AppData").join("Roaming"));
        dir
    }

    #[test]
    fn start_kills_stale_proxy_then_spawns() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("start");
        // Seed a stale pidfile whose pid is "alive".
        Pidfile {
            pid: 999,
            listen: "127.0.0.1:8787".into(),
            version: "0.1.0".into(),
        }
        .write()
        .unwrap();
        let pm = FakePm::default();
        pm.alive.borrow_mut().push(999);

        let report = start(&pm).unwrap();

        assert!(
            pm.killed.borrow().contains(&999),
            "stale proxy must be killed"
        );
        assert!(*pm.spawned.borrow(), "a new proxy must be spawned");
        assert!(
            crate::home::config_path().exists(),
            "config.toml must be created"
        );
        assert_eq!(report.current_version, env!("CARGO_PKG_VERSION"));
        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_clears_pidfile_and_kills() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("stop");
        Pidfile {
            pid: 555,
            listen: "127.0.0.1:8787".into(),
            version: "0.2.0".into(),
        }
        .write()
        .unwrap();
        let pm = FakePm::default();
        pm.alive.borrow_mut().push(555);

        stop(&pm).unwrap();

        assert!(pm.killed.borrow().contains(&555));
        assert!(Pidfile::read().is_none(), "pidfile must be cleared");
        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
