# scripts

Check scripts invoked by `make lint` and `make lint-mobile`, so they run in CI.

See the root `CLAUDE.md` for rules that apply everywhere.

## Every check script has tests beside it

`check-foo.py` has `check-foo.test.py`. Add or update the tests with the script
— these scripts gate CI, and a check that is silently wrong is worse than no
check, for the same reason a guard that passes for the wrong reason is (see the
`guard` skill).

## `check-cache-budget.py` reads a shared, draining resource

It measures the Actions cache against its budget, and that figure **changes
between runs without anyone touching the repo**. Consequences:

- A failure is not a permanent property of a branch. Re-read before concluding.
- Never tell someone "that one always fails" — that instruction was given twice
  in one cycle and was wrong both times; the check was passing.

## Scripts that encode a rule

Several of these exist because a rule was broken once: `check-mobile-build-mark`
(a shipped build number must not be lagged), `check-supply-chain-pins`,
`check-tauri-versions`, `check-symlinks`, `check-required-contexts`. When adding
one, say in the docstring **what went wrong that made it necessary** — that is
what makes it maintainable rather than mysterious.
