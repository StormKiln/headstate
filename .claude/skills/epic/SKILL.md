---
name: epic
description: Use when filing a Headstate epic with linked sub-issues, or when planning multi-PR work - covers the sub-issue API shape and the integrity checks that two real failures earned
---

# Epic

## Linking is keyed on the internal id, not the number

This is the step people get wrong:

```bash
ID=$(gh api repos/OWNER/REPO/issues/<n> -q .id)      # internal id, NOT <n>
gh api -X POST repos/OWNER/REPO/issues/<epic>/sub_issues -F sub_issue_id="$ID"
```

Verify, and use the same query to report progress honestly:

```bash
gh api graphql -f query='{repository(owner:"OWNER",name:"REPO"){
  issue(number:<epic>){ subIssues(first:50){ totalCount nodes{ number state title } } } } }' \
 -q '.data.repository.issue.subIssues | "total=\(.totalCount) closed=\([.nodes[]|select(.state=="CLOSED")]|length)"'
```

Use real sub-issues, not a markdown task list — a checklist produces no
`subIssues` count, so progress cannot be verified.

## Verify body-to-title after filing

**Four sub-issue bodies once shipped as verbatim copies of other issues**
(#1017, #1018, #1020, #1022 — confirmed by md5 against #1013/#1014/#1016).
Several agents filed concurrently and nothing checked the bodies.

The sub-issue list renders **titles only**, so the contamination was invisible
from the epic and survived until someone opened an issue.

After filing, re-read each body and confirm it matches its own title. When
issues were filed concurrently, diff bodies against each other:

```bash
for n in <numbers>; do printf "%s %s\n" "$n" "$(gh issue view $n --json body -q .body | md5)"; done | sort -k2
```

Identical hashes mean duplicated bodies.

## Name the seam between sub-issues

**A component once shipped that nothing rendered.** One PR built
`AllRepositoriesTable`, another built its host; both were green, both correct
alone, and nothing mounted it. Green CI on every PR does not add up to a
working feature when the integration point is what is missing.

When one sub-issue builds a component and another builds its host, say so in
**both** bodies, and require a call-site check before either closes:

```bash
grep -rn 'ComponentName' src --include='*.tsx' | grep -v 'ComponentName.tsx\|\.test\.'
```

Zero hits outside its own file and test means nothing renders it.

## Write bodies worth reading

An issue body should carry the **verified mechanism** — file, line, and the
measurement — not a restatement of the symptom. Include:

- what was observed, in the reporter's words where possible
- the mechanism, traced in the code, with file:line
- what is explicitly **out of scope**, and why
- a test that would have caught it, and why existing tests did not

An issue that says only what the user saw makes the next agent redo the
diagnosis. Several issues in this repo were resolved quickly because the body
already named the line.

## Keep the epic updated as work lands

Comment on the epic when a PR merges: which commit, which sub-issues closed,
and anything learned that changes how the remaining work should be done. The
epic is the durable record; the PR descriptions scatter.
