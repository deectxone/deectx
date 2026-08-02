import { type Plugin } from "@opencode-ai/plugin";

const PROXY = process.env.DEECTX_URL ?? "http://127.0.0.1:8787";

export const Deectx: Plugin = async ({ client }) => {
  return {
    "shell.env": async (_input, output) => {
      // Route model traffic through the deeCtx masking proxy.
      output.env.OPENAI_BASE_URL = `${PROXY}/v1`;
      output.env.ANTHROPIC_BASE_URL = `${PROXY}/v1`;
    },
    "tool.execute.before": async (input, _output) => {
      // Fail closed: refuse to run tools when the proxy is unreachable.
      const ok = await fetch(`${PROXY}/healthz`, { signal: AbortSignal.timeout(2000) })
        .then((r) => r.ok)
        .catch(() => false);
      if (!ok) {
        throw new Error("deeCtx proxy is not running; masking cannot be guaranteed.");
      }
    },
  };
};

export default Deectx;