use serde_json::Value;

#[test]
fn release_artifacts_exist_and_are_valid() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");

    let brew = std::fs::read_to_string(root.join("install/brew/deectx.rb")).unwrap();
    assert!(
        brew.contains("bin.install \"deectx\""),
        "brew formula must install the prebuilt release binary"
    );
    assert!(
        brew.contains(&format!("version \"{version}\"")),
        "brew formula must pin the current crate version {version}"
    );

    let scoop = std::fs::read_to_string(root.join("install/scoop/deectx.json")).unwrap();
    let v: Value = serde_json::from_str(&scoop).expect("scoop manifest must be valid JSON");
    assert_eq!(v["bin"], "deectx.exe");
    assert_eq!(
        v["version"], version,
        "scoop manifest must track the crate version"
    );
    let hash = v["architecture"]["64bit"]["hash"].as_str().unwrap();
    assert_eq!(
        hash.len(),
        64,
        "release zip sha256 must be populated (64 hex chars)"
    );

    assert!(root.join("scripts/release.ps1").exists());
    assert!(root.join("scripts/release.sh").exists());

    let cfg = std::fs::read_to_string(root.join("config.example.toml")).unwrap();
    assert!(
        cfg.contains("ledger_retention_days"),
        "example config must document retention"
    );
}

#[test]
fn example_config_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cfg = deectx::config::Config::load(&root.join("config.example.toml")).unwrap();
    assert_eq!(cfg.ledger_retention_days, 90);
}
