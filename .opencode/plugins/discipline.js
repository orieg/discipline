// Written by `discipline hook install --agent opencode`.
// When a session is created, takes this worktree's lease for it. Before an edit tool, refuses an edit outside this session's worktree (the tool call
// is sent on stdin; a refusal throws, and the model reads the reason). After it, runs
// the discipline check and, when it fails, appends the report to the tool's output so
// the model reads it and repairs the change.
const EDIT_TOOLS = ["edit", "write", "apply_patch"]

export const Discipline = async ({ $, directory }) => ({
  // A new session takes this worktree's lease; it never blocks the session.
  event: async ({ event }) => {
    if (event.type !== "session.created") return
    const start = new Response(JSON.stringify({ input: { sessionID: event.properties?.sessionID }, cwd: event.properties?.info?.directory ?? directory }))
    await $`discipline hook run --agent opencode --event session-start < ${start}`.nothrow().quiet()
  },
  "tool.execute.before": async (input, output) => {
    if (!EDIT_TOOLS.includes(input.tool) && input.tool !== "bash") return
    const call = new Response(JSON.stringify({ input, output, cwd: directory }))
    const r = await $`discipline hook run --agent opencode --event pre-tool --observe < ${call}`.cwd(directory).nothrow().quiet()
    if (r.exitCode !== 0) {
      throw new Error(r.stdout.toString() + r.stderr.toString())
    }
  },
  "tool.execute.after": async (input, output) => {
    if (!EDIT_TOOLS.includes(input.tool)) return
    const r = await $`discipline hook run --agent opencode --observe`.cwd(directory).nothrow().quiet()
    if (r.exitCode !== 0) {
      output.output += "\n\n" + r.stdout.toString() + r.stderr.toString()
    }
  },
})
