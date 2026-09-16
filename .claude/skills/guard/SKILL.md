---
name: guard
description: Use when adding or changing a source-scanning invariant in src-tauri/src/invariants.rs - sabotage-proving in both directions, and the three ways this repo has already got one wrong
---

# Guard

`invariants.rs` holds source-scanning guards that catch classes of defect
ordinary tests miss. They are also the easiest thing to get subtly wrong,
because **a guard that passes for the wrong reason is worse than no guard**: it
reports safety it is not checking.

## Sabotage-prove it, or do not ship it

Break the production code, watch the **specific** guard fail, restore, re-run.
A guard never observed failing has not been tested.

Report what you broke and what you saw. "Added a guard" is not a result.

## A sabotage that does not fail is information

While building #1044, the first attempt to break `board.complete = false` broke
nothing — absent aliases already cleared the flag by another route, so the
assertion was passing for the wrong reason. The fix was a test isolating the
one case where the rule is load-bearing.

If your sabotage does not fail, you have learned the assertion is not carrying
its weight. Isolate the case where it is, or delete it.

## Prove the negative too

A guard with false positives gets weakened or deleted by the next person under
time pressure. After #1048's guard was rescoped in #1050, it had to be shown
**not** to flag the three safe sync mocks in `fetch.rs`.

Both directions, every time: it fails on the real defect, and it stays silent
on the safe shape.

## Three ways this repo has already got it wrong

**Matching a comment instead of code.** #874's own sabotage proof found a guard
matching its pattern inside a doc comment. This codebase documents rules
directly above the code they govern, so the pattern appears in prose far more
often than in a call. Strip comment lines before matching — and skip
`invariants.rs` **by path**, never by comment-matching, since the rule's own
prose contains the pattern.

**Scanning one file.** #875's guard used `include_str!("client.rs")`. #1044 then
added the identical hazard to `board.rs`, which it could not see — and the
resulting flake burned the v5.20.0 tag. Walk the tree with `rust_files(&root)`
so a new file is covered without anyone remembering to add it.

**Scoping too coarsely.** #1048's replacement asked whether a *file's* test
module contained `#[tokio::test]`. Adding one async test to `fetch.rs` made
three genuinely safe sync mocks look like offenders. Scope to the smallest unit
that owns the hazard — the test function, not the file.

## Practical notes

- **Normalise `\r\n` before any `\n`-anchored pattern.** A CRLF checkout makes a
  `\n` split match nothing, so the guard scans an empty string and passes
  silently, on Windows only. Observed here, not feared.
- **Reuse `test_fns`**, which exposes `where_`, `name`, `doc`, `body` and
  `is_async`, rather than re-parsing.
- **The failure message must name the file and the offending line.** The guards
  that get fixed quickly are the ones that say exactly what to change.
- **Prefer a guard to prose.** If a rule can be checked mechanically, it belongs
  here rather than in a CLAUDE.md, which rots while the guard stays true.
