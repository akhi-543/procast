// Quick AI: one streamed answer from Claude. Runs on the host's main thread; no extension
// code is involved. See docs/ai-research.md for why each option is set.
import fs from "node:fs";
import path from "node:path";

const SYSTEM = `You answer quick questions in procast, a Windows launcher. Be direct and brief. Plain text, with short lists or fenced code blocks when they help. You have no tools. Text in <system-reminder> tags comes from the app, not the user.`;

let current = null;

/** The answer text in one SDK stream message, or "". */
export function deltaText(m) {
  const e = m.type === "stream_event" ? m.event : null;
  return e?.type === "content_block_delta" && e.delta?.type === "text_delta" ? e.delta.text : "";
}

function failure(e) {
  if (e?.code === "ERR_MODULE_NOT_FOUND") return "AI is not installed: run npm install in exthost";
  return String(e?.message ?? e);
}

export function stop() {
  current?.abort();
  current = null;
}

export async function ask(id, prompt, model, out) {
  stop();
  const ctl = (current = new AbortController());
  let error;
  try {
    const { query } = await import("@anthropic-ai/claude-agent-sdk");
    const cwd = path.join(process.env.APPDATA, "procast", "ai");
    fs.mkdirSync(cwd, { recursive: true });
    const stream = query({
      prompt,
      options: {
        cwd,
        model,
        systemPrompt: SYSTEM,
        abortController: ctl,
        tools: [],
        maxTurns: 1,
        persistSession: false,
        includePartialMessages: true,
        // Typed text stays text: no /commands or @file expansion.
        verbatimPrompts: true,
        settingSources: [],
        // tools: [] alone still lets claude.ai connectors through on a subscription login.
        strictMcpConfig: true,
        env: { ...process.env, ENABLE_CLAUDEAI_MCP_SERVERS: "false", CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1", CLAUDE_CODE_MCP_STARTUP_WAIT_MS: "0" },
      },
    });
    for await (const m of stream) {
      const text = deltaText(m);
      if (text) out({ t: "aiDelta", id, text });
      if (m.type === "result" && m.is_error) error = m.result ?? m.errors?.join("; ") ?? m.subtype;
      // The result message trails the answer by seconds (post-turn summary), so stop here.
      if (m.type === "stream_event" && m.event?.type === "message_stop") break;
    }
  } catch (e) {
    if (!ctl.signal.aborted) error = failure(e);
  }
  if (current === ctl) current = null;
  ctl.abort();
  out({ t: "aiDone", id, ...(error ? { error } : {}) });
}
