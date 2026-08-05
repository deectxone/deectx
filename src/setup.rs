use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    Opencode,
}

/// User home directory (Windows `USERPROFILE`, unix `HOME`).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(PathBuf::from)
}

impl Tool {
    /// Where the tool stores its user-level config. Derives the home dir from
    /// the environment each call (Windows `USERPROFILE`, unix `HOME`).
    pub fn config_path(&self) -> Option<PathBuf> {
        let home = home_dir()?;
        Some(match self {
            Tool::ClaudeCode => home.join(".claude").join("settings.json"),
            Tool::Codex => home.join(".codex").join("config.toml"),
            Tool::Opencode => home.join(".config").join("opencode").join("opencode.json"),
        })
    }
}

/// Detect which tools are installed and have a config we can patch.
pub fn discover() -> Vec<(Tool, PathBuf)> {
    let mut out = Vec::new();
    for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Opencode] {
        if let Some(p) = tool.config_path() {
            if p.exists() {
                out.push((tool, p));
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchResult {
    AlreadyPatched,
    Patched,
}

/// Rewrite a tool's config to point at the local proxy, backing up the
/// original to `path + ".bak"` first. Idempotent.
pub fn patch_config(tool: Tool, path: &PathBuf) -> Result<PatchResult> {
    let original = std::fs::read_to_string(path)?;
    if wired(tool, &original) {
        return Ok(PatchResult::AlreadyPatched);
    }

    let backup = PathBuf::from(format!("{}.bak", path.display()));
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }

    let patched = patch_for(tool, &original)?;
    std::fs::write(path, patched)?;
    Ok(PatchResult::Patched)
}

/// Provider-specific rewriting of the original config content.
fn patch_for(tool: Tool, original: &str) -> Result<String> {
    let base = "http://127.0.0.1:8787";
    match tool {
        Tool::ClaudeCode => {
            let mut v: serde_json::Value = serde_json::from_str(original)?;
            v["env"]["ANTHROPIC_BASE_URL"] = serde_json::json!(base);
            Ok(serde_json::to_string_pretty(&v)?)
        }
        Tool::Codex => {
            let header = "model_provider = \"deectx\"\n";
            let block = &[
                "[model_providers.deectx]",
                "name = \"deeCtx proxy\"",
                &format!("base_url = \"{base}\""),
                "wire_api = \"chat\"",
                "env_key = \"OPENAI_API_KEY\"",
                "",
            ]
            .join("\n");
            let table_exists = codex_table_exists(original);
            let missing_keys = codex_missing_key_lines(original, base);
            let mut patched = String::new();
            let mut found = false;
            let mut seen_section = false;
            for line in original.lines() {
                let trimmed = line.trim_start();
                if trimmed == "[model_providers.deectx]" {
                    patched.push_str(line);
                    patched.push('\n');
                    // Reuse the existing table: only append keys it is missing,
                    // never emitting a second `[model_providers.deectx]` header.
                    for key_line in &missing_keys {
                        patched.push_str(key_line);
                        patched.push('\n');
                    }
                    seen_section = true;
                    continue;
                }
                if trimmed.starts_with('[') {
                    seen_section = true;
                }
                if !seen_section
                    && (trimmed == "model_provider"
                        || trimmed.starts_with("model_provider ")
                        || trimmed.starts_with("model_provider="))
                {
                    if found {
                        continue;
                    }
                    patched.push_str(header);
                    found = true;
                } else {
                    patched.push_str(line);
                    patched.push('\n');
                }
            }
            if !found {
                patched = format!("{header}{patched}");
            }
            let mut out = patched.trim_end().to_string();
            if !table_exists {
                out.push('\n');
                out.push_str(block);
            }
            Ok(out)
        }
        Tool::Opencode => {
            let mut v: serde_json::Value = serde_json::from_str(original)?;
            v["provider"]["anthropic"]["options"]["baseURL"] = serde_json::json!(base);
            Ok(serde_json::to_string_pretty(&v)?)
        }
    }
}

/// True when the tool's traffic is managed by locked OAuth (e.g. Claude Pro/Max
/// where the config carries an `oauth_account`) and intercepting it is unsafe.
pub fn is_locked(tool: Tool, path: &PathBuf) -> bool {
    let _ = tool;
    match std::fs::read_to_string(path) {
        Ok(content) => content.contains("oauth_account"),
        Err(_) => false,
    }
}

/// True when the original config already declares a `[model_providers.deectx]`
/// table, so patching must reuse it rather than emitting a duplicate header.
fn codex_table_exists(original: &str) -> bool {
    original
        .lines()
        .any(|l| l.trim_start() == "[model_providers.deectx]")
}

/// Key-value lines that the existing `[model_providers.deectx]` table is
/// missing and therefore needs appended (keys already present are left alone).
fn codex_missing_key_lines(original: &str, base: &str) -> Vec<String> {
    let mut in_table = false;
    let mut present: Vec<&str> = Vec::new();
    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_table = trimmed == "[model_providers.deectx]";
            continue;
        }
        if in_table {
            if let Some(key) = trimmed.split('=').next() {
                present.push(key.trim());
            }
        }
    }
    let needed = [
        ("name", "name = \"deeCtx proxy\""),
        ("base_url", &format!("base_url = \"{base}\"")),
        ("wire_api", "wire_api = \"chat\""),
        ("env_key", "env_key = \"OPENAI_API_KEY\""),
    ];
    needed
        .iter()
        .filter(|(key, _)| !present.contains(key))
        .map(|(_, line)| line.to_string())
        .collect()
}

/// True when the tool's config routes through the local proxy. Codex needs both
/// the provider block AND a top-level `model_provider = "deectx"` selection;
/// every other tool only needs the proxy base-URL substring.
pub fn wired(tool: Tool, content: &str) -> bool {
    let has_block = content.contains("127.0.0.1:8787");
    if tool != Tool::Codex {
        return has_block;
    }
    let mut seen_section = false;
    let mut selected = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            seen_section = true;
        } else if !seen_section && trimmed == "model_provider = \"deectx\"" {
            selected = true;
        }
    }
    has_block && selected
}

/// Verify which installed tools are wired to the proxy.
pub fn doctor() -> Result<String> {
    let mut lines = Vec::new();
    let found = discover();
    if found.is_empty() {
        lines.push("deeCtx setup status: no tools found to check".to_string());
    }
    for (tool, path) in found {
        let content = std::fs::read_to_string(&path)?;
        let ok = wired(tool, &content);
        lines.push(format!(
            "{tool:?}: {}",
            if ok { "OK (wired)" } else { "NOT WIRED" }
        ));
    }
    Ok(lines.join("\n"))
}

/// Restore original configs from their `.bak` backups.
pub fn unwrap() -> Result<()> {
    for (_tool, path) in discover() {
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        if backup.exists() {
            std::fs::rename(&backup, &path)?;
        }
    }
    Ok(())
}

/// Windows startup-folder batch file content: runs `<exe> serve` at login.
fn windows_batch(exe: &str) -> String {
    format!("@echo off\r\nstart \"\" \"{exe}\" serve\r\n")
}

/// macOS LaunchAgent plist content: runs `<exe> serve` at login, kept alive.
fn plist_xml(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.deectx.proxy</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// Linux systemd user unit content: runs `<exe> serve`, restarted on failure.
fn systemd_unit(exe: &str) -> String {
    format!(
        "[Unit]\nDescription=deeCtx PII-masking proxy\n\n[Service]\nExecStart={exe} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
    )
}

/// Per-OS autostart artifact path. `None` when the OS is unsupported or the
/// home directory cannot be resolved.
fn daemon_path_for(os: &str) -> Option<PathBuf> {
    match os {
        "windows" => {
            let base = std::env::var("APPDATA")
                .ok()
                .map(PathBuf::from)
                .or_else(|| home_dir().map(|h| h.join("AppData").join("Roaming")))?;
            Some(
                base.join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs")
                    .join("Startup")
                    .join("deectx.cmd"),
            )
        }
        "macos" => Some(
            home_dir()?
                .join("Library")
                .join("LaunchAgents")
                .join("com.deectx.proxy.plist"),
        ),
        "linux" => Some(
            home_dir()?
                .join(".config")
                .join("systemd")
                .join("user")
                .join("deectx.service"),
        ),
        _ => None,
    }
}

/// Per-OS autostart artifact (path + content) for `exe`. Pure with respect to
/// the file system; `None` for unsupported OSes.
fn daemon_artifact_for(os: &str, exe: &str) -> Option<(PathBuf, String)> {
    let content = match os {
        "windows" => windows_batch(exe),
        "macos" => plist_xml(exe),
        "linux" => systemd_unit(exe),
        _ => return None,
    };
    Some((daemon_path_for(os)?, content))
}

/// Write the artifact, creating parent dirs. Separated from spawning so tests
/// never shell out.
fn install_at(path: &std::path::Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Remove the artifact if present. Idempotent.
fn uninstall_at(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Install the per-OS autostart artifact pointing at the current binary
/// running `serve`. On Linux/macOS also runs the systemd/launchctl enable
/// commands (failures ignored — the artifact is the source of truth).
/// Idempotent: safe to re-run.
pub fn install_daemon() -> Result<()> {
    let os = std::env::consts::OS;
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    let (path, content) = daemon_artifact_for(os, &exe)
        .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart daemon"))?;
    install_at(&path, &content)?;
    match os {
        "macos" => {
            let _ = std::process::Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&path)
                .status();
        }
        "linux" => {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "enable", "--now", "deectx.service"])
                .status();
        }
        _ => {}
    }
    Ok(())
}

/// Remove the autostart artifact. On Linux/macOS also runs the
/// systemd/launchctl disable commands (failures ignored). Idempotent: no-op
/// if the artifact is absent.
pub fn uninstall_daemon() -> Result<()> {
    let os = std::env::consts::OS;
    let path = daemon_path_for(os)
        .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart daemon"))?;
    match os {
        "macos" => {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(&path)
                .status();
        }
        "linux" => {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "disable", "--now", "deectx.service"])
                .status();
        }
        _ => {}
    }
    uninstall_at(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deectx_setup_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn original_for(tool: Tool) -> &'static str {
        match tool {
            Tool::ClaudeCode => r#"{"env": {},"model": "opus"}"#,
            Tool::Codex => "model = \"gpt-4o\"\ntemperature = 0\n",
            Tool::Opencode => r#"{"provider":{},"model":"sonnet"}"#,
        }
    }

    fn filename_for(tool: Tool) -> &'static str {
        match tool {
            Tool::ClaudeCode => "settings.json",
            Tool::Codex => "config.toml",
            Tool::Opencode => "opencode.json",
        }
    }

    #[test]
    fn config_path_returns_expected_suffix() {
        let cases = [
            (Tool::ClaudeCode, ".claude/settings.json"),
            (Tool::Codex, ".codex/config.toml"),
            (Tool::Opencode, ".config/opencode/opencode.json"),
        ];
        for (tool, suffix) in cases {
            let path = tool.config_path();
            assert!(path.is_some(), "{tool:?} should resolve a config path");
            let p = path.unwrap().to_string_lossy().replace('\\', "/");
            assert!(
                p.ends_with(suffix),
                "{tool:?}: expected {p:?} to end with {suffix:?}"
            );
        }
        // discover() must not panic and returns a Vec.
        discover();
    }

    #[test]
    fn patch_for_then_unwrap_roundtrips_byte_identical() {
        for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Opencode] {
            let dir = temp_dir(&format!("roundtrip_{tool:?}"));
            let path = dir.join(filename_for(tool));
            let original = original_for(tool);
            std::fs::write(&path, original).unwrap();

            assert_eq!(patch_config(tool, &path).unwrap(), PatchResult::Patched);

            let backup = PathBuf::from(format!("{}.bak", path.display()));
            assert!(backup.exists(), "{tool:?} should create a .bak");
            assert_eq!(
                std::fs::read(&backup).unwrap(),
                original.as_bytes(),
                "{tool:?} .bak must hold the original bytes"
            );

            std::fs::rename(&backup, &path).unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                original.as_bytes(),
                "{tool:?} unwrap must restore byte-identical original"
            );

            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn patch_config_is_idempotent() {
        for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Opencode] {
            let dir = temp_dir(&format!("idempotent_{tool:?}"));
            let path = dir.join(filename_for(tool));
            let original = original_for(tool);
            std::fs::write(&path, original).unwrap();

            assert_eq!(patch_config(tool, &path).unwrap(), PatchResult::Patched);
            let bak_path = dir.join(format!("{}.bak", filename_for(tool)));
            let backup = std::fs::read(&bak_path).unwrap();
            let patched = std::fs::read(&path).unwrap();

            // .bak holds the original bytes (never overwritten).
            assert_eq!(backup, original.as_bytes());

            // Second run: AlreadyPatched, content and .bak unchanged.
            assert_eq!(
                patch_config(tool, &path).unwrap(),
                PatchResult::AlreadyPatched
            );
            assert_eq!(std::fs::read(&path).unwrap(), patched);
            assert_eq!(std::fs::read(&bak_path).unwrap(), backup);

            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn codex_patch_adds_model_provider_selection() {
        let original = "model = \"gpt-4o\"\ntemperature = 0\n";
        let patched = patch_for(Tool::Codex, original).unwrap();

        let header_pos = patched
            .find("model_provider = \"deectx\"")
            .unwrap_or_else(|| panic!("patched must select the deectx provider"));
        let first_section = patched.find('[').unwrap_or(usize::MAX);
        assert!(
            header_pos < first_section,
            "top-level model_provider must precede any [section] header"
        );
        assert!(patched.contains("[model_providers.deectx]"));
        assert!(patched.contains("base_url = \"http://127.0.0.1:8787\""));
    }

    #[test]
    fn codex_patch_replaces_existing_model_provider() {
        let original = "model_provider = \"openai\"\nmodel = \"gpt-4\"\n";
        let patched = patch_for(Tool::Codex, original).unwrap();

        assert_eq!(patched.matches("model_provider = \"deectx\"").count(), 1);
        assert!(
            !patched.contains("model_provider = \"openai\""),
            "existing top-level model_provider value must be replaced"
        );
    }

    #[test]
    fn codex_patch_does_not_touch_section_level_keys() {
        let original = "model = \"gpt-4o\"\n[model_providers.custom]\nname = \"Custom\"\nbase_url = \"http://other:9999\"\n";
        let patched = patch_for(Tool::Codex, original).unwrap();

        assert!(patched.contains("[model_providers.custom]"));
        assert!(patched.contains("name = \"Custom\""));
        assert!(patched.contains("base_url = \"http://other:9999\""));
        assert!(
            patched.matches("model_provider = \"deectx\"").count() == 1,
            "exactly one top-level model_provider selection"
        );
    }

    #[test]
    fn codex_patch_preserves_model_provider_alias() {
        let original = "model_provider_alias = \"custom\"\nmodel_provider = \"openai\"\n";
        let patched = patch_for(Tool::Codex, original).unwrap();

        assert!(
            patched.contains("model_provider_alias = \"custom\""),
            "unrelated top-level key must be preserved: {patched}"
        );
        assert_eq!(patched.matches("model_provider = \"deectx\"").count(), 1);
        assert!(!patched.contains("model_provider = \"openai\""));
    }

    #[test]
    fn codex_patch_reuses_existing_deectx_table() {
        let original = "model = \"gpt-4o\"\n[model_providers.deectx]\nname = \"Custom\"\nbase_url = \"http://127.0.0.1:8787\"\nwire_api = \"chat\"\nenv_key = \"OPENAI_API_KEY\"\n";
        let patched = patch_for(Tool::Codex, original).unwrap();

        assert_eq!(
            patched.matches("[model_providers.deectx]").count(),
            1,
            "existing deectx table must be reused, not duplicated: {patched}"
        );
        assert!(
            patched.contains("name = \"Custom\""),
            "existing table keys must be preserved: {patched}"
        );
        assert_eq!(patched.matches("model_provider = \"deectx\"").count(), 1);
        toml::from_str::<toml::Value>(&patched)
            .unwrap_or_else(|e| panic!("patched must stay valid TOML: {e}\n{patched}"));
    }

    #[test]
    fn codex_stale_partial_block_self_heals() {
        let stale = "[model_providers.deectx]\nbase_url = \"http://127.0.0.1:8787\"\nwire_api = \"chat\"\nenv_key = \"OPENAI_API_KEY\"\n";
        let dir = temp_dir("stale_codex");
        let path = dir.join("config.toml");
        std::fs::write(&path, stale).unwrap();

        assert!(
            !wired(Tool::Codex, stale),
            "block without the selection is NOT wired"
        );
        assert_eq!(
            patch_config(Tool::Codex, &path).unwrap(),
            PatchResult::Patched,
            "stale partial state must be patched (self-heal)"
        );
        let patched = std::fs::read_to_string(&path).unwrap();
        assert!(wired(Tool::Codex, &patched), "patched must be fully wired");
        assert_eq!(
            patched.matches("[model_providers.deectx]").count(),
            1,
            "no duplicate table after self-heal: {patched}"
        );
        toml::from_str::<toml::Value>(&patched)
            .unwrap_or_else(|e| panic!("self-healed config must parse as TOML: {e}\n{patched}"));

        assert_eq!(
            patch_config(Tool::Codex, &path).unwrap(),
            PatchResult::AlreadyPatched,
            "self-healed config is idempotent"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wired_requires_codex_selection() {
        let only_block = "[model_providers.deectx]\nbase_url = \"http://127.0.0.1:8787\"\n";
        let both = "model_provider = \"deectx\"\n[model_providers.deectx]\nbase_url = \"http://127.0.0.1:8787\"\n";

        assert!(
            !wired(Tool::Codex, only_block),
            "Codex needs the block AND the top-level selection"
        );
        assert!(wired(Tool::Codex, both));
        assert!(
            wired(Tool::ClaudeCode, only_block),
            "non-Codex tools only need the block substring"
        );
    }

    #[test]
    fn plist_xml_contains_exe_and_serve() {
        let xml = plist_xml("/usr/local/bin/deectx");
        assert!(xml.contains("<string>/usr/local/bin/deectx</string>"));
        assert!(xml.contains("<string>serve</string>"));
        assert!(xml.contains("RunAtLoad"));
        assert!(xml.contains("com.deectx.proxy"));
    }

    #[test]
    fn systemd_unit_contains_execstart() {
        let unit = systemd_unit("/home/u/bin/deectx");
        assert!(unit.contains("ExecStart=/home/u/bin/deectx serve"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn windows_batch_contains_exe_and_serve() {
        let batch = windows_batch("C:\\bin\\deectx.exe");
        assert!(batch.contains("serve"));
        assert!(batch.contains("C:\\bin\\deectx.exe"));
    }

    #[test]
    fn unsupported_os_errors() {
        assert!(daemon_artifact_for("unsupported", "/bin/deectx").is_none());
        assert!(daemon_artifact_for("freebsd", "/bin/deectx").is_none());
        // supported OSes resolve to an artifact on this host
        assert!(daemon_artifact_for(std::env::consts::OS, "/bin/deectx").is_some());
    }

    #[cfg(windows)]
    #[test]
    fn install_then_uninstall_windows_roundtrip() {
        let dir = temp_dir("daemon_roundtrip");
        let path = dir.join("deectx.cmd");
        let content = windows_batch("C:\\bin\\deectx.exe");

        install_at(&path, &content).unwrap();
        assert!(path.exists(), "install must write the startup artifact");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("serve"));
        assert!(written.contains("C:\\bin\\deectx.exe"));

        uninstall_at(&path).unwrap();
        assert!(!path.exists(), "uninstall must remove the artifact");

        // idempotent: second uninstall is a no-op, no error
        uninstall_at(&path).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_locked_detects_oauth() {
        let dir = temp_dir("locked");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"oauth_account":{"org":1}}"#).unwrap();
        assert!(is_locked(Tool::ClaudeCode, &path));
        std::fs::write(&path, r#"{"env":{}}"#).unwrap();
        assert!(!is_locked(Tool::ClaudeCode, &path));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
