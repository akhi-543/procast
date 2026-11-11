// Quick AI: one Claude session per chat, held open so follow-ups keep their context and
// skip the process start. Runs on the host's main thread; no extension code is involved.
// See docs/ai-research.md for why each option is set.
import fs from "node:fs";
import path from "node:path";

const SYSTEM = `You answer quick questions in procast, a Windows launcher. Be direct and brief. Plain text, with short lists or fenced code blocks when they help. You have no tools. Text in <system-reminder> tags comes from the app, not the user.`;
// ponytail: a chat left open and unused is closed after this long (frees ~300 MB); the next
// question starts a new one without the earlier context.
const IDLE_MS = Number(process.env.PROCAST_AI_IDLE_MS) || 10 * 60 * 1000;

let session = null;

/** The answer text in one SDK stream message, or "". */
export function deltaText(m) {
  const e = m.type === "stream_event" ? m.event : null;
  return e?.type === "content_block_delta" && e.delta?.type === "text_delta" ? e.delta.text : "";
}

function failure(e) {
  if (e?.code === "ERR_MODULE_NOT_FOUND") return "AI is not installed: run npm install in exthost";
  return String(e?.message ?? e);
}

/** Ends the chat and its Claude process. */
export function stop() {
  const s = session;
  session = null;
  if (!s) return;
  clearTimeout(s.idle);
  s.ctl.abort();
  s.wake?.();
}

/** Starts the chat's Claude process if there is none; called early so it is ready by the first question. */
export function open(model, out) {
  if (session) return session;
  const s = (session = { ctl: new AbortController(), queue: [], turns: [], wake: null, idle: null, query: null });
  const touch = () => {
    clearTimeout(s.idle);
    s.idle = setTimeout(() => {
      if (session !== s || s.turns.length) return touch();
      stop();
      out({ t: "aiClosed" });
    }, IDLE_MS);
  };
  s.touch = touch;
  touch();
  // Each turn is {id, done}; the SDK answers them in order, one `result` per turn.
  const finish = (error) => {
    const turn = s.turns[0];
    if (turn && !turn.done) {
      turn.done = true;
      out({ t: "aiDone", id: turn.id, ...(error ? { error } : {}) });
    }
  };
  async function* input() {
    while (!s.ctl.signal.aborted) {
      while (s.queue.length) yield { type: "user", message: { role: "user", content: s.queue.shift() }, parent_tool_use_id: null };
      await new Promise((r) => (s.wake = r));
    }
  }
  (async () => {
    let error;
    try {
      const { query } = await import("@anthropic-ai/claude-agent-sdk");
      const cwd = path.join(process.env.APPDATA, "procast", "ai");
      fs.mkdirSync(cwd, { recursive: true });
      s.query = query({
        prompt: input(),
        options: {
          cwd,
          model,
          systemPrompt: SYSTEM,
          abortController: s.ctl,
          tools: [],
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
      for await (const m of s.query) {
        const text = deltaText(m);
        const turn = s.turns[0];
        if (text && turn && !turn.done) out({ t: "aiDelta", id: turn.id, text });
        // The result message trails the answer by seconds (post-turn summary), so the
        // answer counts as done at message_stop; the result then moves on to the next turn.
        if (m.type === "stream_event" && m.event?.type === "message_stop") finish();
        if (m.type === "result") {
          finish(m.is_error ? (m.result ?? m.errors?.join("; ") ?? m.subtype) : undefined);
          s.turns.shift();
        }
      }
    } catch (e) {
      if (!s.ctl.signal.aborted) error = failure(e);
    }
    // The process is gone: fail whatever was still waiting, unless we closed it ourselves.
    if (session === s) {
      session = null;
      clearTimeout(s.idle);
      while (s.turns.length) {
        finish(error ?? "Claude stopped");
        s.turns.shift();
      }
    }
    s.ctl.abort();
  })();
  return s;
}

/** Asks in the open chat (starting one if needed). A question asked mid-answer cuts that answer short. */
export function ask(id, prompt, model, out) {
  const s = open(model, out);
  if (s.turns.some((t) => !t.done)) s.query?.interrupt().catch(() => {});
  s.turns.push({ id, done: false });
  s.queue.push(prompt);
  s.touch();
  s.wake?.();
}
