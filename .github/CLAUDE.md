# .github

CI and release workflows. See the root `CLAUDE.md` for rules that apply
everywhere, and the `release` skill for cutting a release.

## The release gate sees every check-run attempt

`release.yml`'s gate sets `wait-for-duplicates: true`, so it waits on **all**
check-run attempts on the tagged commit, including superseded ones.

**One failed attempt burns that commit for releases permanently.** Re-running
adds a passing attempt; it never removes the failed one.

The trap: the default check-runs API returns only the latest attempt per job, so
a burned commit can look entirely green.

```bash
gh api "repos/OWNER/REPO/commits/SHA/check-runs?per_page=100&filter=all" \
  -q '[.check_runs[]|.conclusion]|group_by(.)|map({(.[0]//"null"):length})|add'
```

This burned v5.20.0. `f497c75` still shows `success / failure / success`.

## A tag push triggers a second CI run

On a commit already verified on `main` — so every release has two independent
chances to flake. Check `filter=all` again after tagging.

## `allowed-conclusions: success`

`skipped` is **not** allowed. That cost a release at v5.4.0, which is why
`ignore-checks` lists the jobs that legitimately do not run.

## Versions come from the tag

`release.yml` stamps from `GITHUB_REF_NAME`. The in-repo `0.1.0` is correct and
needs no bump commit.

## The lint job runs `make lint`

Not `yarn lint`. That difference — knip and clippy — is the most common reason a
green local check is followed by a red CI run.
