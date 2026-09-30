# GitLab slice 6: source selector and combined queues

The source control persists `github`, `gitlab`, or `both` in the shared frontend's
local preferences. The desktop also persists the choice in SQLite. On restart,
GitLab-only mode pauses the GitHub queue poll before its first request. GitLab.com
authored and `reviews_for_me` queues start when GitLab is selected; the current
frontend runs bounded refreshes on activation and every 60 seconds while open.
The paired phone uses the same frontend and the desktop's source-scoped read
commands and event stream. Its own source choice does not change the desktop's
poll preference.

GitHub-only mode retains the existing list, filters, actions, and detail route.
GitLab and Both use a combined queue ordered by creation time. Provider, host,
and full project path label every row. Repository filters use the provider and
host as well as project path; selecting a row uses the four-part PR identity.
GitLab rows and detail summaries are read-only pending the detail and action
slices. GitLab list CI and review states remain visibly unmeasured, not green or
zero. A source failure keeps the other source's rows and the failed source's
saved rows, with separate error, partial coverage, and stale markers.

The enabled GitLab host is currently GitLab.com. Self-managed host fetching is
not configured by the queue adapter. GitLab stats remain separate and are
supplied by slice 8's `ProviderStatsPage`; integration replaces the temporary
message in `App.tsx` with `<ProviderStatsPage selection={selection} />`.

This slice registers `get_source_snapshot`, `get_source_poll_status`, and
`refresh_source` on desktop and paired-phone surfaces. `set_source_selection`
is desktop-local because a phone's view choice must not change the desktop's
polling preference.

Verification at this checkpoint: prior source-cache tests passed 3/3; current
`cargo check --lib --no-default-features`, `cargo fmt --check`, and
`git diff --check` passed. A new snapshot wire test was added but its test
binary could not be linked after the machine reached its disk limit. TypeScript
transpilation parsed nine changed files with no syntax diagnostics; a partial
typecheck found no remaining diagnostics in changed files after local cached
React/Zustand types were supplied. Full Yarn and mobile gates remain for
integration, since Yarn/Corepack is unavailable offline in this worktree.
