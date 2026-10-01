# Synthetic persisted task records

`task-results.json` and `task-stop.json` are selected generic records from a synthetic Claude Code 2.1.284 session prepared for release 8.2. They preserve actual persisted `toolUseResult` casing and shapes; no private corpus or CLI initialization records are included.

TaskOutput input tests use the published official Agent SDK 0.2.80 `sdk-tools.d.ts` contract (`task_id: string`, `block: boolean`, `timeout: number`). That tool was unavailable in the installed CLI, so these are contract tests, not a claim of observed TaskOutput execution. TaskStop's optional deprecated `shell_id` alias is declared in SDK 0.2.80 and 0.3.286.

Sources: https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.2.80.tgz and https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.286.tgz.
