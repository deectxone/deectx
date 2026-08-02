# deeCtx shims

Enforce that AI coding tools route through the deeCtx masking proxy, failing closed
when it is not running.

## Cursor

1. Copy `shims/cursor/hooks.json` to your project as `.cursor/hooks.json`.
2. Copy `shims/cursor/deectx-gate.sh` to `.cursor/hooks/deectx-gate.sh` and make it
   executable (`chmod +x .cursor/hooks/deectx-gate.sh`).
3. Set `DEECTX_URL` in your shell if the proxy is not at `http://127.0.0.1:8787`.
   Start the proxy: `deectx serve --config config.toml`.

The `preToolUse` hook runs for `Shell|Read|Write|Edit` and, with `failClosed: true`,
blocks the action whenever the proxy is unreachable.

## opencode

1. Copy `shims/opencode/opencode.json` into your `~/.config/opencode/opencode.json`
   (or merge the `provider.deectx` block into the existing config) to add the
   `deectx` provider.
2. Copy `shims/opencode/deectx-plugin.ts` to `~/.config/opencode/plugins/deectx-plugin.ts`.
   The plugin injects `OPENAI_BASE_URL`/`ANTHROPIC_BASE_URL` into tool environments
   and blocks tool execution when the proxy is unreachable.