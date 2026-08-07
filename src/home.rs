use std::path::PathBuf;

/// deeCtx runtime home: `$DEECTX_HOME` or `~/.deectx`. All runtime state
/// (config, ledger, pidfile) lives here so the daemon and CLI agree on paths
/// regardless of the process working directory.
pub fn deectx_home() -> PathBuf {
    if let Ok(h) = std::env::var("DEECTX_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".deectx")
}

/// Default config location: `<home>/config.toml`.
pub fn config_path() -> PathBuf {
    deectx_home().join("config.toml")
}

/// Default ledger location: `<home>/ledger.jsonl`.
pub fn ledger_path() -> PathBuf {
    deectx_home().join("ledger.jsonl")
}

/// Pidfile location: `<home>/deectx.pid`.
pub fn pidfile_path() -> PathBuf {
    deectx_home().join("deectx.pid")
}

/// Create the home directory if it does not exist.
pub fn ensure_home() -> std::io::Result<()> {
    std::fs::create_dir_all(deectx_home())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env mutation across tests in this file.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn deectx_home_env_override_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEECTX_HOME", "/tmp/deectx-test-home");
        assert_eq!(deectx_home(), PathBuf::from("/tmp/deectx-test-home"));
        assert_eq!(
            config_path(),
            PathBuf::from("/tmp/deectx-test-home").join("config.toml")
        );
        assert_eq!(
            ledger_path(),
            PathBuf::from("/tmp/deectx-test-home").join("ledger.jsonl")
        );
        std::env::remove_var("DEECTX_HOME");
    }

    #[test]
    fn deectx_home_defaults_under_profile() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("DEECTX_HOME");
        let home = deectx_home();
        assert!(
            home.ends_with(".deectx"),
            "default home must end with .deectx: {home:?}"
        );
    }
}
