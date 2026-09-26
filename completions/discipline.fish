# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_discipline_global_optspecs
    string join \n h/help V/version
end

function __fish_discipline_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_discipline_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_discipline_using_subcommand
    set -l cmd (__fish_discipline_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c discipline -n "__fish_discipline_needs_command" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_needs_command" -s V -l version -d 'Print version'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "check" -d 'Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "diff" -d 'Shorthand for checking uncommitted or working tree changes against HEAD'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "baseline" -d 'Record or manage grandfathered finding baselines'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "init" -d 'Write a minimal discipline.toml: gates run at their built-in defaults; commented examples show what to change'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "gates" -d 'List every gate: id, suite, availability, and effective state'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "schema" -d 'Print the JSON Schema for discipline.toml'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "self-test" -d 'Run the embedded negative / positive controls against this binary'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "completions" -d 'Generate shell completion script to stdout (bash, zsh, fish, powershell, elvish)'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "docs" -d 'Generate or check reference docs and schemas against sources of truth'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "install-hooks" -d 'Install pre-commit hook in the local git repository'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "hook" -d 'Run the gates inside a coding agent\'s edit loop (Claude Code, Codex, Cursor, Aider, Copilot CLI, agy, Qwen Code, OpenCode)'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "explain" -d 'Explain a gate: what it checks, its state here, and the directive that lifts a finding'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "replay" -d 'Replay the last N merged changes through a configuration: what it would have blocked'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "mcp" -d 'Serve the gates to an MCP client over stdio (read-only tools: check_diff, list_gates, explain_finding)'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "bench" -d 'Benchmark tooling for the bench-regression gate'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "doctor" -d 'Check that the repository and its platform enforce discipline: workflows, CODEOWNERS, branch protection. Exit 0 = healthy, 1 = a failing check, 2 = could not check'
complete -c discipline -n "__fish_discipline_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -s c -l config -d 'Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l config-override -d 'Inline TOML merged over the file (tables merge, lists append, scalars replace)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l enable -d 'Gate ids to force on (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l disable -d 'Gate ids to force off (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -s s -l suite -d 'Which check suite to run' -r -f -a "all\t''
agent-guard\t''
hygiene\t''
integrity\t''
quality\t''
verification\t''
bench\t''"
complete -c discipline -n "__fish_discipline_using_subcommand check" -s b -l base -d 'Base branch or commit to measure the change against (auto-detected in CI if omitted)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l commit -d 'Specific commit to inspect, against its parent <sha>~1; it must be the commit checked out (exit 2 otherwise)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l commit-range -d 'Commit range to inspect (<before>..<after> or <before>...<after>); <after>, when given, must be the commit checked out (exit 2 otherwise)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l pr-body-file -l commit-msg-file -d 'File holding the PR body or commit message (override directives, hygiene scanning). Falls back to the PR_BODY environment variable' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l pr-title -d 'PR title for PR-level hygiene checks (e.g. issue-link). Falls back to the PR_TITLE environment variable' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l policy-from -d 'Which side\'s discipline.toml judges the change. `base` reads it from the base ref, so a policy edit takes effect once merged; `config-integrity` still reports it' -r -f -a "head\t'The configuration in the working tree (the change\'s own copy)'
base\t'The configuration on the base ref'"
complete -c discipline -n "__fish_discipline_using_subcommand check" -l actor -d 'Actor executing the check (for actor-aware override authorization). Falls back to DISCIPLINE_ACTOR, GITHUB_ACTOR, GITEA_ACTOR, FORGEJO_ACTOR, GITLAB_USER_LOGIN' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l directive-sources -d 'Comma-separated list of allowed directive sources (pr-body, commits, merged-pr-body)' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -s f -l format -d 'Output format' -r -f -a "terminal\t''
github-summary\t''
json\t''
junit\t''
sarif\t''
gitlab\t''
agent-prompt\t''"
complete -c discipline -n "__fish_discipline_using_subcommand check" -l json-out -d 'Also write the JSON report to this path, whatever --format is' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -s o -l output-file -d 'Write the formatted report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l report-gitlab -d 'Write GitLab Code Quality JSON report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l report-junit -d 'Write JUnit XML report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l report-sarif -d 'Write SARIF report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l bench-provenance -d 'Expected host or runner provenance tag for benchmark artifacts' -r
complete -c discipline -n "__fish_discipline_using_subcommand check" -l bench-base-file -d 'In-job base benchmark result file for bench-regression dual-mode' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l bench-head-file -d 'In-job head benchmark result file for bench-regression dual-mode' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l baseline-file -d 'Path to grandfathering baseline file (defaults to discipline-baseline.toml if present)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand check" -l staged -d 'Inspect the index against HEAD instead (pre-commit hook mode)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l fail-on-warnings -d 'Treat warnings as failures'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l fail-on-overrides -d 'Treat applied overrides as failures (requires human sign-off)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l advisory -d 'Advisory mode: run all checks and emit reports, but exit code 0 even if violations occur'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l comment -d 'Post the report as one pull-request comment, edited on every run (needs a token that can write comments; off by default)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l trust-workspace -d 'Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -s q -l quiet -d 'Suppress output on success (only print output when violations are found)'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l allow-cross-host-bench -d 'Allow benchmark comparison across mismatched host/runner provenance tags'
complete -c discipline -n "__fish_discipline_using_subcommand check" -l no-baseline -d 'Ignore grandfathering baseline even if present'
complete -c discipline -n "__fish_discipline_using_subcommand check" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s c -l config -d 'Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l config-override -d 'Inline TOML merged over the file (tables merge, lists append, scalars replace)' -r
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l enable -d 'Gate ids to force on (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l disable -d 'Gate ids to force off (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s s -l suite -d 'Which check suite to run' -r -f -a "all\t''
agent-guard\t''
hygiene\t''
integrity\t''
quality\t''
verification\t''
bench\t''"
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s b -l base -d 'Base branch or commit ref to compare against (defaults to HEAD for uncommitted changes)' -r
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s f -l format -d 'Output format' -r -f -a "terminal\t''
github-summary\t''
json\t''
junit\t''
sarif\t''
gitlab\t''
agent-prompt\t''"
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l json-out -d 'Also write the JSON report to this path, whatever --format is' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s o -l output-file -d 'Write the formatted report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l report-gitlab -d 'Write GitLab Code Quality JSON report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l report-junit -d 'Write JUnit XML report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l report-sarif -d 'Write SARIF report to this path' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l baseline-file -d 'Path to grandfathering baseline file (defaults to discipline-baseline.toml if present)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l no-baseline -d 'Ignore grandfathering baseline even if present'
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l trust-workspace -d 'Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1)'
complete -c discipline -n "__fish_discipline_using_subcommand diff" -l advisory -d 'Advisory mode: run checks and emit reports, but exit 0 even if violations are found'
complete -c discipline -n "__fish_discipline_using_subcommand diff" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -s c -l config -d 'Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l config-override -d 'Inline TOML merged over the file (tables merge, lists append, scalars replace)' -r
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l enable -d 'Gate ids to force on (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l disable -d 'Gate ids to force off (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l baseline-file -d 'Path to grandfathering baseline file (defaults to discipline-baseline.toml)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -s b -l base -d 'Base branch or commit ref to compare against' -r
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -s s -l suite -d 'Specific suite to run: all, agent-guard, hygiene, integrity ...' -r -f -a "all\t''
agent-guard\t''
hygiene\t''
integrity\t''
quality\t''
verification\t''
bench\t''"
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l write -d 'Record current findings to the baseline file'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l migrate -d 'Rewrite a fingerprint-version-1 baseline to version 2: every entry a current finding still matches is kept under its finding code, and stale entries are dropped. Commit the result in a change of its own, which `config-integrity` accepts without a directive'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l whole-tree -d 'Record every pre-existing finding in the tree, not just the diff. Use when adopting discipline on an existing repository; conflicts with --base'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l all-severities -d 'Also record warnings and notes. By default only findings that would block under the current configuration are recorded: `error`, plus `warning` under --fail-on-warnings'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l fail-on-warnings -d 'Treat warnings as blocking when choosing what to record (same switch as `check --fail-on-warnings`)'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -l trust-workspace -d 'Trust the workspace and disable libgit2 repository owner validation'
complete -c discipline -n "__fish_discipline_using_subcommand baseline" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand init" -s n -l name -d 'Name of the project (defaults to current directory name)' -r
complete -c discipline -n "__fish_discipline_using_subcommand init" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand gates" -s c -l config -d 'Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand gates" -l config-override -d 'Inline TOML merged over the file (tables merge, lists append, scalars replace)' -r
complete -c discipline -n "__fish_discipline_using_subcommand gates" -l enable -d 'Gate ids to force on (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand gates" -l disable -d 'Gate ids to force off (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand gates" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand schema" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand self-test" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand completions" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand docs" -l check -d 'Check that reference documentation is up to date with sources'
complete -c discipline -n "__fish_discipline_using_subcommand docs" -l write -d 'Regenerate and write reference documentation across the repository'
complete -c discipline -n "__fish_discipline_using_subcommand docs" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand install-hooks" -s f -l force -d 'Overwrite existing pre-commit hook if present'
complete -c discipline -n "__fish_discipline_using_subcommand install-hooks" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and not __fish_seen_subcommand_from run install help" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and not __fish_seen_subcommand_from run install help" -f -a "run" -d 'Check the change so far and answer in the agent\'s hook contract (reads the hook payload on stdin)'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and not __fish_seen_subcommand_from run install help" -f -a "install" -d 'Write the agent\'s hook configuration at the repository root (or, with --user, the user-level one); an existing file is never rewritten'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and not __fish_seen_subcommand_from run install help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from run" -l agent -d 'The agent whose hook contract to answer in' -r -f -a "claude-code\t'Claude Code (`.claude/settings.json`, PostToolUse + Stop)'
codex\t'OpenAI Codex CLI (`.codex/hooks.json`, PostToolUse + Stop)'
cursor\t'Cursor (`.cursor/hooks.json`, stop)'
aider\t'Aider (`.aider.conf.yml`, lint-cmd)'
copilot\t'GitHub Copilot CLI (`.github/hooks/discipline.json`, postToolUse + agentStop)'
agy\t'Antigravity CLI (`.agents/hooks.json`, Stop)'
qwen\t'Qwen Code (`.qwen/settings.json`, PostToolUse + Stop)'
opencode\t'OpenCode (`.opencode/plugins/discipline.js`, a plugin after edit tools)'"
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from run" -s b -l base -d 'Base to measure the change against (default: the merge base with origin\'s default branch, else main / master)' -r
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from run" -l if-configured -d 'Pass silently unless the working directory is in a git repository with a discipline.toml at its root (for a user-level hook, which runs in every folder)'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from run" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from install" -l agent -d 'The agent to configure' -r -f -a "claude-code\t'Claude Code (`.claude/settings.json`, PostToolUse + Stop)'
codex\t'OpenAI Codex CLI (`.codex/hooks.json`, PostToolUse + Stop)'
cursor\t'Cursor (`.cursor/hooks.json`, stop)'
aider\t'Aider (`.aider.conf.yml`, lint-cmd)'
copilot\t'GitHub Copilot CLI (`.github/hooks/discipline.json`, postToolUse + agentStop)'
agy\t'Antigravity CLI (`.agents/hooks.json`, Stop)'
qwen\t'Qwen Code (`.qwen/settings.json`, PostToolUse + Stop)'
opencode\t'OpenCode (`.opencode/plugins/discipline.js`, a plugin after edit tools)'"
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from install" -l user -d 'Write the user-level hook instead (copilot: hooks/discipline.json in the Copilot home directory, .copilot in your home or COPILOT_HOME), which runs in every folder but checks only repositories with a discipline.toml'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from install" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "run" -d 'Check the change so far and answer in the agent\'s hook contract (reads the hook payload on stdin)'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "install" -d 'Write the agent\'s hook configuration at the repository root (or, with --user, the user-level one); an existing file is never rewritten'
complete -c discipline -n "__fish_discipline_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand explain" -s c -l config -d 'Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand explain" -l config-override -d 'Inline TOML merged over the file (tables merge, lists append, scalars replace)' -r
complete -c discipline -n "__fish_discipline_using_subcommand explain" -l enable -d 'Gate ids to force on (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand explain" -l disable -d 'Gate ids to force off (comma separated)' -r
complete -c discipline -n "__fish_discipline_using_subcommand explain" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand replay" -l last -d 'Number of first-parent commits (merged changes) to replay, newest first' -r
complete -c discipline -n "__fish_discipline_using_subcommand replay" -l ref -d 'Branch whose history is replayed (default: origin\'s default branch, else main / master)' -r
complete -c discipline -n "__fish_discipline_using_subcommand replay" -s c -l config -d 'Configuration under test (default: discipline.toml in the working tree)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand replay" -l json -d 'Print the summary as JSON'
complete -c discipline -n "__fish_discipline_using_subcommand replay" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand mcp" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and not __fish_seen_subcommand_from derive help" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and not __fish_seen_subcommand_from derive help" -f -a "derive" -d 'Derive paired-ratio noise floors and baseline ratios from repeated same-commit runs'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and not __fish_seen_subcommand_from derive help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from derive" -l baseline -d 'Merge the derived platform entry into this ratio baseline file (created if absent)' -r -F
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from derive" -l ceiling-pct -d 'Derived cell floors above this percentage are reported but not gated' -r
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from derive" -l allow-mixed-commits -d 'Accept runs of different commits (recorded in the baseline); only when the differences cannot move a number'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from derive" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from help" -f -a "derive" -d 'Derive paired-ratio noise floors and baseline ratios from repeated same-commit runs'
complete -c discipline -n "__fish_discipline_using_subcommand bench; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -l branch -d 'Branch whose protection is checked (default: the repository\'s default branch)' -r
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -l repo -d 'Repository path on the forge, e.g. OWNER/NAME (default: from the CI environment or the `origin` remote; set DISCIPLINE_FORGE for a self-hosted forge)' -r
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -s f -l format -d 'Output format' -r -f -a "text\t''
json\t''"
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -l local-only -d 'Check only local files; skip the platform API'
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -l strict -d 'Treat warnings as failures'
complete -c discipline -n "__fish_discipline_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "check" -d 'Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "diff" -d 'Shorthand for checking uncommitted or working tree changes against HEAD'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "baseline" -d 'Record or manage grandfathered finding baselines'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "init" -d 'Write a minimal discipline.toml: gates run at their built-in defaults; commented examples show what to change'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "gates" -d 'List every gate: id, suite, availability, and effective state'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "schema" -d 'Print the JSON Schema for discipline.toml'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "self-test" -d 'Run the embedded negative / positive controls against this binary'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "completions" -d 'Generate shell completion script to stdout (bash, zsh, fish, powershell, elvish)'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "docs" -d 'Generate or check reference docs and schemas against sources of truth'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "install-hooks" -d 'Install pre-commit hook in the local git repository'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "hook" -d 'Run the gates inside a coding agent\'s edit loop (Claude Code, Codex, Cursor, Aider, Copilot CLI, agy, Qwen Code, OpenCode)'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "explain" -d 'Explain a gate: what it checks, its state here, and the directive that lifts a finding'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "replay" -d 'Replay the last N merged changes through a configuration: what it would have blocked'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "mcp" -d 'Serve the gates to an MCP client over stdio (read-only tools: check_diff, list_gates, explain_finding)'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "bench" -d 'Benchmark tooling for the bench-regression gate'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "doctor" -d 'Check that the repository and its platform enforce discipline: workflows, CODEOWNERS, branch protection. Exit 0 = healthy, 1 = a failing check, 2 = could not check'
complete -c discipline -n "__fish_discipline_using_subcommand help; and not __fish_seen_subcommand_from check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c discipline -n "__fish_discipline_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "run" -d 'Check the change so far and answer in the agent\'s hook contract (reads the hook payload on stdin)'
complete -c discipline -n "__fish_discipline_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "install" -d 'Write the agent\'s hook configuration at the repository root (or, with --user, the user-level one); an existing file is never rewritten'
complete -c discipline -n "__fish_discipline_using_subcommand help; and __fish_seen_subcommand_from bench" -f -a "derive" -d 'Derive paired-ratio noise floors and baseline ratios from repeated same-commit runs'
