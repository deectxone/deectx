//! Runs the masking proxy on a background thread with its own Tokio
//! runtime, controllable in-process via a graceful-shutdown channel — so
//! the tray can stop/start masking without exiting the process hosting the
//! icon. Wraps [`crate::proxy::run_proxy_with_shutdown`]; no proxy logic of
//! its own.

use crate::config::Config;

// Consumed starting in a later task (tray icon/menu wiring); allowed dead
// here since this task is deliberately just the in-process control plane.
// The non-test `--lib` build doesn't see `#[cfg(test)] mod tests` usage below,
// so without this it dead-codes exactly like `tray::icon` did in an earlier
// task (see commit "tray: silence dead_code on icon items unused until later
// tasks").
#[allow(dead_code)]
pub(crate) struct ProxyHandle {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[allow(dead_code)]
impl ProxyHandle {
    /// Wires every installed, non-locked tool to the proxy (same as
    /// `lifecycle::start`'s wiring step), then spawns the masking proxy on a
    /// background thread. Returns immediately; the proxy binds asynchronously
    /// on that thread.
    pub(crate) fn start(cfg: Config) -> ProxyHandle {
        let _wired = crate::lifecycle::wire_tools();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("tray: failed to start proxy runtime: {e}");
                    return;
                }
            };
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(e) = rt.block_on(crate::proxy::run_proxy_with_shutdown(cfg, shutdown)) {
                tracing::warn!("tray-hosted proxy exited with error: {e}");
            }
        });
        ProxyHandle {
            shutdown_tx: Some(tx),
            thread: Some(thread),
        }
    }

    /// Unwires tools (same as `deectx stop`'s cleanup — including the
    /// opencode fail-closed plugin removal) and signals the background
    /// thread to close its listener, then waits for it to finish. Returns
    /// any restore warnings, exactly like `lifecycle::stop`.
    pub(crate) fn stop(&mut self) -> Vec<String> {
        let warnings = crate::setup::unwrap()
            .into_iter()
            .map(|(tool, e)| {
                format!(
                    "{tool:?} config could not be restored ({e}) — it may still point at \
                     the proxy; close/restart {tool:?} after fixing this"
                )
            })
            .collect();
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        warnings
    }

    /// True once `start` has been called and `stop` hasn't (yet) joined the
    /// background thread. Does not confirm the listener has actually bound —
    /// good enough for the icon's optimistic on/off state in v1.
    pub(crate) fn is_running(&self) -> bool {
        self.thread.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(ledger_name: &str) -> Config {
        let ledger_path = std::env::temp_dir().join(format!(
            "deectx_trayhandle_{ledger_name}_{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&ledger_path);
        Config {
            listen: "127.0.0.1:0".into(),
            ledger_path,
            ..Default::default()
        }
    }

    /// Point DEECTX_HOME/USERPROFILE/HOME at a fresh temp dir so
    /// `ProxyHandle::start`'s call into `lifecycle::wire_tools` (and `stop`'s
    /// call into `setup::unwrap`) find no real tool configs on the machine
    /// running the tests, instead of patching/restoring the developer's
    /// actual Claude Code/Codex/Opencode config files. Mirrors
    /// `lifecycle::tests::isolated_home`. Caller must hold
    /// `crate::home::ENV_LOCK` for the duration of the test.
    fn isolated_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deectx_trayhandle_home_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DEECTX_HOME", &dir);
        std::env::set_var("USERPROFILE", &dir);
        std::env::set_var("HOME", &dir);
        std::env::set_var("APPDATA", dir.join("AppData").join("Roaming"));
        dir
    }

    #[test]
    fn start_then_stop_reports_running_state_correctly() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("start_stop");

        let mut handle = ProxyHandle::start(test_config("start_stop"));
        assert!(handle.is_running(), "must report running right after start");

        let warnings = handle.stop();
        assert!(
            warnings.is_empty(),
            "no tools were ever wired in this test environment, so nothing should fail to restore: {warnings:?}"
        );
        assert!(!handle.is_running(), "must report stopped after stop()");

        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_is_safe_to_call_when_already_stopped() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("double_stop");

        let mut handle = ProxyHandle::start(test_config("double_stop"));
        handle.stop();
        let warnings = handle.stop();
        assert!(warnings.is_empty());
        assert!(!handle.is_running());

        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
