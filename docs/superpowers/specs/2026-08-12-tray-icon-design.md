# deeCtx — Tray Icon — Design

*Date: 2026-08-12 · Status: approved design, pre-implementation*

## Problem

deeCtx runs invisibly today: `deectx start` wires tools and spawns `deectx
serve` as a headless background process with no on-screen presence at all.
Nothing on the desktop tells you deeCtx is protecting you, and there's no
lightweight way to check status or turn it off short of a terminal. Field
feedback: users want "at least something there" — a taskbar/menu-bar icon
that shows running vs. stopped at a glance, opens the local dashboard on
click, and lets you stop the proxy without a terminal.

## Goals

- A tray icon (Windows) / menu-bar icon (macOS) that visibly indicates
  whether deeCtx is actively masking.
- Click through to the existing local dashboard (`GET /` — shipped
  separately, reused here unchanged).
- Stop/start masking from the tray, using the exact same tool-unwiring path
  as `deectx stop` today (including its recent fixes: partial-failure
  reporting, opencode fail-closed plugin cleanup).
- `deectx start`'s autostart entry launches this instead of bare `serve` on
  Windows/macOS, so the icon appears automatically after login — that's the
  actual UX win being asked for.

## Non-goals

- Linux tray support. `deectx serve` (headless, unchanged) remains Linux's
  only mode; no tray attempt, no new Linux dependency.
- Any new masking/detection behavior. This is a pure control-surface feature
  over functionality that already exists (dashboard, `lifecycle::stop`).
- A settings/preferences UI in the tray. Config stays TOML-file-based.
- IPC or a second always-on supervisor process. The tray process *is* the
  proxy process (see Architecture) — no new process class introduced.

## UX model

Two visible tray states:

| State | Icon | Menu |
|---|---|---|
| Masking ON | active/colored D-mark | "Open Dashboard" · **Stop Masking** · Quit deeCtx |
| Masking OFF | dimmed/outline D-mark | "Open Dashboard" (disabled) · **Start Masking** · Quit deeCtx |

- **Open Dashboard** — opens `http://127.0.0.1:<port>/` in the default
  browser (via the `open` crate). Disabled when masking is off (nothing
  running to show).
- **Stop Masking / Start Masking** — a single context-sensitive toggle item,
  not two separate ones.
- **Quit deeCtx** — full teardown: stop (if running) + close the tray + exit
  the process. The autostart artifact is untouched, so the icon returns next
  login.
- A partial-stop failure (a tool config couldn't be restored — the same
  class of failure `lifecycle::stop` already detects and reports as
  `Vec<String>` warnings) surfaces as a distinct "warning" icon state rather
  than silently showing "stopped" as if everything succeeded.

## Architecture

Approach: **the tray lives inside the proxy process itself** — a new
long-lived run mode, not a separate helper talking over IPC. One process,
one source of truth for status.

### 1. `src/tray.rs` (new)

- Owns tray icon/menu construction and the OS event loop only. Contains no
  masking or proxy logic — calls into existing `lifecycle`/`proxy` code for
  everything that isn't tray-specific.
- On launch: spawns the existing masking-proxy logic (same routes as
  `serve`: `/healthz`, `/stats`, `/audit/today`, dashboard, completions) on a
  background thread running its own `tokio` runtime; the native tray event
  loop owns the main thread (required by macOS/AppKit).
- Menu actions call `lifecycle::stop`/an in-process equivalent of
  `lifecycle::start`'s wiring step, updating icon state from the result.
- If tray/display init fails (no session available — e.g. Windows Server
  Core, a non-interactive SSH session), log a warning and fall back to
  running exactly like plain `serve`: headless, no icon, proxy still works.
  The proxy must never fail to start because the tray couldn't render.

### 2. `src/main.rs`

- New `Cmd::Tray` subcommand → `tray::run()`. Plain `deectx serve` is
  unchanged and still available directly (used by Linux, and by anyone who
  wants headless-only on any OS).

### 3. `src/lifecycle.rs`, `src/setup.rs`

- `daemon_artifact_for("windows" | "macos", exe)` changes its generated
  artifact to launch `<exe> tray` instead of `<exe> serve`.
- `daemon_artifact_for("linux", exe)` is unchanged (`<exe> serve`).
- No change to `start`/`stop`/`uninstall`'s external contract — they still
  work identically whether the running process is `serve` or `tray`, since
  both expose the same pidfile + HTTP surface.

### 4. New dependencies

- `tray-icon` + `muda` (native tray icon and menu, no full GUI framework —
  same crates Tauri uses under the hood).
- `open` (launch the dashboard URL in the user's default browser).
- Added unconditionally to `[dependencies]` (matches how `stats_enabled`
  already works — always compiled in, runtime-conditional behavior) rather
  than a Cargo feature flag. This does add these as build dependencies for
  the Linux release target too, even though Linux never invokes tray code at
  runtime; acceptable since none of the three require Linux-only system
  libraries at *link* time (only `tray-icon` itself would if a Linux
  backend were enabled, which it explicitly won't be — see Icon Assets).

### Icon assets

Reuse the deeCtx D-mark brand glyph (already used for the dashboard/favicon,
`src/dashboard.html`) rendered at platform-appropriate sizes:
- Windows: `.ico` (multi-resolution), colored (active) and greyscale/dimmed
  (stopped) variants.
- macOS: template image (monochrome, OS handles light/dark menu-bar
  rendering automatically) for the base shape; state is distinguished by
  opacity (full for active, ~40% for stopped) on the same glyph — no
  dot-badge or shape change, since template images can't carry color and a
  badge would need extra compositing the simpler opacity approach avoids.
- Assets live under a new top-level `assets/tray/` directory (sibling to
  `install/`, `shims/` — non-Rust-source content stays out of `src/`),
  embedded via `include_bytes!("../assets/tray/…")` from `src/tray.rs` (same
  self-contained-binary pattern as `dashboard.html`'s `include_str!`).

## Data flow

1. `deectx start` (or login, via the now-updated autostart artifact) → tools
   wired → `deectx tray` launched instead of bare `serve`.
2. `tray` starts the masking proxy on a background thread, shows the
   "masking ON" icon.
3. User clicks the icon → OS-native menu appears → **Open Dashboard** opens
   the browser to the already-shipped dashboard page; no new dashboard code.
4. User clicks **Stop Masking** → tray runs the same tool-unwiring path as
   `deectx stop` (in-process call, not a shelled-out CLI invocation) and
   closes the proxy listener. Tray process stays alive; icon flips to
   "masking OFF"; menu item flips to **Start Masking**.
5. User clicks **Quit deeCtx** → stop (if not already) + tray event loop
   exits + process exits. Autostart artifact remains installed.

## Error handling

- Tray/display init failure at launch → warn + continue headless (proxy
  still starts and works; no icon). Never block proxy startup on tray
  failure.
- Stop-from-tray partial failure (tool config restore fails, e.g. a locked
  file) → icon shows a distinct warning state; menu surfaces the specific
  warning text (same strings `lifecycle::stop` already produces for the
  CLI) rather than just "stopped."
- Tray process crash (e.g. a menu-library panic) → out of scope to catch
  from inside the crashing process; the pidfile/`is_deectx` staleness
  detection `lifecycle::start` already has handles recovery on next
  `start`, same as any other unexpected proxy exit today.

## Testing

- **Unit-testable (my responsibility, will be covered):** the icon/menu
  state-transition logic as a pure function of proxy state
  (running/stopped/warning) — same pattern as `StatusReport`/
  `render_status`; `daemon_artifact_for`'s updated Windows/macOS output
  (`<exe> tray` instead of `<exe> serve`) with Linux's untouched.
- **Integration-testable (my responsibility, will be covered):** `deectx
  tray`'s embedded proxy exposes the same routes as `serve`, so the existing
  `proxy_integration.rs` suite continues to validate masking behavior
  unchanged when driven against it.
- **Not testable by me, manual verification required by the user post-ship:**
  the actual rendered icon appearance, OS-native menu click behavior, and
  all macOS behavior (no display/session reachable from the implementation
  environment, and no macOS build/test capability at all).

## Backward compatibility

- `deectx serve` unchanged — still the direct, headless entry point; still
  what Linux autostart uses; still usable manually on any OS by anyone who
  doesn't want a tray icon.
- Existing installs: the new tray behavior only takes effect after a
  `deectx start` re-run (which refreshes the autostart artifact) following
  an upgrade — no forced migration, no change until the user re-runs start.

## Out of scope (tracked elsewhere)

- Linux tray support (may follow later if a clean cross-desktop approach
  emerges).
- A way to disable the tray and keep autostart headless on Windows/macOS
  (today: use `deectx serve` manually + your own autostart entry if you want
  that; no dedicated config flag for it in this iteration).
