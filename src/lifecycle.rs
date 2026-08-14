use crate::home::Pidfile;
use crate::setup::Tool;

/// Abstraction over OS process operations so the lifecycle logic is testable
/// without spawning or killing anything.
pub trait ProcessManager {
    /// Spawn the resident process detached and return its pid: `<current_exe>
    /// tray` on Windows/macOS (the icon hosts the proxy itself), `<current_exe>
    /// serve` on Linux (no tray support). Must match what `install_daemon`
    /// wires for next login — otherwise `start` and a fresh boot disagree about
    /// whether this session gets a tray icon.
    fn spawn_serve(&self) -> anyhow::Result<u32>;
    /// True if a process with `pid` is currently alive.
    fn is_alive(&self, pid: u32) -> bool;
    /// True if `pid` is alive AND is a deectx process. Guards against killing an
    /// unrelated process that reused a stale pidfile's PID after a crash/reboot.
    fn is_deectx(&self, pid: u32) -> bool;
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

/// Kill a running proxy recorded in the pidfile, then clear the pidfile. Only
/// kills when the PID is alive AND confirmed to be a deectx process, so a
/// stale pidfile whose PID was reused never kills an unrelated process.
fn stop_running_proxy<P: ProcessManager>(pm: &P) {
    if let Some(pf) = Pidfile::read() {
        if pm.is_alive(pf.pid) && pm.is_deectx(pf.pid) {
            let _ = pm.kill(pf.pid);
        }
        Pidfile::clear();
    }
}

/// Ensure `<home>/config.toml` exists; write a default file (GDPR + CDR-AU
/// packs active, everything else defaulted) if not. A new install should
/// mask both classes of regulated PII out of the box rather than leaving the
/// dashboard's "Active packs" showing nothing but the bare `default` pack
/// until the user finds `active_packs` in the docs.
fn ensure_config() -> anyhow::Result<()> {
    crate::home::ensure_home()?;
    let path = crate::home::config_path();
    if !path.exists() {
        std::fs::write(
            &path,
            "# deeCtx config — see ARCHITECTURE.md §11\nactive_packs = [\"gdpr\", \"cdr-au\"]\n",
        )?;
    }
    Ok(())
}

/// Wire every installed, non-locked tool to the proxy. Returns per-tool wired
/// state for the status report; a single tool failing never aborts the rest.
pub(crate) fn wire_tools() -> Vec<(Tool, bool)> {
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

/// Block briefly until the freshly spawned `serve` child writes its pidfile and
/// is confirmed alive, so the status printed right after `start` reflects the
/// running proxy instead of racing the spawn. Gives up after ~2s.
fn await_started<P: ProcessManager>(pm: &P) {
    for _ in 0..40 {
        if Pidfile::read()
            .map(|p| pm.is_alive(p.pid) && pm.is_deectx(p.pid))
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Turn deeCtx ON. Idempotent: replaces any running/stale proxy with the
/// current binary, wires tools, installs autostart, and starts serving.
pub fn start<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    ensure_config()?;
    stop_running_proxy(pm);
    let _tools = wire_tools();
    let _ = crate::setup::install_daemon();
    let _pid = pm.spawn_serve()?;
    await_started(pm);
    status(pm)
}

/// Turn deeCtx OFF: restore tool configs (direct to API), stop the proxy, and
/// remove the login autostart so it stays off until `start`. Returns a
/// warning per tool whose config could not be restored (e.g. a locked file) —
/// callers must surface these rather than claiming a clean stop, since a
/// still-wired tool will fail every request against the now-dead proxy.
pub fn stop<P: ProcessManager>(pm: &P) -> anyhow::Result<Vec<String>> {
    let warnings = crate::setup::unwrap()
        .into_iter()
        .map(|(tool, e)| {
            format!(
                "{tool:?} config could not be restored ({e}) — it may still point at \
                 the proxy; close/restart {tool:?} after fixing this"
            )
        })
        .collect();
    stop_running_proxy(pm);
    let _ = crate::setup::uninstall_daemon();
    Ok(warnings)
}

/// Full teardown: stop, then optionally delete config + ledger. Never removes
/// the binary. Propagates any restore warnings from `stop`.
pub fn uninstall<P: ProcessManager>(pm: &P, delete_data: bool) -> anyhow::Result<Vec<String>> {
    let warnings = stop(pm)?;
    if delete_data {
        let _ = std::fs::remove_file(crate::home::config_path());
        let _ = std::fs::remove_file(crate::home::ledger_path());
    }
    Ok(warnings)
}

/// Build the current status snapshot from the pidfile, process liveness, tool
/// wiring, and version comparison.
pub fn status<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let pf = Pidfile::read();
    // "running" means our deectx proxy is alive — not merely that the recorded
    // PID exists (it may have been reused by an unrelated process).
    let running = pf
        .as_ref()
        .map(|p| pm.is_alive(p.pid) && pm.is_deectx(p.pid))
        .unwrap_or(false);

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

/// Real process operations. `is_alive`/`is_deectx`/`kill` shell out to platform
/// tools to avoid a new native dependency; `port_in_use` probes with a bind.
pub struct OsProcessManager;

impl ProcessManager for OsProcessManager {
    fn spawn_serve(&self) -> anyhow::Result<u32> {
        let exe = std::env::current_exe()?;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let arg = "tray";
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let arg = "serve";
        let child = std::process::Command::new(exe).arg(arg).spawn()?;
        Ok(child.id())
    }

    fn is_alive(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        {
            let out = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output();
            match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                Err(_) => false,
            }
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    fn is_deectx(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        let image = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        #[cfg(not(windows))]
        let image = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output();
        image
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .to_ascii_lowercase()
                    .contains("deectx")
            })
            .unwrap_or(false)
    }

    fn kill(&self, pid: u32) -> anyhow::Result<()> {
        #[cfg(windows)]
        let status = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()?;
        #[cfg(not(windows))]
        let status = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("failed to kill pid {pid}")
        }
    }

    fn port_in_use(&self, addr: &str) -> bool {
        std::net::TcpListener::bind(addr).is_err()
    }
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

    #[test]
    fn os_pm_reports_self_alive() {
        let pm = OsProcessManager;
        assert!(pm.is_alive(std::process::id()), "our own pid must be alive");
        assert!(!pm.is_alive(0), "pid 0 is never a live user process");
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
            let pid = 4242;
            // Emulate the real serve: register as alive and write the pidfile so
            // start()'s await_started/status observe a running proxy.
            self.alive.borrow_mut().push(pid);
            Pidfile {
                pid,
                listen: "127.0.0.1:8787".into(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }
            .write()
            .ok();
            Ok(pid)
        }
        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains(&pid)
        }
        fn is_deectx(&self, pid: u32) -> bool {
            self.alive.borrow().contains(&pid)
        }
        fn kill(&self, pid: u32) -> anyhow::Result<()> {
            self.killed.borrow_mut().push(pid);
            self.alive.borrow_mut().retain(|&p| p != pid);
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
        assert!(
            report.running,
            "status after start must reflect the running proxy"
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

    #[test]
    fn stale_pidfile_with_foreign_pid_is_not_killed() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("foreign");
        Pidfile {
            pid: 777,
            listen: "127.0.0.1:8787".into(),
            version: "0.2.0".into(),
        }
        .write()
        .unwrap();
        // A ProcessManager where 777 is alive but NOT a deectx process (PID reuse).
        struct ForeignPm;
        impl ProcessManager for ForeignPm {
            fn spawn_serve(&self) -> anyhow::Result<u32> {
                Ok(1)
            }
            fn is_alive(&self, _pid: u32) -> bool {
                true
            }
            fn is_deectx(&self, _pid: u32) -> bool {
                false
            }
            fn kill(&self, _pid: u32) -> anyhow::Result<()> {
                panic!("must not kill a foreign PID");
            }
            fn port_in_use(&self, _addr: &str) -> bool {
                false
            }
        }
        stop(&ForeignPm).unwrap();
        assert!(
            Pidfile::read().is_none(),
            "stale pidfile must still be cleared"
        );
        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
