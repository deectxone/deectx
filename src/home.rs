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

/// Contents of `<home>/deectx.pid`, written by `serve` on start and read by
/// the lifecycle commands. Three newline-separated lines: pid, listen, version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pidfile {
    pub pid: u32,
    pub listen: String,
    pub version: String,
}

impl Pidfile {
    pub fn encode(&self) -> String {
        format!("{}\n{}\n{}\n", self.pid, self.listen, self.version)
    }

    pub fn parse(s: &str) -> Option<Pidfile> {
        let mut lines = s.lines();
        let pid = lines.next()?.trim().parse().ok()?;
        let listen = lines.next()?.trim().to_string();
        let version = lines.next().unwrap_or("").trim().to_string();
        Some(Pidfile {
            pid,
            listen,
            version,
        })
    }

    pub fn write(&self) -> std::io::Result<()> {
        ensure_home()?;
        std::fs::write(pidfile_path(), self.encode())
    }

    pub fn read() -> Option<Pidfile> {
        std::fs::read_to_string(pidfile_path())
            .ok()
            .and_then(|s| Pidfile::parse(&s))
    }

    pub fn clear() {
        let _ = std::fs::remove_file(pidfile_path());
    }
}

/// Serializes tests that mutate the process-global `DEECTX_HOME`/profile env.
/// Shared across modules (e.g. `config`) so their env-mutating tests don't race
/// this file's under parallel `cargo test`.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn pidfile_roundtrips() {
        let pf = Pidfile {
            pid: 4321,
            listen: "127.0.0.1:8787".into(),
            version: "0.2.0".into(),
        };
        let parsed = Pidfile::parse(&pf.encode()).unwrap();
        assert_eq!(parsed, pf);
    }

    #[test]
    fn pidfile_parse_rejects_garbage() {
        assert!(Pidfile::parse("not-a-number\n").is_none());
        assert!(Pidfile::parse("").is_none());
    }

    #[test]
    fn pidfile_write_read_clear() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("deectx_pf_{}", std::process::id()));
        std::env::set_var("DEECTX_HOME", &dir);
        let pf = Pidfile {
            pid: 7,
            listen: "127.0.0.1:9999".into(),
            version: "9.9.9".into(),
        };
        pf.write().unwrap();
        assert_eq!(Pidfile::read().unwrap(), pf);
        Pidfile::clear();
        assert!(Pidfile::read().is_none());
        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
