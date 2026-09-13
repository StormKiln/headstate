#!/usr/bin/env python3
"""The Actions cache must stay inside a budget somebody chose.

#901: the repository sat at 9.92GB of a 10GB Actions cache quota and was
evicting mid-day, so run 34713149169 reported

    ##[warning]Cache not found for keys: v0-rust-platform-Windows_NT-...

for a key that had reported `full match: true` twenty-five minutes
earlier. That turned a 562s job into 1716s. Over quota, GitHub evicts
least-recently-used, so a measured cache HIT followed by a miss on the
same key is expected behaviour rather than a flake -- and CI timings stop
being reproducible, which quietly invalidates any speed work measured
against a warm cache.

The cause was NOT what #901 guessed. It hypothesised two live generations
per job from recent dependency churn. The duplicates split by REF:

    refs/heads/main       5.29GB   6 entries
    refs/pull/895/merge   4.63GB   6 entries

One open pull request held 46% of the quota in entries only it could ever
read, because GitHub scopes cache WRITES to the branch that made them.
`save-if` in `.github/actions/setup/action.yml` fixes that; this script is
the other half of #901's ask -- "decide a budget per job class and assert
it, rather than discovering the ceiling through a 3x-slower run".

---- Why a budget guard and not a prune workflow ----

A scheduled REST prune was the other candidate. It is real new surface --
a workflow, a token, a deletion loop that can delete the wrong thing --
and the leak it would paper over has a one-line cause that is now fixed.
Measuring and asserting is the honest first step; if this guard starts
firing on steady state alone, THAT is the evidence that automation is
warranted, and it will arrive with numbers attached.

---- Why this one may skip, when the others may not ----

Unlike every other guard in `lint-deps`, this one needs the network and a
token, so it follows `check-mobile-build-mark.py` exactly: locally it
reports what it found and skips when it cannot look; CI passes --require,
where a token exists and an unreachable API is worth seeing.

That is NOT the cry-wolf shape `supply-chain`'s yarn audit refuses. The
distinction is the same one `check-required-contexts.py` draws: the
ruleset's contents are a CONTRACT that belongs in the tree, so checking
them over the network would be trading a real guard for a flaky one. Cache
SIZE is a live measurement with no committed equivalent -- there is
nothing to read but the API -- so the only choices are to ask it or not to
ask at all.

---- AND WHY IT IS NOT A REQUIRED CHECK (yet) ----

This guard is in `make lint-deps` and deliberately NOT in `ci.yml`'s
`lint` job, which is where its three siblings live. The reason is the
state of the cache on the day it was written: 9.92GB, over the 8.5GB
budget, and this guard correctly fails on it.

4.63GB of that is one open pull request's entries, which GitHub reclaims
when the PR merges or after seven idle days. So a CI gate added today
would fail every pull request on a condition NO pull request author could
fix -- which is the same unmergeable shape #887 exists to prevent, bought
with a guard meant to prevent slowness. A cache measured on a shared,
draining resource is not a property of the branch under test.

It earns a place in `lint` once steady state is measured under budget on
`main` with the `save-if` fix in place. Until then it is the thing you run
when CI timings look unreproducible, and it answers in one API call.
"""

import argparse
import json
import os
import re
import subprocess
import sys

GIB = 1024**3

# GitHub's per-repository Actions cache quota. Not ours to choose; the
# budget below has to live inside it.
QUOTA_GIB = 10.0

# The ceiling for EVERYTHING, chosen with headroom under the quota rather
# than pressed against it. At 7.37GB measured steady state this leaves
# ~1.1GB, which is roughly one spare copy of the largest entry -- enough
# to absorb a single overlapping generation during a dependency bump
# without evicting, which is the event that caused #901's incident.
TOTAL_BUDGET_GIB = 8.5

# Per job class, keyed by the rust-cache key with its trailing lockfile
# hash stripped. Measured on 2026-09-12 and rounded UP to the next 0.1GB,
# so these are ceilings rather than observations.
#
# The per-class ceilings exist because the TOTAL is not enough on its own:
# #901's incident was one job class (`platform-Windows`) with two live
# generations while the total was still under quota. A total-only budget
# would have called that run healthy.
#
# A class appearing here TWICE in one measurement is therefore a failure
# even when the total is fine: it means two generations are live, which is
# the state that evicts.
CLASS_BUDGET_GIB = {
    "v0-rust-platform-Linux-x64": 1.7,
    "v0-rust-mobile-android-Linux-x64": 1.6,
    "v0-rust-platform-Windows_NT-x64": 1.6,
    "v0-rust-build-Darwin-arm64": 0.9,
    "v0-rust-test-rust-Darwin-arm64": 0.7,
    "v0-rust-mobile-ios-Darwin-arm64": 0.6,
    "v0-rust-lint-Darwin-arm64": 0.5,
    "v0-rust-supply-chain-Darwin-arm64": 0.2,
    "v0-rust-test-frontend-Darwin-arm64": 0.2,
}

# rust-cache keys are `<prefix>-<job>-<platform>-<envhash>-<lockhash>`.
# Both trailing hashes are stripped to get the job class: the env hash
# moves with the Rust version, which is not a budget change.
KEY_SHAPE = re.compile(r"^(v0-rust-.+?)-[0-9a-f]{8}-[0-9a-f]{8}$")


def job_class(key: str) -> str | None:
    """The budgeted class of a cache key, or None if it is not rust-cache.

    Returning None rather than guessing matters: a yarn or pip cache added
    later counts against the QUOTA but is not a Rust job class, and
    treating it as an unbudgeted one would fail this guard for a reason
    that has nothing to do with it.
    """
    m = KEY_SHAPE.match(key)
    return m.group(1) if m else None


# The ref whose caches are the repository's steady state. Everything else
# is a branch's own copy, which `save-if` stops creating and which GitHub
# reclaims on merge or after seven idle days.
BASE_REF = "refs/heads/main"


def _is_base(entry: dict) -> bool:
    ref = entry.get("ref", "")
    return ref == BASE_REF or ref.startswith("refs/tags/")


def verdict(entries: list[dict]) -> list[str]:
    """BUDGET findings for the measured cache entries. Empty means healthy.

    Judges the STEADY STATE -- `main` and tags -- because that is the part
    this repository controls and the part `save-if` now guarantees is the
    only part. Entries on other refs are reported separately by
    `leftovers()`: they are a draining resource nobody can fix from a
    branch, so failing on them would be the cry-wolf shape this project
    refuses elsewhere.

    Pure on purpose -- it takes the parsed API rows and makes every
    decision, so the self-test can drive it without a network or a token.
    """
    problems: list[str] = []

    # THE FLOOR, first and before any arithmetic. No entries is not a tidy
    # cache; it is a guard that failed to look, and a 0.00GB "under
    # budget" would be the most reassuring possible way to report that
    # (#853). Every sibling guard asserts a floor for the same reason.
    #
    # Asserted on the RAW list, before the base-ref filter: "the API
    # returned nothing" and "main happens to hold nothing right now" are
    # different facts, and only the first is a broken measurement.
    if not entries:
        problems.append(
            "no cache entries were measured at all. That is not an empty cache, "
            "it is a measurement that did not happen -- a budget check with "
            "nothing in it passes trivially and tells you nothing."
        )
        return problems

    base = [e for e in entries if _is_base(e)]

    total = sum(e["size_in_bytes"] for e in base) / GIB
    if total > TOTAL_BUDGET_GIB:
        problems.append(
            f"`{BASE_REF}` holds {total:.2f}GB, over the {TOTAL_BUDGET_GIB}GB budget "
            f"(GitHub's quota is {QUOTA_GIB}GB for the whole repository, and over it "
            f"entries are evicted least-recently-used -- which is how a 562s job "
            f"became 1716s in #901)"
        )

    # Group by class so both "one entry grew" and "two generations are
    # live" are visible, since those need different fixes.
    by_class: dict[str, list[dict]] = {}
    for e in base:
        cls = job_class(e["key"])
        if cls is not None:
            by_class.setdefault(cls, []).append(e)

    for cls, rows in sorted(by_class.items()):
        if cls not in CLASS_BUDGET_GIB:
            size = sum(r["size_in_bytes"] for r in rows) / GIB
            problems.append(
                f"`{cls}` has no budget ({size:.2f}GB measured). A new Rust job is "
                f"how the ceiling gets exceeded without anyone deciding to -- add "
                f"it to CLASS_BUDGET_GIB with a measured figure, and check the "
                f"total still fits"
            )
            continue

        budget = CLASS_BUDGET_GIB[cls]
        if len(rows) > 1:
            size = sum(r["size_in_bytes"] for r in rows) / GIB
            problems.append(
                f"`{cls}` has {len(rows)} live generations on `{BASE_REF}` totalling "
                f"{size:.2f}GB. Two generations of one class on the base ref is the "
                f"state that evicts (#901), and it means a dependency bump left the "
                f"old generation resident -- the budget must fit 2x or the old one "
                f"must go"
            )
        for r in rows:
            size = r["size_in_bytes"] / GIB
            if size > budget:
                problems.append(
                    f"`{cls}` is {size:.2f}GB, over its {budget}GB ceiling "
                    f"({r['key']}). Either what it caches grew, or the ceiling was "
                    f"set too tight -- decide which, and move the number on purpose"
                )

    return problems


def leftovers(entries: list[dict]) -> dict[str, float]:
    """{ref: GB} for caches held by refs other than the base, largest first.

    Reported, never failed on. Since `save-if` these should only be
    entries predating it or written by a tag/merge-group run, and GitHub
    reclaims them on merge or after seven idle days -- so a branch cannot
    fix them and must not be blocked by them. They are shown because they
    DO count against the 10GB quota, which is what makes them worth
    seeing when a timing looks wrong (#901).
    """
    held: dict[str, float] = {}
    for e in entries:
        if _is_base(e):
            continue
        held[e.get("ref", "?")] = held.get(e.get("ref", "?"), 0.0) + e["size_in_bytes"] / GIB
    return dict(sorted(held.items(), key=lambda kv: -kv[1]))


def measure() -> list[dict] | str:
    """The live cache entries, or a string saying why we could not look."""
    repo = os.environ.get("GITHUB_REPOSITORY", "pktstorm/headstate")
    try:
        out = subprocess.run(
            ["gh", "api", "--paginate", f"repos/{repo}/actions/caches?per_page=100"],
            capture_output=True,
            text=True,
            timeout=60,
        )
    except FileNotFoundError:
        return "the `gh` CLI is not installed"
    except subprocess.TimeoutExpired:
        return "the GitHub API did not answer within 60s"
    if out.returncode != 0:
        first = (out.stderr or "").strip().splitlines()
        return f"the GitHub API call failed: {first[0] if first else 'no detail'}"

    entries: list[dict] = []
    # --paginate concatenates one JSON object per page.
    decoder = json.JSONDecoder()
    text = out.stdout.strip()
    idx = 0
    while idx < len(text):
        try:
            page, end = decoder.raw_decode(text, idx)
        except json.JSONDecodeError:
            return "the GitHub API returned something that is not JSON"
        entries.extend(page.get("actions_caches", []))
        idx = end
        while idx < len(text) and text[idx] in " \t\r\n":
            idx += 1
    return entries


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--require",
        action="store_true",
        help="treat an unreachable API as a failure (CI, where a token exists)",
    )
    args = ap.parse_args()

    measured = measure()
    if isinstance(measured, str):
        # The skip path. Exits 0 WITHOUT --require on purpose, which is
        # also why the self-test exists: a bug that always took this
        # branch would leave the budget unguarded while printing
        # something reassuring.
        if args.require:
            print(f"Could not measure the Actions cache: {measured}.")
            print("Passed --require, so this is a failure rather than a skip.")
            return 1
        print(f"Could not measure the Actions cache ({measured}); skipping.")
        print("CI asks with --require, where a token exists.")
        return 0

    problems = verdict(measured)
    total = sum(e["size_in_bytes"] for e in measured) / GIB
    base_total = sum(e["size_in_bytes"] for e in measured if _is_base(e)) / GIB
    held = leftovers(measured)

    # Reported either way, because it is the number that explains a
    # surprising eviction even when the budget itself is fine.
    if held:
        print("Caches held by refs other than the base (not budgeted; they drain")
        print("on merge or after seven idle days, but they DO count against the")
        print(f"{QUOTA_GIB}GB quota):")
        for ref, gb in held.items():
            print(f"  {gb:.2f}GB  {ref}")
        print()

    if problems:
        print(f"The Actions cache is outside its budget ({base_total:.2f}GB on the base ref):")
        for p in problems:
            print(f"  {p}")
        print()
        print("Why this is a failure and not a warning: over GitHub's")
        print(f"{QUOTA_GIB}GB quota, entries are evicted least-recently-used, so a")
        print("run can evict the entry the next run needs. A cache that does not")
        print("fit is slower than no cache at all, because it pays the upload")
        print("too -- and it makes every CI timing unreproducible (#901).")
        return 1

    print(f"The Actions cache is within budget: {base_total:.2f}GB of {TOTAL_BUDGET_GIB}GB")
    print(f"on the base ref ({len(measured)} entries and {total:.2f}GB in total;")
    print(f"GitHub's quota is {QUOTA_GIB}GB).")
    if total > QUOTA_GIB:
        # Worth saying out loud: the budget is met and the repository is
        # STILL over quota, so eviction is happening anyway. That is the
        # leftovers above, and it is information rather than a verdict.
        print()
        print(f"NOTE: the repository is nonetheless over the {QUOTA_GIB}GB quota, so")
        print("eviction is still possible. The excess is on the refs listed above.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
