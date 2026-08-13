//! Windows/macOS tray icon: shows whether deeCtx is actively masking, opens
//! the local dashboard, and starts/stops masking without a terminal. Lives
//! inside the proxy process itself — see
//! `docs/superpowers/specs/2026-08-12-tray-icon-design.md`.

pub(crate) mod icon;
pub(crate) mod proxy_handle;
