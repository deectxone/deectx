//! Windows/macOS tray icon: shows whether deeCtx is actively masking, opens
//! the local dashboard, and starts/stops masking without a terminal. Lives
//! inside the proxy process itself — see
//! `docs/superpowers/specs/2026-08-12-tray-icon-design.md`.

use crate::tray::icon::{COLOR_ACTIVE, COLOR_STOPPED, COLOR_WARNING};

pub(crate) mod icon;
pub(crate) mod proxy_handle;

/// What the tray icon/menu currently show. Kept separate from the GUI crates
/// so the mapping from state to color/label is unit-testable without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayState {
    Active,
    Stopped,
    /// Stopped, but the last stop left a restore warning to surface.
    Warning(String),
}

pub(crate) fn color_for(state: &TrayState) -> [u8; 3] {
    match state {
        TrayState::Active => COLOR_ACTIVE,
        TrayState::Stopped => COLOR_STOPPED,
        TrayState::Warning(_) => COLOR_WARNING,
    }
}

pub(crate) fn toggle_label_for(state: &TrayState) -> &'static str {
    match state {
        TrayState::Active => "Stop Masking",
        TrayState::Stopped | TrayState::Warning(_) => "Start Masking",
    }
}

pub(crate) fn tooltip_for(state: &TrayState) -> String {
    match state {
        TrayState::Active => "deeCtx — masking active".to_string(),
        TrayState::Stopped => "deeCtx — masking stopped".to_string(),
        TrayState::Warning(w) => format!("deeCtx — stopped with a warning: {w}"),
    }
}

use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{TrayIcon, TrayIconBuilder};

fn load_config() -> anyhow::Result<crate::config::Config> {
    let path = crate::home::config_path();
    if path.exists() {
        Ok(crate::config::Config::load(&path)?)
    } else {
        Ok(crate::config::Config::default())
    }
}

fn build_icon(state: &TrayState) -> anyhow::Result<tray_icon::Icon> {
    let rgba = icon::render_circle_rgba(color_for(state));
    Ok(tray_icon::Icon::from_rgba(
        rgba,
        icon::ICON_SIZE,
        icon::ICON_SIZE,
    )?)
}

/// Entry point for `deectx tray`. Owns the calling thread (the native tray
/// event loop requires this, especially on macOS) — callers must invoke this
/// directly from `main()`, never from inside a Tokio runtime. Falls back to
/// plain headless `proxy::run_proxy` if the tray/display setup itself fails
/// (no session available, backend unavailable, etc.) rather than failing the
/// whole command — the proxy must work even where the icon can't render.
pub fn run() -> anyhow::Result<()> {
    let cfg = load_config()?;

    match try_run_with_tray(cfg.clone()) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!(
                "tray: could not start ({e}); falling back to headless (no icon, masking still works)"
            );
            tokio::runtime::Runtime::new()?.block_on(crate::proxy::run_proxy(cfg))
        }
    }
}

/// The actual tray/menu/event-loop implementation, isolated from `run()` so
/// a setup failure can be caught and turned into the headless fallback above
/// instead of propagating out of the whole `deectx tray` command.
fn try_run_with_tray(cfg: crate::config::Config) -> anyhow::Result<()> {
    let dashboard_url = format!("http://{}/", cfg.listen);

    let menu = Menu::new();
    let open_item = MenuItem::new("Open Dashboard", true, None);
    let toggle_item = MenuItem::new(toggle_label_for(&TrayState::Active), true, None);
    let quit_item = MenuItem::new("Quit deeCtx", true, None);
    menu.append(&open_item)?;
    menu.append(&toggle_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let mut state = TrayState::Active;
    let mut handle = proxy_handle::ProxyHandle::start(cfg);
    let tray_icon: TrayIcon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip_for(&state))
        .with_icon(build_icon(&state)?)
        .build()?;

    let menu_channel = MenuEvent::receiver();
    let event_loop = EventLoopBuilder::new().build();

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(250),
        );

        let Ok(event) = menu_channel.try_recv() else {
            return;
        };

        if event.id == open_item.id() {
            if matches!(state, TrayState::Active) {
                let _ = open::that(&dashboard_url);
            }
        } else if event.id == toggle_item.id() {
            state = if handle.is_running() {
                let warnings = handle.stop();
                if let Some(first) = warnings.into_iter().next() {
                    TrayState::Warning(first)
                } else {
                    TrayState::Stopped
                }
            } else {
                handle = proxy_handle::ProxyHandle::start(load_config().unwrap_or_default());
                TrayState::Active
            };
            toggle_item.set_text(toggle_label_for(&state));
            open_item.set_enabled(matches!(state, TrayState::Active));
            tray_icon.set_tooltip(Some(tooltip_for(&state))).ok();
            if let Ok(icon) = build_icon(&state) {
                tray_icon.set_icon(Some(icon)).ok();
            }
        } else if event.id == quit_item.id() {
            if handle.is_running() {
                let warnings = handle.stop();
                for w in &warnings {
                    tracing::warn!("tray: {w}");
                }
            }
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn active_state_is_green_and_offers_stop() {
        assert_eq!(color_for(&TrayState::Active), COLOR_ACTIVE);
        assert_eq!(toggle_label_for(&TrayState::Active), "Stop Masking");
    }

    #[test]
    fn stopped_state_is_grey_and_offers_start() {
        assert_eq!(color_for(&TrayState::Stopped), COLOR_STOPPED);
        assert_eq!(toggle_label_for(&TrayState::Stopped), "Start Masking");
    }

    #[test]
    fn warning_state_is_amber_and_offers_start_with_the_warning_in_the_tooltip() {
        let state = TrayState::Warning("ClaudeCode config could not be restored".into());
        assert_eq!(color_for(&state), COLOR_WARNING);
        assert_eq!(toggle_label_for(&state), "Start Masking");
        assert!(tooltip_for(&state).contains("ClaudeCode config could not be restored"));
    }
}
