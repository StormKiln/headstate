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

## The merge queue

Enabled 2026-09-16, on ruleset `21146057` alongside the four existing rules.
It is a RULESET setting, not a file, so nothing in the tree shows it -- this
section is the record.

```
merge_method                      SQUASH   (matches pull_request's allowed_merge_methods)
grouping_strategy                 ALLGREEN
max_entries_to_build / to_merge   5
min_entries_to_merge              1
min_entries_to_merge_wait_minutes 0
check_response_timeout_minutes    30
```

**Why these numbers.** `min_entries_to_merge: 1` with a zero wait, because with
a single active author a batch of one is the normal case and waiting for a
batch to fill would make every merge slower rather than faster. `5` matches the
six-PR pile-up #764 measured. `30` is double the 13-minute worst case recorded
there: a timeout shorter than a real run turns slow-but-passing CI into a queue
ejection, which reads as flakiness.

**`ci.yml` was already ready.** It triggers on `merge_group` and exempts
merge-group runs from `cancel-in-progress`, both with comments explaining why.
Neither needed changing -- but if either is ever removed, queued pull requests
hang until they time out and get ejected.

**The app has an Enqueue action that now works.** `github/mutate.rs` reads
`mergeQueueEntry` and treats a null entry as a failure, with the message "the
base branch may not use a merge queue". That sentence was unconditionally true
until now.

**`strict_required_status_checks_policy` stays true.** The two are
complementary rather than redundant: `strict` guarantees a pull request was
tested against `main` as it WAS, and the queue guarantees it was tested against
`main` as it WILL BE, alongside whatever merges with it. Two green pull
requests that have never been compiled together is the failure neither a rebase
nor a clean git merge can catch.
