use anyhow::Result;
use clap::{Parser, Subcommand};
use deectx::config::Config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "deectx", about = "Local PII-masking proxy for AI tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the local proxy
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Summarize the hash-only ledger for audit / DPIA reporting
    Audit {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        #[arg(long)]
        today: bool,
        #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "-")]
        export: Option<String>,
    },
    /// Query the running proxy's live /stats endpoint
    Status {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Auto-wire installed AI tools to the local proxy
    Setup,
    /// Verify which tools are wired to the proxy
    Doctor,
    /// Restore original configs from .bak backups
    Unwrap,
    /// Install the autostart daemon so the proxy runs at login
    DaemonInstall,
    /// Remove the autostart daemon
    DaemonUninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { config } => {
            let cfg = if config.exists() {
                Config::load(&config)?
            } else {
                Config::default()
            };
            deectx::proxy::run_proxy(cfg).await?;
        }
        Cmd::Audit {
            config,
            today,
            export,
        } => {
            let cfg = Config::load(&config).unwrap_or_default();
            let summary = if today {
                deectx::audit::summarize_for_date(
                    &cfg.ledger_path,
                    chrono::Utc::now().date_naive(),
                )?
            } else {
                let entries = deectx::ledger::Ledger::read_all(&cfg.ledger_path)?;
                deectx::audit::summarize(&entries, "all")
            };
            let json = serde_json::to_string_pretty(&summary)?;
            match export {
                None => {
                    println!("deeCtx audit — {}", summary.date);
                    println!("  requests: {}", summary.total_requests);
                    println!("  masked events: {}", summary.masked_events);
                    println!("  redacted events: {}", summary.redacted_events);
                    println!("  alerts: {}", summary.alerts);
                    println!("  distinct sessions: {}", summary.distinct_sessions);
                    for (k, v) in &summary.entities {
                        println!("  entity {k}: {v}");
                    }
                    for (k, v) in &summary.packs {
                        println!("  pack {k}: {v}");
                    }
                }
                Some(p) if p == "-" => println!("{json}"),
                Some(p) => std::fs::write(&p, json)?,
            }
        }
        Cmd::Status { config, json } => {
            let cfg = Config::load(&config).unwrap_or_default();
            let url = format!("http://{}/stats", cfg.listen);
            let body = match reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()?
                .get(&url)
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.text())
            {
                Ok(b) => b,
                Err(e) => {
                    anyhow::bail!(
                        "deectx status: could not reach proxy at {url} (is `deectx serve` running?) — {e}"
                    );
                }
            };
            if json {
                println!("{body}");
            } else {
                println!("{}", deectx::status::format_status(&body)?);
            }
        }
        Cmd::Setup => {
            let found = deectx::setup::discover();
            if found.is_empty() {
                println!("deectx setup: no installed tools found to wire");
            }
            for (tool, path) in &found {
                if deectx::setup::is_locked(*tool, path) {
                    println!("{tool:?}: locked OAuth provider, cannot intercept");
                    continue;
                }
                match deectx::setup::patch_config(*tool, path) {
                    Err(e) => {
                        eprintln!("failed to patch {tool:?}: {e}");
                        continue;
                    }
                    Ok(deectx::setup::PatchResult::AlreadyPatched) => {
                        println!("{tool:?}: already wired")
                    }
                    Ok(deectx::setup::PatchResult::Patched) => {
                        println!("{tool:?}: patched -> {}", path.display())
                    }
                }
            }
            if let Err(e) = deectx::setup::install_daemon() {
                println!("daemon install skipped: {e}");
            } else {
                println!("autostart daemon installed; proxy will start at login");
            }
            println!("proxy listen URL: http://{}", Config::default().listen);
            println!("done; start the proxy: deectx serve");
        }
        Cmd::Doctor => {
            println!("{}", deectx::setup::doctor()?);
        }
        Cmd::Unwrap => {
            deectx::setup::unwrap()?;
            println!("restored all original configs");
        }
        Cmd::DaemonInstall => {
            deectx::setup::install_daemon()?;
            println!("autostart daemon installed");
        }
        Cmd::DaemonUninstall => {
            deectx::setup::uninstall_daemon()?;
            println!("autostart daemon removed");
        }
    }
    Ok(())
}
