# CI throughput implementation plan

Goal: reduce macOS queue pressure without reducing required validation.
Approved spec: user's seven-item CI velocity plan in this conversation.

1. Split static lint (Linux), native lint (macOS), and strict required lint aggregate. Move frontend/contrast and existing multi-target dependency audit to Linux. Add Node/Rust setup selectors with unchanged defaults. Keep native crash evidence on macOS and all race, platform and mobile checks.
2. Add default-branch-only maintenance workflow and tested Python client: cancel only CI merge-group runs whose exact queue ref has disappeared or moved; recheck before cancellation; never cancel PR/main/tag/release or fail open on API errors. Emit per-job queue/setup/execution timing summaries without altering CI results.
3. Add workflow contract regression tests and invoke them in lint. Run existing workflow/supply-chain/mobile guard suites, actionlint, frontend, Rust and full make lint. Review before push; open PR and monitor CI without adding concurrent merge groups.
4. Queue settings (concurrency1, timeout120) and actual dequeue require API capabilities unavailable in current MCP. Browser permission pending. Do not claim configured until verified.

Rulings: Use existing ~/code/ghstat branch, no new worktree. No platform-specific test moved out of macOS. No reduction in repeated race tests. Preserve existing nine required check names. Keep cache writes restricted to main.
