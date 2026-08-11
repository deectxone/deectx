use anyhow::Result;
use clap::{Parser, Subcommand};
use deectx::config::Config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "deectx", about = "Local PII-masking proxy for AI tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Turn deeCtx on: wire tools, install autostart, start masking
    Start,
    /// Turn deeCtx off: restore tools to direct API, stop the proxy
    Stop,
    /// Remove deeCtx: stop + restore tools + optionally delete data
    Uninstall {
        /// Also delete config + ledger without prompting
        #[arg(long)]
        purge: bool,
    },
    /// Start the local proxy (run by the autostart daemon / `start`)
    Serve {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Summarize the hash-only ledger for audit / DPIA reporting
    Audit {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        today: bool,
        #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "-")]
        export: Option<String>,
    },
    /// Query the running proxy's live /stats endpoint
    Status {
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Alias of `start` (auto-wire installed AI tools + start)
    Setup,
    /// Verify which tools are wired to the proxy
    Doctor,
    /// Alias of `stop` (restore original configs from .bak backups)
    Unwrap,
    /// Install the autostart daemon so the proxy runs at login
    DaemonInstall,
    /// Remove the autostart daemon
    DaemonUninstall,
}

/// Load config if it exists, else fall back to defaults. Unlike Serve, this is
/// shared by read-only subcommands (audit/status) so a malformed config fails
/// loudly instead of being silently ignored.
fn load_config(config: &std::path::Path) -> Result<Config> {
    if config.exists() {
        Ok(Config::load(config)?)
    } else {
        Ok(Config::default())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let pm = deectx::lifecycle::OsProcessManager;
    match cli.cmd {
        None => {
            let report = deectx::lifecycle::status(&pm)?;
            println!("{}", deectx::lifecycle::render_status(&report));
        }
        Some(Cmd::Start) | Some(Cmd::Setup) => {
            let report = deectx::lifecycle::start(&pm)?;
            println!("{}", deectx::lifecycle::render_status(&report));
        }
        Some(Cmd::Stop) | Some(Cmd::Unwrap) => {
            let warnings = deectx::lifecycle::stop(&pm)?;
            if warnings.is_empty() {
                println!("deeCtx stopped; tools restored to direct API access.");
            } else {
                println!("deeCtx stopped, but some tools were NOT restored:");
                for w in &warnings {
                    println!("  ⚠ {w}");
                }
            }
        }
        Some(Cmd::Uninstall { purge }) => {
            let delete = purge || {
                use std::io::Write;
                print!("Also delete config + ledger (audit data)? [y/N] ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            };
            let warnings = deectx::lifecycle::uninstall(&pm, delete)?;
            for w in &warnings {
                println!("  ⚠ {w}");
            }
            println!(
                "deeCtx removed. To remove the binary: `scoop uninstall deectx` (or brew/cargo)."
            );
        }
        Some(Cmd::Serve { config }) => {
            let config = config.unwrap_or_else(deectx::home::config_path);
            let cfg = if config.exists() {
                Config::load(&config)?
            } else {
                Config::default()
            };
            deectx::proxy::run_proxy(cfg).await?;
        }
        Some(Cmd::Audit {
            config,
            today,
            export,
        }) => {
            let config = config.unwrap_or_else(deectx::home::config_path);
            let cfg = load_config(&config)?;
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
        Some(Cmd::Status { config, json }) => {
            let config = config.unwrap_or_else(deectx::home::config_path);
            let cfg = load_config(&config)?;
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
        Some(Cmd::Doctor) => {
            println!("{}", deectx::setup::doctor()?);
        }
        Some(Cmd::DaemonInstall) => {
            deectx::setup::install_daemon()?;
            println!("autostart daemon installed");
        }
        Some(Cmd::DaemonUninstall) => {
            deectx::setup::uninstall_daemon()?;
            println!("autostart daemon removed");
        }
    }
    Ok(())
}
