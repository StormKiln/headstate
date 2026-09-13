# Contributing to Headstate

Thanks for taking a look. Headstate is a small, opinionated macOS app —
this doc covers the one rule you can't infer from the code, plus the
day-to-day workflow.

## The privacy rule (read this first)

**Headstate is a public repository.** Fixtures, screenshots, and
documentation must use synthetic data — `octocat/hello-world`,
`octocat/spoon-knife`, and similar, drawn from GitHub's own demo org. Never
commit a real repository name, PR title, branch name, or URL from a private
employer or private codebase. Once something is pushed to a public repo's
history, it's effectively permanent.

CI enforces this on every push and PR via `scripts/check-privacy.sh`, which
scans for `github.com/<owner>/<repo>` URLs, `git@github.com:<owner>/<repo>`
SSH remotes, `<owner>/<repo>#<number>` issue/PR shorthand, GitHub
Enterprise Server URLs, `ssh://git@<host>/...git` clone URLs to any host,
employer email addresses, Slack/Atlassian workspace URLs, and
`PREFIX-NNNN`-shaped internal ticket IDs.

This script is an **allow-list**, deliberately. A deny-list would have to
spell out the very names it exists to keep out — putting them in the repo
in plain text and defeating itself. An allow-list also catches owners
nobody thought to enumerate ahead of time. If you legitimately need to
reference a new public repository owner (a real upstream dependency, for
example), add it to the `ALLOWED` list at the top of the script rather than
working around the check.

Run the guard locally before you push:

```
./scripts/check-privacy.sh
```

It should print `privacy check: clean`. If it doesn't, fix the reference
(swap in synthetic data) rather than adding the real owner to the
allow-list unless it's genuinely a public, legitimate dependency.

The guard also refuses to run (exit 2) if you have **untracked files** in
the working tree. `git grep`, which the guard uses, can only see tracked
and staged content — an untracked file is invisible to it, so a "clean"
result while one exists would be false. Run `git add` (or `git add -N` to
stage without changing content) on any new files and re-run the guard.

## Workflow

Install dependencies once with `yarn install --immutable`, then use the
Makefile targets for everything:

```
make dev      # run the app locally with live reload
make test     # cargo test + yarn vitest run
make lint     # cargo fmt --check, clippy, tsc, eslint, knip
make fmt      # cargo fmt (writes, doesn't just check)
```

Run `make lint` and `make test` before opening a PR. Every CI check must be
green before merge — there's no fast-tracking a red check.

These targets are not the same thing as CI, and it's worth knowing where
they stop. `make lint` and `make test` cover the checks that answer in
seconds; CI additionally runs:

| What CI adds | Run it locally with |
| --- | --- |
| Race check — the Rust suite **three times** at `--test-threads=8` | `make test-race` |
| `cargo deny check` for `src-tauri` (supply chain) | `make deny` |
| `cargo check` for the Intel target the release also builds | `make check-intel` |
| `yarn npm audit` | *(no target — needs the npm advisories service)* |
| A full `tauri build`, and the Windows/Linux `platform` jobs | *(CI only)* |

The race check is the one to reach for when a test passes locally and fails
in CI: **one green run does not prove a race is absent**, which is exactly
why CI repeats the suite. The other two are cheap insurance before a
release, since an arch-gated link failure or a new advisory would otherwise
first appear at tag time.

Two guards that used to be CI-only now run in `make lint` (via
`lint-deps`): `scripts/check-privacy.sh` and
`scripts/check-workflow-shells.py`. The privacy one matters most, because
it scans **commit messages** as well as files — catching it locally means
an edit, and catching it in CI means an amend or an interactive rebase
after the content has already reached a public remote.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, etc.) — look at
`git log` for the pattern the project already uses.

## The `cargo build --release` trap

If you work in `src-tauri/`, do not run `cargo build --release` (or plain
`cargo build`) directly and expect it to produce a runnable app.

`cargo build` only compiles the Rust binary. It has no idea the project
also has a frontend, so it skips Tauri's `beforeBuildCommand` (`yarn
build`), which is what actually compiles the React app into `dist/` and
embeds it in the bundle. The result: a binary that's noticeably smaller
than a real build (no embedded JS/CSS/assets), and if you launch it, the
window opens and renders a blank **white screen** — before any JavaScript
ever loads, because the frontend was never built or embedded in the first
place. There's no error message; it just never shows anything.

This burned a real debugging session, which is why it's called out here
instead of left to be rediscovered. Always build through Tauri:

```
make build            # or: yarn tauri build
```

`cargo test` and `cargo clippy` are fine to run directly from `src-tauri/`
— it's specifically *building the app bundle* that needs to go through
`tauri build`, since only that path runs `beforeBuildCommand`.

## Two lints that keep catching people

**`items_after_test_module`.** Appending a new function or constant to the
end of a Rust file puts it *after* `#[cfg(test)] mod tests`, which clippy
rejects. It compiles and the tests pass, so it only fails at `make lint` —
and it has caught this project three times. Insert new items **before** the
test module, not at the end of the file.

**Your local Rust may be older than CI's.** `clippy::unnecessary_sort_by`
fired on CI's 1.98 while passing locally on 1.93.1. A clean local `make
lint` is necessary but not sufficient; if CI disagrees, check
`rustc --version` before assuming a flake.

## When `main` goes red

A red `main` is a stop. It blocks everyone, and it blocks tagging — so the
first question is not "whose change was it" but "is anyone about to
release".

**Reproduce before you re-run.** A re-run is evidence only when the failure
does *not* reproduce. If it reproduces locally, it is a bug and the re-run
is how it gets shipped anyway. Two examples from one day:

- `a_seeded_budget_can_actually_refuse` reproduced at **4 failures in 6
  local runs** (#868). It had also *passed* on one `main` commit, so a
  green re-run was always available and would have proved nothing.
- `App.test.tsx:246` did **not** reproduce in 5 local runs (#898) — so the
  re-run was justified, and the work was to find out *why* CI differed. It
  races a lazy import that resolves in single-digit milliseconds locally.

The distinction is the whole rule: re-running a reproducible failure hides
it; re-running an unreproducible one is a legitimate step, and the next
step is to explain the difference.

**Capture the log before it disappears.** A superseding push cancels the
run, and a cancelled run's logs can become unreadable. If you re-run first,
you may destroy the only evidence.

**A flake that blocks a tag gets an issue, not a shrug.** Four were fixed
in one cycle, and two of them had doc comments *naming the hazard they then
committed* (#861, #868). `invariants.rs` now catches that shape
mechanically. Fix the assertion so it tests the property rather than a
proxy — do not add a retry, and do not raise a timeout. A retried race
check is a race check that no longer detects races.

**Revert or fix forward?** Fix forward if the cause is understood and the
fix is one cycle away. Otherwise revert — every minute red is a minute
nobody else can merge. Branch protection requires branches be current with
`main`, so a red `main` also means every open PR needs another cycle once
it clears.

**A merged release epic is not a shipped release.** The tag is the only
source of truth (see below). v5.12.0's work was merged and never tagged,
and the next person to pull "5.12" got the old build with every fixed bug
apparently still broken. Whoever merges the last PR owns cutting the tag,
or says out loud that they are not.

## Cutting a release

Releases are driven entirely by tags. There is nothing to click and no
version to bump by hand:

```
git tag v0.2.0
git push origin v0.2.0
```

That fires `.github/workflows/release.yml`, which:

1. **Stamps the version from the tag** into `package.json`,
   `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`. The tag is the
   single source of truth — those three files stay at whatever they say in
   `main` and are only rewritten inside the CI job, never committed. Without
   this, tagging `v0.2.0` would ship `Headstate_0.1.0_universal.dmg` and an
   About box reading `0.1.0`.
2. Builds a **universal** binary (Apple Silicon + Intel), so one download
   runs on both.
3. Signs and notarizes it **if** the Apple secrets are present (see below).
   They are not today, so this step is skipped.
4. Creates the GitHub Release with the `.dmg` and a `.app.tar.gz`, and
   generates release notes from the commits since the last tag.

The tag must be `vMAJOR.MINOR.PATCH`. Anything else (`v1.2`, `latest`,
`vfoo`) fails the job early with a clear message rather than publishing a
mislabelled build.

**The release notes adapt to the signing state on their own.** While
releases are unsigned, every release gets the `xattr -dr
com.apple.quarantine` instruction prepended automatically. Once the signing
secrets exist, that text is replaced with a note that the build is signed
and notarized — no edit to the workflow, and no stale instruction left
behind for users to follow unnecessarily.

To undo a bad tag before anyone downloads it, delete it locally and
remotely (`git tag -d v0.2.0 && git push origin :v0.2.0`) and delete the
GitHub Release. Re-tagging the same version works, but only if the release
and tag are both gone first.

## Code signing

macOS releases are **signed and notarized** as of v5.3.0. The steps live
in `.github/workflows/release.yml`, guarded on the secrets below, so a
fork without them still builds unsigned rather than failing. These are
the secrets the signing path reads (Settings → Secrets and variables →
Actions):

| Secret | What it is |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` export of a **Developer ID Application** certificate (not Apple Distribution, not Apple Development — those don't work for distribution outside the App Store) |
| `APPLE_CERTIFICATE_PASSWORD` | The password the `.p12` was exported with |
| `KEYCHAIN_PASSWORD` | Any password; used only for the throwaway keychain CI creates to hold the imported cert |
| `APPLE_ID` | The Apple ID email used for notarization |
| `APPLE_PASSWORD` | An [app-specific password](https://support.apple.com/en-ca/HT204397) for that Apple ID — not the account password |
| `APPLE_TEAM_ID` | The Apple Developer Team ID (found on the [membership page](https://developer.apple.com/account/#/membership)) |

`APPLE_SIGNING_IDENTITY` is deliberately not listed as a secret to set —
Tauri infers it from the imported `APPLE_CERTIFICATE` at build time, so
there's nothing to hardcode ahead of having a real certificate.

Verify a release actually got signed rather than trusting a green build —
the workflow takes the unsigned path silently when a secret is missing:

```
spctl -a -vvv -t install /Volumes/Headstate/Headstate.app
```

`accepted` with `source=Notarized Developer ID` is the answer you want.
`xcrun stapler validate` on the same path confirms the ticket is stapled,
which is what lets the app launch on a machine with no network.

Notarization adds real time to the macOS job: it waits on Apple's
service, and the first submission from a new signing identity took about
an hour where the build alone takes twelve minutes. Later ones are
usually far quicker.

Headstate started strictly read-only. Since the write path landed it can
also act on a pull request — merge, close, mark as draft, enqueue,
approve, request changes, comment, resolve or reply to a review thread,
re-run checks, update the branch, toggle auto-merge, and open a
package-update PR — but **only when you click that action in the app**.
Nothing runs on a timer, nothing is batched, and each write goes through
one module (`src-tauri/src/github/mutate.rs`) so the full list is in one
place. If you're proposing a change that would have Headstate write to
GitHub on its own initiative, expect that to be a bigger conversation
about scope, not a quick PR.
