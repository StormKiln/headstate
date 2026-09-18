# Headstate

Headstate is a desktop app — macOS, Windows and Linux — that shows you the
real state of the work on your machine and in your GitHub account: every
open pull request, the worktrees and branches scattered across your
checkouts, what Docker and stale build artifacts are costing you in disk,
and whether the machine itself is healthy. One window, refreshed in the
background, instead of a browser tab per repo and a terminal per question.

It started from one recurring moment: you want to ask a couple of
colleagues for reviews, and writing that Slack message by hand means
re-checking each PR's CI and merge status first. The nudge wizard turns
"which PRs need a nudge, and what's their state" into a paste-ready list in
a few clicks. The rest grew from the same principle — the answer should
already be on screen when you think to ask.

There is also an **iOS companion** that pairs with your desktop over the
local network and shows the same views on a phone. It holds no GitHub
token of its own: every question it asks is forwarded to the paired
desktop, which is the only thing that talks to GitHub.

![Headstate splash](public/splash.png)

## Status

Headstate reads from `api.github.com` and shows you what it finds. It
also **writes, but only when you ask it to** — merge, close, reopen,
draft/ready, the merge queue, auto-merge, branch deletion, and reviews
(approve, request changes, comment).

Every write is an explicit action on a pull request you are looking at.
Nothing is automated, nothing runs in the background, and every write is
logged with its repo, number, and action. Reads and writes live in
separate modules so the read path stays independently auditable.

Local actions — removing a worktree, deleting a branch, reclaiming build
output — are separated the same way, and the destructive ones confirm
first. Anything the app cannot establish is safe is refused rather than
attempted: a directory that may still be written to, a worktree with
uncommitted work, a scan that could not complete. "We could not tell" is
never reported as "nothing found".

The iOS companion holds no token and reaches GitHub only through the
paired desktop, which classifies every forwarded command as a read, a
write, or a destructive action; destructive ones require a biometric
step-up on the phone before the desktop will run them.

## Prerequisites

- macOS, Windows, or Linux.
- The [GitHub CLI](https://cli.github.com/), authenticated:

  ```
  brew install gh
  gh auth login
  ```

  Headstate reads your GitHub token from `GH_TOKEN` or `GITHUB_TOKEN` if
  either is set, and otherwise from `gh auth token`. If no token is found,
  Headstate shows you the same two commands on launch and won't proceed
  until they work — there is no separate login flow inside the app itself.

#### Token scopes

Headstate needs three scopes:

| Scope | What stops working without it |
| --- | --- |
| `repo` | Everything. Pull requests are not readable at all. |
| `read:org` | **PR Stats only.** The sidebar lists no organizations, so an org or a team cannot be selected — which looks like having no organizations rather than like a missing permission. |
| `gist` | Nothing in Headstate. It is in `gh auth login`'s own minimum set, so a `gh`-authenticated token has it regardless. |

`gh auth login` requests all three: `repo`, `read:org` and `gist` are its
stated minimum (`gh auth login --help`, verified on gh 2.100.0), so if you
authenticated that way there is nothing to do.

The gap is a **hand-made token**. A classic personal access token or a CI
token in `GH_TOKEN` / `GITHUB_TOKEN` carries only the scopes it was created
with, and `read:org` is easy to leave off — it is not needed for anything
except PR Stats. Check what a token actually has:

```
gh api -i user 2>/dev/null | grep -i '^x-oauth-scopes:'
```

and if `read:org` is missing from a `gh`-managed token, add it in place:

```
gh auth refresh -s read:org
```

Fine-grained personal access tokens report no scopes at all through that
header; they carry permissions instead, and the one to grant is
**Organization permissions → Members: read**.

### Building from source on Linux

Releases ship a `.deb` and an `.AppImage`, so building from source is only
necessary to develop against the app or to run an unreleased commit. The
steps below were verified on Ubuntu 26.04; package names differ on other
distributions, but the four things you need are the same.

Install `gh` from the [GitHub CLI site](https://cli.github.com/), which
carries current apt, dnf, and Homebrew instructions. The version in Ubuntu's
own archive lags well behind and may sit behind an ESM subscription, so
prefer GitHub's apt repository over `apt install gh`.

Tauri links against GTK and WebKit at build time. The runtime libraries are
usually installed already; the matching `-dev` packages that provide the
headers and `.pc` files are usually not:

```
sudo apt install -y \
  build-essential pkg-config libssl-dev \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev \
  libxdo-dev libayatana-appindicator3-dev librsvg2-dev
```

Rust, stable channel, which is what CI pins:

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
```

Node 22 or newer, plus Yarn 4 from Corepack:

```
corepack enable
yarn install --immutable
```

Use `corepack enable` rather than `npm i -g yarn`. The global npm package
installs Yarn 1.22, which refuses outright to run a project whose
`package.json` pins `"packageManager": "yarn@4.18.0+sha224.…"`. The
`+sha224.` suffix is the hash of the Yarn bundle itself: corepack
verifies what it downloads against it and refuses a mismatch, so the
package manager is pinned to content, not just to a version number.
`scripts/check-supply-chain-pins.py` keeps it that way, along with the
other places CI fetches from outside the repository.

Confirm the backend compiles before launching anything:

```
cd src-tauri && cargo check
```

Then `make dev` from the repository root.

CI runs the Rust and frontend suites plus Clippy on `ubuntu-latest` as well
as macOS, so a Linux-only build or link failure is caught before it reaches
you. The release workflow builds the `.deb` and `.AppImage` there too.

## Install

Download the latest `.dmg` or `.app.tar.gz` from the
[releases page](https://github.com/pktstorm/headstate/releases) and drag
`Headstate.app` into `/Applications`.

macOS builds are signed with a Developer ID certificate and notarized by
Apple, so they open normally — no quarantine workaround, no "unidentified
developer" dialog. Releases are universal binaries: one download works on
both Apple Silicon and Intel.

Signing landed in v5.3.0. An earlier release needs the quarantine flag
cleared once before it will launch:

```
xattr -dr com.apple.quarantine /Applications/Headstate.app
```

### If Headstate says it cannot find `gh`

A GUI application does not inherit your shell's `PATH` — on a clean Mac it
gets `/usr/bin:/bin:/usr/sbin:/sbin`, which excludes Homebrew. Headstate
therefore also looks in `/opt/homebrew/bin`, `/usr/local/bin`, and
`/opt/local/bin`, which covers Homebrew on Apple Silicon and Intel plus
MacPorts.

If `gh` lives somewhere else, point Headstate at it directly:

```
launchctl setenv HEADSTATE_GH /full/path/to/gh
```

then relaunch the app. `which gh` in your terminal prints the path to use.

## What it shows

**Pull request list.** Every open PR you authored, across every repo you
have access to, in one list — the chrome mirrors GitHub's own
`<owner>/<repo>/pulls` view: filter by label (include *and* exclude —
GitHub's own UI only lets you include), review state, drafts, and sort
order.

**Notifications.** When a pull request you opened newly breaks — CI turns
red, or a merge conflict appears — Headstate posts a desktop notification.
Only transitions fire, so an already-broken PR is not reported again every
minute, and the first poll after launch is always silent. Recoveries,
approvals, and new reviews deliberately do not notify: an interruption
should mean something needs your hands, and the tray badge already carries
the rest passively.

**Branch pairs.** Each row shows what merges into what
(`ci_fix_2 → main`). A target that is not the default branch is tinted:
that pull request is stacked on another one and cannot merge until its
base does, which nothing else in the row would tell you.

**Unresolved conversations.** A row shows how many review conversations
are still open on the current code, so a pull request waiting on replies
is visible rather than merely stalled. Resolved and outdated threads are
excluded. It reports the count rather than claiming the PR is blocked:
whether a repository requires resolution before merging is only readable
with admin access on that repository.

**Priorities strip.** Pinned above the list: PRs blocked on *you* and
nobody else — real merge conflicts or failing CI — so the thing you need to
fix first doesn't get lost in a longer list. Quiet when nothing is blocked.

**PR Stats.** The first entry in the view menu, answering what the open-PR
list cannot: how much is actually getting done, whether that is improving,
and — for a team or org lead — how the team is doing.

The sidebar is a GitHub hierarchy rather than a list of local checkouts,
because this page is about GitHub activity and a local clone is neither
necessary nor sufficient for it:

```
Organizations
  <org>
    Repos     -> All repos, or one line per repository
    Members   -> one line per member, scoping the page to that person
Personal
  All repos, or one line per repository
```

Every scope offers two views. **Mine** is your own figures. **Others** is
the same measures for everyone else in scope, plus leaderboards — top
authors, top reviewers, top by code volume.

Nothing queries until you ask. Each scope sits behind an explicit load,
which is not merely a performance nicety: at organisation scale a
render-on-navigate would issue tens of sequential requests per sidebar
click.

Available on the iOS companion as well as the desktop.

- **Four headline figures** — merged and opened this week, merged this
  month, and median cycle time — each with its change against the previous
  period. Every window is stated in words, because the comparison
  deliberately excludes the current (incomplete) day: counting a partial
  day against complete ones drags every number down.
- **Activity chart** — opened against merged per day over 7, 14, or 30
  days. The two series are overlaid rather than stacked; they measure
  overlapping populations, so a stacked total would be meaningless. The gap
  between them reads as the backlog growing or draining.
- **Insight cards** — cycle time (median and p90), total lines changed, and
  median PR size, computed over a sample of your most recently merged PRs
  and labelled with that sample size.
- **Merged by repository** — where the work actually landed, with each row
  a way back into that repo's filtered list.

The view fetches only while it is open. At the 30-day range it costs eight
GitHub rate-limit points out of 5,000 per hour: six day-bucket chunks, plus
the period comparisons, plus the merged-PR sample. The daily series uses aliased queries rather
than paginating merged PRs: an aliased search costs a single point no
matter how many aliases it carries, so a month of history is cheaper than
one page of paginated results.

The series is fetched in chunks of five days, concurrently. GitHub returns 502 Bad Gateway
on a query that takes too long to evaluate, and search aliases are evaluated
serially — so alias count drives elapsed time rather than being a limit of its
own. Measured: every 502 landed at around eleven seconds regardless of shape,
while 80 count-only aliases answered in under that and 48 node-heavy ones did
not. Requests stay well under the deadline rather than retrying into it.

**Worktrees.** Every git worktree across your checkout directories, with a
safety verdict per row: merged, unmerged, uncommitted work, never pushed,
or locked by an agent. The verdict is the point — "safe to remove" is only
offered when it is actually safe, and uncommitted work outranks merge
status, so a dirty worktree on a merged branch is never presented as
disposable. Rows you can act on carry Remove; ones you cannot say why.
Worktrees on unmerged branches can be handed to Claude Code for an
assessment of whether the work is still wanted.

**Branches.** Local branches and whether each is deletable, with the reason
in words rather than a boolean — merged upstream, unmerged, or checked out
somewhere. Deletions are batched and confirmed, because no reflog undoes a
remote branch deletion.

**Repositories.** Browse the files of any repository in your scanned
folders, the way GitHub's code view does: a directory listing, click to
descend, click a file to read it. The listing comes from the index
rather than from the filesystem, so what you see is the repository and
not its build output -- in Headstate's own checkout that is the
difference between a few hundred entries and several hundred thousand.
Files are read up to a fixed limit and the truncation is stated rather
than silent; binaries are named rather than rendered; and symbolic links
are shown but not followed. Available on the phone companion too, where
the files are otherwise unreachable.

**Docker.** What Docker is costing you in disk: images, containers,
volumes and build cache, with the dangling and reclaimable portions
separated from what is actually in use. Build images accumulate silently
and this is where that shows up.

**Artifacts.** Stale build output — `target/`, `node_modules/`, virtualenvs
and caches — found beside your checkouts rather than inside your worktrees,
which is where it actually lives. On the machine this was built for, 108 GB
of Rust build output sat next to main checkouts and 0.28 GB inside
worktrees, so removing every worktree would not have touched 99.7% of it.
Nothing is deleted without a confirmation, and a directory that may still
be written to is refused rather than removed.

**Package updates.** Outdated dependencies per repository, across npm,
Cargo, pip and friends, so "is anything behind" is one glance rather than a
command per repo.

**CLAUDE.md.** The `CLAUDE.md` files across your repositories with
estimated token counts, so instruction files that have quietly grown past
useful are visible. The counts are estimates and every label says so.

**Claude Code.** Your Claude Code sessions, and the ones worth bringing
back. Sessions are read from the transcripts Claude Code already writes, so
the view opens with real history rather than waiting for something to
happen. Each row carries a three-state liveness — running, not running, or
could not tell — and the resumable ones offer the exact `claude --resume`
command, prefixed with a `cd` to the directory the session ran in, because
`--resume` adopts the directory it is invoked from. An overview page counts
what is resurrectable and what is archived because its directory is gone.

**This is off by default.** Turn it on in Settings, under Claude
Integrations. It stays off until asked for because the optional session hook
appends to `~/.claude/settings.json`, and a feature that edits another
tool's configuration should be requested rather than assumed. Without the
hook the view still lists every session from the transcripts; the hook adds
the process id, which is what lets Headstate tell a session that is running
from one that was killed.

**System health.** CPU, memory, disk, network, GPU and battery for the
machine itself, with drill-down pages that answer the "why" a summary can
only raise — a panel says memory is at 88%, the Memory page says which
processes. Conditions worth investigating appear at the top: a process
holding a moderate amount of CPU for long enough to be suspicious, or a
machine that has been oversubscribed for a while. The thresholds favour
duration over level, because a compiler is hot and brief while an abandoned
loop is neither.

**Filters and repo sidebar.** A sidebar of repos with open PR counts, plus a
filter bar for labels, review state, and drafts.

**Nudge wizard.** A three-step flow — pick repos, pick which PRs qualify
(ready for review only, green CI only, needs-attention only, stale only),
then pick a text format — that produces a paste-ready block and copies it
to your clipboard. Nothing here calls GitHub; it only reads PRs already in
memory and formats them as text.

## Nudge output formats

The wizard composes plain text for pasting into Slack, a PR description, or
anywhere else. Every example below uses the `octocat` GitHub demo org.

**Flat markdown** (default, under the auto-group threshold):

```
- [octocat/hello-world#42] Add retry to the fetch client — https://github.com/octocat/hello-world/pull/42
- [octocat/spoon-knife#7] Bump the parser dependency — https://github.com/octocat/spoon-knife/pull/7
```

**Grouped by repo** (auto-enables at 3+ distinct repos, or toggle it
yourself), with status annotations on:

```
**octocat/hello-world**
- [#42] Add retry to the fetch client (green, approved) — https://github.com/octocat/hello-world/pull/42
- [#43] Fix flaky timezone test (CI failing) — https://github.com/octocat/hello-world/pull/43

**octocat/spoon-knife**
- [#7] Bump the parser dependency (CI running) — https://github.com/octocat/spoon-knife/pull/7
```

**Slack format**, grouped and annotated: Slack renders mrkdwn, not
markdown — a `[text](url)` link shows up as literal text there, and bold is
`*single asterisks*`, not `**double**`. That's why the Slack toggle exists;
it isn't cosmetic.

```
*octocat/hello-world*
- <https://github.com/octocat/hello-world/pull/42|#42> Add retry to the fetch client (green, approved)
- <https://github.com/octocat/hello-world/pull/43|#43> Fix flaky timezone test (CI failing)

*octocat/spoon-knife*
- <https://github.com/octocat/spoon-knife/pull/7|#7> Bump the parser dependency (CI running)
```

Status annotations, when enabled, are one of: `(CI failing)`,
`(needs rebase)`, `(draft)`, `(green, approved)`, `(green, awaiting review)`,
or `(CI running)`.

## Known limitations

- **Windows and Linux builds are unsigned.** The macOS build is signed and
  notarized. On Windows, SmartScreen will warn on first run — choose "More
  info" then "Run anyway". Linux ships a `.deb` and an `.AppImage`; mark the
  AppImage executable with `chmod +x` before running it.
- **A `.deb` install cannot self-update.** Tauri publishes no updater
  signature for a `.deb`, so it is never advertised in the update manifest
  and stays on the version you installed. The `.AppImage` is the
  auto-updating Linux artifact; re-download the `.deb` per release.
- **x86_64 only on Windows and Linux.** No arm64 builds are produced for
  either, so there is nothing for the updater to offer an arm64 install.
  macOS ships a universal binary.
- **Merged history is a sample.** The insight cards and the repository
  breakdown are computed over your 100 most recently merged pull requests,
  not your whole history. The figures are labelled with that sample size;
  the daily chart and the headline counts are exact.

## Development

Headstate is a [Tauri 2](https://v2.tauri.app/) app: a Rust backend
(`src-tauri/`, using the `octocrab` crate for the GitHub client and SQLite
for the local snapshot cache) and a React 19 + TypeScript + Tailwind 4 +
shadcn/ui frontend (`src/`), wired together with
[TanStack Query](https://tanstack.com/query) and Zustand.

Install dependencies with `yarn install --immutable`, then use the
Makefile for everything else:

```
make doctor       # check this checkout is ready before anything else
make dev          # yarn tauri dev — run the app locally, live reload
make build         # yarn tauri build — produce a runnable .app / .dmg
make test          # both suites below
make test-rust     # cargo test (src-tauri)
make test-ui       # yarn vitest run
make lint          # both linters below
make lint-rust     # cargo fmt --check && cargo clippy -D warnings
make lint-ui       # yarn tsc -b --force && yarn eslint . && yarn knip
make fmt           # cargo fmt
make icons         # regenerate app + tray icons from the master PNG
```

**Do not run `cargo build --release` directly and expect a runnable app.**
See [CONTRIBUTING.md](CONTRIBUTING.md) for why — it's a real trap, not a
theoretical one.

CI (`.github/workflows/ci.yml`) runs the privacy guard, Rust formatting and
Clippy, TypeScript typechecking, ESLint, Knip, the full Rust test suite
(plus a 10x repeat to catch races), the frontend test suite, an app-bundle
build, and a supply-chain check (`cargo-deny` + `yarn npm audit`) on every
push and pull request. All of it must be green before merge.

## License

Apache-2.0. See [LICENSE](LICENSE).
