use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    Opencode,
}

impl Tool {
    /// Where the tool stores its user-level config. Derives the home dir from
    /// the environment each call (Windows `USERPROFILE`, unix `HOME`).
    pub fn config_path(&self) -> Option<PathBuf> {
        let home = std::env::var("USERPROFILE")
            .ok()
            .or_else(|| std::env::var("HOME").ok())?;
        let home = PathBuf::from(home);
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
    if original.contains("127.0.0.1:8787") {
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
            let block = &[
                "[model_providers.deectx]",
                "name = \"deeCtx proxy\"",
                &format!("base_url = \"{base}\""),
                "wire_api = \"chat\"",
                "env_key = \"OPENAI_API_KEY\"",
                "",
            ]
            .join("\n");
            Ok(format!("{}\n{block}", original.trim_end()))
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

/// Verify which installed tools are wired to the proxy.
pub fn doctor() -> Result<String> {
    let mut lines = Vec::new();
    let found = discover();
    if found.is_empty() {
        lines.push("deeCtx setup status: no tools found to check".to_string());
    }
    for (tool, path) in found {
        let content = std::fs::read_to_string(&path)?;
        let ok = content.contains("127.0.0.1:8787");
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
