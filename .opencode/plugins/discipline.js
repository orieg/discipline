// Written by `discipline hook install --agent opencode`.
// discipline-hook-file: mode=observe sha256=0ab3f4de7c96c13dbc33c92d1b7167c70916890896dac0a71b1c18908293325a
// When a session is created, takes this worktree's lease for it. Before an edit tool, refuses an edit outside this session's worktree (the tool call
// is sent on stdin; a refusal throws, and the model reads the reason). After it, runs
// the discipline check and, when it fails, appends the report to the tool's output so
// the model reads it and repairs the change.
// Observe mode: no hook below refuses a tool call or throws, whatever the command does (a non-zero exit, a command that is not found, any other error).
import { $ } from "bun"

const EDIT_TOOLS = ["edit", "write", "apply_patch"]
// OpenCode 2.x names its shell tool `shell`; 1.x named it `bash`.
const SHELL_TOOLS = ["bash", "shell"]
const HOOK_TIMEOUT_MS = 30_000
const CHECK_TIMEOUT_MS = 120_000

const withTimeout = (p, ms) => {
  let timer
  const limit = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`discipline hook timed out after ${ms / 1000}s`)), ms)
    timer?.unref?.()
  })
  return Promise.race([Promise.resolve(p), limit]).finally(() => clearTimeout(timer))
}

export const Discipline = async ({ $, directory }) => ({
  // A new session takes this worktree's lease; it never blocks the session.
  event: async ({ event }) => {
    try {
      if (event.type !== "session.created") return
      const start = new Response(JSON.stringify({ input: { sessionID: event.properties?.sessionID }, cwd: event.properties?.info?.directory ?? directory }))
      await withTimeout($`discipline hook run --agent opencode --event session-start < ${start}`.nothrow().quiet(), HOOK_TIMEOUT_MS)
    } catch {}
  },
  "tool.execute.before": async (input, output) => {
    try {
      if (!EDIT_TOOLS.includes(input.tool) && input.tool !== "bash") return
      const call = new Response(JSON.stringify({ input, output, cwd: directory }))
      await withTimeout($`discipline hook run --agent opencode --event pre-tool --observe < ${call}`.cwd(directory).nothrow().quiet(), HOOK_TIMEOUT_MS)
    } catch {}
  },
  "tool.execute.after": async (input, output) => {
    try {
      if (!EDIT_TOOLS.includes(input.tool)) return
      const r = await withTimeout($`discipline hook run --agent opencode --observe`.cwd(directory).nothrow().quiet(), CHECK_TIMEOUT_MS)
      if (r.exitCode !== 0) {
        output.output += "\n\n" + r.stdout.toString() + r.stderr.toString()
      }
    } catch {}
  },
})

export default {
  id: "discipline",
  setup({ tool, event, location }) {
    const directory = location?.directory ?? process.cwd()

    // OpenCode 2.x registers no event callback: `event.subscribe()` returns the events
    // as an async iterable. A session new to this plugin takes the lease on the first of
    // `session.created` (not delivered to a plugin loaded after the session was made,
    // as under `opencode run --standalone`) and `session.execution.started`.
    const events = event?.subscribe?.()
    if (typeof events?.[Symbol.asyncIterator] === "function") {
      const leased = new Set()
      ;(async () => {
        for await (const e of events) {
          try {
            if (e?.type !== "session.created" && e?.type !== "session.execution.started") continue
            const sessionID = e.data?.sessionID
            if (typeof sessionID !== "string" || leased.has(sessionID)) continue
            leased.add(sessionID)
            const start = new Response(JSON.stringify({
              input: { sessionID },
              cwd: e.location?.directory ?? directory
            }))
            await withTimeout($`discipline hook run --agent opencode --event session-start < ${start}`.nothrow().quiet(), HOOK_TIMEOUT_MS)
          } catch {}
        }
      })().catch(() => {})
    }

    tool?.hook?.("execute.before", async (call) => {
      try {
        const toolName = call.tool ?? call.input?.tool
        if (!EDIT_TOOLS.includes(toolName) && !SHELL_TOOLS.includes(toolName)) return
        const payload = new Response(JSON.stringify({
          input: { tool: toolName, sessionID: call.sessionID },
          output: { args: call.input ?? {} },
          cwd: directory
        }))
        await withTimeout($`discipline hook run --agent opencode --event pre-tool --observe < ${payload}`.cwd(directory).nothrow().quiet(), HOOK_TIMEOUT_MS)
      } catch {}
    })

    tool?.hook?.("execute.after", async (call) => {
      try {
        const toolName = call.tool ?? call.input?.tool
        if (!EDIT_TOOLS.includes(toolName)) return
        const none = new Response("")
        const r = await withTimeout($`discipline hook run --agent opencode --observe < ${none}`.cwd(directory).nothrow().quiet(), CHECK_TIMEOUT_MS)
        if (r.exitCode !== 0 && call.result) {
          const report = "\n\n" + r.stdout.toString() + r.stderr.toString()
          if (Array.isArray(call.result.content)) {
            const part = call.result.content.find((c) => c?.type === "text" && typeof c.text === "string")
            if (part) part.text += report
            else call.result.content.push({ type: "text", text: report.trimStart() })
          } else if (typeof call.result.output === "string") {
            call.result.output += report
          }
        }
      } catch {}
    })
  }
}
