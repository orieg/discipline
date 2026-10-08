// Written by `discipline hook install --agent opencode`.
// discipline-hook-file: mode=observe sha256=abffb77e5610677b6c4457a1984b56c688f4c4a36b2032dc21d3e0cca097a505
// When a session is created, takes this worktree's lease for it. Before an edit tool, refuses an edit outside this session's worktree (the tool call
// is sent on stdin; a refusal throws, and the model reads the reason). After it, runs
// the discipline check and, when it fails, appends the report to the tool's output so
// the model reads it and repairs the change.
// Observe mode: no hook below refuses a tool call or throws, whatever the command does (a non-zero exit, a command that is not found, any other error).
import { $ } from "bun"

const EDIT_TOOLS = ["edit", "write", "apply_patch"]

export const Discipline = async ({ $, directory }) => ({
  // A new session takes this worktree's lease; it never blocks the session.
  event: async ({ event }) => {
    try {
      if (event.type !== "session.created") return
      const start = new Response(JSON.stringify({ input: { sessionID: event.properties?.sessionID }, cwd: event.properties?.info?.directory ?? directory }))
      await $`discipline hook run --agent opencode --event session-start < ${start}`.nothrow().quiet()
    } catch {}
  },
  "tool.execute.before": async (input, output) => {
    try {
      if (!EDIT_TOOLS.includes(input.tool) && input.tool !== "bash") return
      const call = new Response(JSON.stringify({ input, output, cwd: directory }))
      await $`discipline hook run --agent opencode --event pre-tool --observe < ${call}`.cwd(directory).nothrow().quiet()
    } catch {}
  },
  "tool.execute.after": async (input, output) => {
    try {
      if (!EDIT_TOOLS.includes(input.tool)) return
      const r = await $`discipline hook run --agent opencode --observe`.cwd(directory).nothrow().quiet()
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
            await $`discipline hook run --agent opencode --event session-start < ${start}`.nothrow().quiet()
          } catch {}
        }
      })().catch(() => {})
    }

    tool?.hook?.("execute.before", async (call) => {
      try {
        const toolName = call.tool ?? call.input?.tool
        if (!EDIT_TOOLS.includes(toolName) && toolName !== "bash") return
        const payload = new Response(JSON.stringify({
          input: { tool: toolName, sessionID: call.sessionID },
          output: { args: call.input ?? {} },
          cwd: directory
        }))
        await $`discipline hook run --agent opencode --event pre-tool --observe < ${payload}`.cwd(directory).nothrow().quiet()
      } catch {}
    })

    tool?.hook?.("execute.after", async (call) => {
      try {
        const toolName = call.tool ?? call.input?.tool
        if (!EDIT_TOOLS.includes(toolName)) return
        const r = await $`discipline hook run --agent opencode --observe`.cwd(directory).nothrow().quiet()
        if (r.exitCode !== 0) {
          if (call.result) {
            call.result.output = (call.result.output ?? "") + "\n\n" + r.stdout.toString() + r.stderr.toString()
          }
        }
      } catch {}
    })
  }
}
