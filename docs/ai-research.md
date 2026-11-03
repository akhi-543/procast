# AI in akscast via Claude Code: research (2026-11-03)

Research input for a later phase. Nothing here is implemented. A research subagent compiled it from Anthropic's docs.

Legend: **D** = documented on a cited Anthropic page. **I** = inference, to verify in the spike. The AUP quote came through a summarizer; check its exact wording before relying on it. Flag and version details (e.g. `--restricted` from v2.1.248, `permission_policy` from SDK v0.3.301) come from the cited pages and should be re-checked against the installed version.

## Summary

Use the **Claude Agent SDK (TypeScript)** inside the existing Node sidecar as the AI engine. Expose akscast's own tools as an in-process SDK MCP server, and remove all built-in Claude Code tools so the model can only call akscast tools.

The SDK provides sessions, streaming, interrupt, permission callbacks and MCP, so there is no harness to build. Use `claude -p` for scripts and tests. Use the raw Messages API only if you want to drop Claude Code's harness and pay per token.

**Personal use with your own claude.ai login is workable. Distribution is the blocker.** Anthropic's terms do not let third-party apps offer claude.ai login or route Pro/Max credentials for their users. Sharing akscast needs per-user API keys (or each user signing into Claude Code themselves), or prior written approval.

No latency figures are published, so measure in a spike.

## 1. Integration options

### (a) CLI print mode (`claude -p`)

| Aspect | Details | Source |
|---|---|---|
| Process model | One process per run; exit code 0 on success, non-zero on failure | **D** [headless](https://code.claude.com/docs/en/headless.md) |
| Long-lived input | `--input-format stream-json --output-format stream-json --verbose` | **D** |
| | The stdin message framing isn't spelled out on the CLI pages; verify it or use the SDK | **I** |
| Streaming | `--include-partial-messages`; `system/init` is the first event and `result` the last | **D** |
| Sessions | `--continue`, `--resume <id\|name\|file.jsonl>`, `--fork-session`, `--session-id <uuid>`, `--no-session-persistence` | **D** [CLI reference](https://code.claude.com/docs/en/cli-reference.md) |
| | Transcripts are stored in `~/.claude/projects/<encoded-cwd>/*.jsonl`; `--resume <id>` searches all projects | **D** [sessions](https://code.claude.com/docs/en/agent-sdk/sessions.md) |
| Tools | `--tools ""` removes all built-ins | **D** |
| | `--allowedTools` / `--disallowedTools` take permission rules; a bare name removes the tool | **D** |
| | `--permission-mode dontAsk`, `--permission-prompt-tool mcp__<server>__<tool>`, `--permission-prompts none` | **D** |
| | `--restricted` removes command-running tools and WebFetch, confines file tools to the working directories, and refuses `bypassPermissions` | **D** |
| MCP | `--mcp-config`, `--strict-mcp-config` | **D** |
| | In-process tools exist only in the SDK, so a CLI-only design needs a standalone MCP server process | **I** |
| Other flags | `--append-system-prompt[-file]`, `--system-prompt[-file]`, `--model` (aliases `sonnet` / `opus` / `haiku` / `fable`), `--max-turns`, `--max-budget-usd` | **D** |
| Cancel | SIGINT ends the turn; SIGTERM exits with 143 and leaves the turn unrecorded | **D** |
| | Behaviour of these signals on Windows is unverified | **I** |

### (b) Agent SDK (TypeScript `@anthropic-ai/claude-agent-sdk`, Python `claude-agent-sdk`)

| Aspect | Details | Source |
|---|---|---|
| Requirements | Node 18.14+ or Python 3.10+; both bundle the native Claude Code binary | **D** [quickstart](https://code.claude.com/docs/en/agent-sdk/quickstart.md) |
| Process model | A library that spawns one `claude` subprocess per session over stdio; the subprocess owns the cwd and transcripts | **D** [hosting](https://code.claude.com/docs/en/agent-sdk/hosting.md) |
| | Running it from the sidecar fits; spawning a child process from a `worker_thread` needs confirming | **I** |
| Streaming input | A persistent session with queued messages, interruption and image attachments | **D** [streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode.md) |
| | Single-message mode has no real-time interruption | **D** |
| Sessions | `continue`, `resume`, `forkSession`, `listSessions()`, `getSessionMessages()`, `renameSession()`, `tagSession()`; `persistSession: false` is TypeScript only | **D** |
| Tools and permissions | Built-ins plus custom tools, `allowedTools` / `disallowedTools` / `tools`, `permissionMode`, `canUseTool`, hooks | **D** [permissions](https://code.claude.com/docs/en/agent-sdk/permissions.md) |
| Cancel | `interrupt()`; `canUseTool` receives an `AbortSignal` in TypeScript | **D** [user input](https://code.claude.com/docs/en/agent-sdk/user-input.md) |
| Windows | Git for Windows is recommended for the Bash tool; otherwise PowerShell is used | **D** [setup](https://code.claude.com/docs/en/setup.md) |
| | No native-Windows sandboxing (WSL 2 has it) | **D** |
| | ARM64 Windows gets a Python source distribution with no bundled binary | **D** |
| | `npm ci --omit=optional` installs no binary | **D** |

### (c) Messages API (direct, API key)

| Aspect | Details | Source |
|---|---|---|
| Streaming | HTTPS with `"stream": true` returns SSE events for text, tool-use and thinking deltas | **D** [streaming](https://platform.claude.com/docs/en/build-with-claude/streaming.md) |
| Tools | None built in: you write the tool loop yourself or use the SDK tool runner | **D** [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview.md) |
| Sessions | No server-side session: you store and resend history | **I** |
| Cancel | Close the HTTP stream | **I** |
| Trade-off | You rebuild permissions, sessions, MCP and the agent loop yourself | — |

## 2. Auth, billing, terms

**Personal use** (documented):
- A `/login` claude.ai subscription (Pro/Max/Ultra) is the default credential when no API key or token is set. **D** [authentication](https://code.claude.com/docs/en/authentication.md)
- `--bare` never reads OAuth or the keychain and needs `ANTHROPIC_API_KEY`. **D**
- `claude setup-token` creates a one-year `CLAUDE_CODE_OAUTH_TOKEN`. It is ignored in `--bare`. **D**
- Pro/Max limits "assume ordinary, individual usage of Claude Code and the Agent SDK." **D** [legal and compliance](https://code.claude.com/docs/en/legal-and-compliance.md)

**Terms:**
- Legal page: "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication through Claude Console or a supported cloud provider. Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, Max, or Ultra plan credentials on behalf of their users." Developers also "may not collect, store, or intermediate Claude.ai credentials or session tokens." **D**
- Agent SDK overview: "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK." **D**
- Products that ship Claude Code: "Each end user must authenticate with their own Anthropic API key, Claude subscription plan credentials, or 3P inference provider credential." The binary must not be modified, and "Claude Code" may not appear in the product name. **D**
- AUP, "Do Not Abuse Our Platform": prohibits "Resell, proxy, or otherwise provide access to Claude through unauthorized means." **D** (summarized; verify wording) [AUP](https://www.anthropic.com/legal/aup)
- Unresolved: the Agent SDK overview says Commercial Terms apply to products built on it, while the legal page lists Consumer Terms for Pro/Max users. Ask Anthropic sales. **D**/**I**

**Reading** (inferred):
- Personal use on your own login is fine.
- To distribute: use a per-user API key, or have each user install Claude Code and sign in themselves.
- akscast must never hold anyone's claude.ai tokens.
- Get written approval before anything beyond that.

## 3. Latency

- No time-to-first-token or cold-start figures were found. **D** (absence) Measure in the spike.
- **Cold path:** spawn the CLI, load CLAUDE.md, hooks, skills and plugins, then connect MCP. The first turn waits up to 30 s (`MCP_TIMEOUT`) for pending stdio servers and in-process SDK servers. **D**
- **Trimming startup with an API key:** `--bare` skips hooks, skills, plugins, MCP, auto memory and CLAUDE.md. **D**
- **Trimming startup on a subscription:** use `settingSources: []`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `CLAUDE_CODE_MCP_STARTUP_WAIT_MS=0` and `ENABLE_CLAUDEAI_MCP_SERVERS=false`. The global `~/.claude.json` is still read. **D** [settings features](https://code.claude.com/docs/en/agent-sdk/claude-code-features.md), [MCP](https://code.claude.com/docs/en/agent-sdk/mcp.md)
- **Keeping it warm:** one subprocess per session via streaming input or `ClaudeSDKClient`. TypeScript `startup()` / `prewarm()` pre-spawn subprocesses. **D** [hosting](https://code.claude.com/docs/en/agent-sdk/hosting.md)
- **Prompt cache:** affects prompt reprocessing, not spawn time. The TTL is 1 hour on a subscription within plan usage and 5 minutes on an API key, and each hit resets it. Switching models or effort mid-session misses the cache. **D** [prompt caching](https://code.claude.com/docs/en/prompt-caching.md)
- **Quick AI:** keep one pre-spawned process with `tools: []` and `maxTurns: 1`. **I**

## 4. Mapping Raycast AI features

| Raycast feature | Claude mapping | Notes |
|---|---|---|
| Quick AI | One-shot `query()`: `tools: []`, `maxTurns: 1`, `model: "haiku"`, no persistence, `includePartialMessages`, custom `systemPrompt` | A custom string replaces the preset. Add a line explaining system reminders. **D** |
| AI Chat with sessions | One session per chat: store `session_id` and `resume`, or hold a streaming session open. Sidebar from `listSessions` / `getSessionMessages` / `renameSession` | Transcripts live in `~/.claude`, so keep your own chat index. A `SessionStore` can mirror them. **D** |
| AI Commands (`{selection}`, `{clipboard}`) | Substitute placeholders in akscast and send the result as the prompt. Each command can set its own append text and model | Skills or `.claude/commands` also work but add context and need setting sources. Plain templates are simpler. **I** |
| AI Extensions / Tools | In-process SDK MCP server: `tool()` + `createSdkMcpServer`, names `mcp__<server>__<tool>`, `readOnlyHint`, `permission_policy: "always_ask"` (TS), `canUseTool` for UI approval | **D** |
| MCP management | Per-session `mcpServers` (stdio/http/sse), `.mcp.json`, `mcpServerStatus()`, `reconnectMcpServer()`. Isolate with `settingSources: []` / `strictMcpConfig` | claude.ai connectors load only with a claude.ai login, unless disabled. **D** |
| Agents | `agents` option (`AgentDefinition`) plus `allowedTools: ["Agent"]` | Subagents cannot use `AskUserQuestion`. Agent teams are CLI-only. **D** |
| Output styles | `settings: { outputStyle }`, with user/project setting sources loaded | Optional. **D** |
| Hooks | `PreToolUse` for audit or deny; `PermissionRequest` for notifications | **D** |

## 5. Giving Claude akscast's capabilities

Illustrative TypeScript:

```ts
query({
  prompt,
  options: {
    cwd: sandboxDir,              // empty directory owned by akscast
    settingSources: [],           // no user/project CLAUDE.md, hooks, skills
    tools: [],                    // removes Bash, Read, Write, Edit, Agent, WebFetch
    mcpServers: { akscast },      // in-process tools only
    allowedTools: ["mcp__akscast__search_files", "mcp__akscast__clipboard_history"],
    permissionMode: "default",    // unlisted mutating tools reach canUseTool
    canUseTool: approveInUi,
  },
});
```

- `tools: []` removes all built-ins; MCP tools are unaffected. **D** [custom tools](https://code.claude.com/docs/en/agent-sdk/custom-tools.md)
- A bare deny name hides the tool; a scoped rule like `Bash(rm *)` leaves it visible. **D**
- Don't use `dontAsk` if you want an approval UI: `canUseTool` is never called in that mode. **D**
- Never use `bypassPermissions`; `allowedTools` doesn't constrain it. **D**
- Don't use `disallowedTools: ["*"]`: it would also remove your own tools. **D** (implied by the glob rules)
- Default mode auto-allows file reads inside the working directory, so keep `cwd` empty. **D**
- Sandboxed Bash isn't available on native Windows, so remove Bash rather than rely on a sandbox. **D**
- CLI equivalent: `--tools "" --strict-mcp-config --mcp-config <file> --permission-mode dontAsk`, or `--restricted`. **D**

**Routing `AI.ask` / `useAI`:**
- The `@raycast/api` shim in the sidecar sends an IPC message to an AI broker.
- The broker makes one `query()` with `tools: []`, no akscast MCP, `maxTurns: 1` and no persistence, and streams back to the worker. Extensions can't add tools to this call. **I**
- `useAI` wraps the same call; check its option surface against the shim. **I**
- Extension AI tools: a per-extension MCP server, only in a chat the user opened, with side-effecting tools set to `always_ask`. **I**
- Raycast AI options (model, creativity) haven't been mapped yet. Check them against real extensions. **I**

## 6. Safety and UX

- **Approval cards:** `canUseTool` gets the tool name and input and returns allow (optionally with `updatedInput`) or deny with a message. "Always allow" uses `suggestions` → `updatedPermissions`. **D**
- **Unattended runs:** `--permission-prompts none` denies anything that would prompt. **D**
- **Clarifying questions** arrive through `canUseTool` as `AskUserQuestion`: 1–4 questions, 2–4 options each. **D**
- **Interrupt:** `interrupt()` in streaming mode, SIGINT for the CLI. **D**
- **Cost:** `total_cost_usd` is a client-side estimate, not billing. `modelUsage` gives a per-model breakdown and `maxBudgetUsd` caps a call. On a subscription, plan limits show in `/usage`. **D** [cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking.md)
- **Streaming partial output:** `includePartialMessages`. **D**
- **Quick AI** should send on Enter only, with a concurrency cap; per-keystroke calls would exhaust subscription limits. **I**

## 7. Recommended architecture

```
Rust core (GPUI, hotkey, cloaked window)
  | IPC: requests, streamed tokens, approval cards
  v
Node sidecar (worker_threads)
  |- AI service
  |    |- query() / streaming session per chat or Quick AI call (TS Agent SDK)
  |    |- canUseTool -> approval card in GPUI -> allow / deny
  |    `- in-process SDK MCP server "akscast"  (tool handlers call Rust over IPC)
  |- @raycast/api shim: AI.ask / useAI -> AI service with tools: []
  v
claude CLI subprocess (bundled binary, one per session, stdio)
  | HTTPS
  v
api.anthropic.com  (your claude.ai login for personal use; API key if distributed)
```

**Phased plan**
0. **Spike (1–2 days).**
   - Run `query()` from a `worker_thread`.
   - Measure cold start, time to first partial token and warm-turn latency.
   - Confirm `tools: []` leaves only akscast tools.
   - Test `interrupt()` and exit behaviour on Windows.
   - Pin the SDK version.
1. **Quick AI:** one-shot, `tools: []`, `maxTurns: 1`, streamed, pre-spawned process, send on Enter, budget cap.
2. **Chat:** one session per chat, your own chat index, sidebar from `listSessions` / `getSessionMessages`, interrupt, model picker.
3. **AI Commands:** placeholder substitution, per-command append text and model, no tools.
4. **Tools:** the in-process akscast server.
   - Read-only tools are auto-allowed; mutating tools go through the approval card.
   - `PreToolUse` writes an audit log.
   - SDK tools are deferred by tool search by default, so use `alwaysLoad` on the few core tools. **D**
5. **MCP management and extensions:** a per-session `mcpServers` UI, the `AI.ask` / `useAI` broker, extension tools.
6. **Agents** (subagents), later.
7. **Distribution gate:** legal review and bring-your-own API key before sharing.

## Risks and unknowns

- **Distribution terms** (§2). This is the biggest risk.
- **Governing terms** are unclear: Commercial vs Consumer.
- **No published latency figures**; spike first.
- **Shared usage limits:** heavy Quick AI use can exhaust plan limits. **I**
- **Windows:** signal and interrupt behaviour is unverified; Bash needs Git Bash; there is no native sandbox; ARM64 Python needs a native install.
- **Spawning a child process from a `worker_thread`** is unverified. **I**
- **Prompt injection:** clipboard, files and extension output can steer tool calls. Keep tools least-privilege and put side effects behind approval. **D** [secure deployment](https://code.claude.com/docs/en/agent-sdk/secure-deployment.md)
- **Privacy:** clipboard and chat content persist in `~/.claude` transcripts. Add a delete or retention control; deletion behaviour is unverified. **I**
- **Version drift:** `--bare` is slated to become the default for `-p`. Pin the SDK and watch the changelog. **D**
- **Python sidecar pitfall:** a generator exception can stall the session silently, so use TypeScript. **D**
- **Extension API surface** (`AI.ask` options, `useAI`) isn't verified against Raycast's API. **I**

## Sources

- https://code.claude.com/docs/en/cli-reference.md
- https://code.claude.com/docs/en/headless.md
- https://code.claude.com/docs/en/authentication.md
- https://code.claude.com/docs/en/legal-and-compliance.md
- https://code.claude.com/docs/en/setup.md
- https://code.claude.com/docs/en/prompt-caching.md
- https://code.claude.com/docs/en/agent-sdk/overview.md
- https://code.claude.com/docs/en/agent-sdk/quickstart.md
- https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode.md
- https://code.claude.com/docs/en/agent-sdk/sessions.md
- https://code.claude.com/docs/en/agent-sdk/permissions.md
- https://code.claude.com/docs/en/agent-sdk/user-input.md
- https://code.claude.com/docs/en/agent-sdk/custom-tools.md
- https://code.claude.com/docs/en/agent-sdk/mcp.md
- https://code.claude.com/docs/en/agent-sdk/hosting.md
- https://code.claude.com/docs/en/agent-sdk/cost-tracking.md
- https://code.claude.com/docs/en/agent-sdk/claude-code-features.md
- https://code.claude.com/docs/en/agent-sdk/modifying-system-prompts.md
- https://code.claude.com/docs/en/agent-sdk/secure-deployment.md
- https://platform.claude.com/docs/en/build-with-claude/streaming.md
- https://www.anthropic.com/legal/aup
