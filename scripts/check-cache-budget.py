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

---- What FAILS a run, and what only gets printed (#1107) ----

Two findings block: a Rust job class with no declared budget, and a
class that has outgrown its ceiling. Both are fixed by editing this file
or the job. WHERE they block changed in #1505 -- see the last section:
a local `make lint` only warns, and the scheduled job enforces.

Two are the repository's state at that moment and are printed WITHOUT
failing: a base-ref total over budget, and duplicate live generations
left by a dependency bump that already merged. Neither can be caused or
cured from a branch -- nobody can evict a cache entry from a pull
request -- so blocking on them stopped unrelated work for a reason its
author could not act on. That is the cry-wolf shape this project refuses
elsewhere, and dismissing a guard is how a real regression gets through.

The floor is blocking despite not being anybody's fault: a measurement
that did not happen is a broken guard, and a 0.00GB "under budget" would
be the most reassuring possible way to report that (#853).

---- Why a budget guard and not a prune workflow ----

A scheduled REST prune was the other candidate. It is real new surface --
a workflow, a token, a deletion loop that can delete the wrong thing --
and the leak it would paper over has a one-line cause that is now fixed.
Measuring and asserting is the honest first step; if this guard starts
firing on steady state alone, THAT is the evidence that automation is
warranted, and it will arrive with numbers attached.

ASKED AGAIN IN #1107, and the answer is still no, now with the numbers
the paragraph above asked for. Measured 2026-09-22 across six release
tags (v6.0.0, v6.0.1, v7.0.0, v7.1.0, v7.2.0, v7.2.1):

  - rust-cache entries on ANY tag: 2, both on v6.0.0, both created
    2026-09-19, together 1.39GB. v6.0.0 was the LAST release cut before
    #1204 removed tags from `save-if`. Every release since has written
    ZERO rust-cache entries. The recurring leak is already closed at the
    source, and 1.39GB of one-off residue drains on its own clock.

  - What each release still leaves is 10.6MB: a `setup-ruby` bundler
    cache from `lewagon/wait-on-check-action`, which runs inside the
    release gate and caches its own Ruby deps. Six tags hold 63.5MB
    between them -- 0.6% of the quota.

A deletion loop with a token, able to race a release build still holding
the entry it is about to delete, is not a proportionate answer to 0.6%
that GitHub already reclaims after seven idle days. The pruning this
issue imagined would, today, correctly identify 1.39GB of v6.0.0 residue
and then have nothing left to do on any subsequent run.

What WAS broken is the accounting, and that is what #1107 fixed: the
guard could not see tag-held entries at all (see `_is_tag`), so it
reported 7.49GB "within budget" while the repository held 8.88GB. A
budget that excludes a real consumer keeps passing while CI degrades --
which is the actual failure this issue describes.

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

---- WHO MAY WRITE, checked from the tree (#1556) ----

Every other finding here is a measurement. One is not: which refs are
allowed to SAVE is decided by `save-if` in the tree, and it went wrong
once in a way the budget could only see after the damage. #906 let the
merge queue save "because it is the last build before a merge lands";
but a save is scoped to the ref that wrote it, a queue ref is deleted
when its merge lands, and nothing reads the entry again. On 2026-09-27
one such entry, 1.69GB and never accessed after it was written, took
the repository to 9.45GiB of its 10GiB quota.

So `save_policy()` reads the workflow files and requires every
`Swatinem/rust-cache` step to save on `main` ONLY, and every
`actions/cache` step to be restore-only. That finding follows from the
diff under test, needs no network, and so BLOCKS in every mode --
`--advisory` included -- and runs before the API is asked anything.

---- WHERE IT IS ENFORCED, and why a local run only warns (#1505) ----

"Blocking" findings were said to "follow from a diff". From a LOCAL run
they cannot: what this measures is `main`'s cache, and since `save-if`
only `main` writes it (the merge queue stopped in #1556) -- never the
branch being linted. A class over its ceiling reflects a diff that ALREADY MERGED.
So `make lint` failed on every branch for a state none of them caused:
on 2026-09-26 build-Darwin sat at 0.94GB against 0.9, and every local
gate went red until the ceiling moved -- which trains people to read a
red `make lint` as noise.

So the split is now by WHERE, not only by what:

  - `make lint` passes `--advisory`: every finding is printed, over-
    ceiling ones under a WARNING heading, and the exit is 0. The floor
    still fails -- a measurement that returned nothing is a broken guard
    on any machine, not a state of the cache.
  - `.github/workflows/cache-budget.yml` runs it daily with `--require`,
    judging `main` where a ceiling is actually crossed, and fails there.
    That run's check attaches to `main`'s head commit, and the desktop
    release gate reads EVERY check run on a tagged commit -- so its job
    name is in release.yml's `ignore-checks`. Cache size says nothing
    about whether a build is releasable, and must not burn a tag.
"""

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys

GIB = 1024**3

# GitHub's per-repository Actions cache quota. Not ours to choose; the
# budget below has to live inside it.
QUOTA_GIB = 10.0

# Past this fraction of the quota, the total is reported even while it is
# still under (#1556). At 9.45 of 10GiB nothing is evicted yet, but the
# next dependency bump writes a new generation of every class at once --
# the largest alone is ~1.7GB -- so "under quota" was not a reassurance
# anyone should have been given on that day.
QUOTA_WARN_FRACTION = 0.9

# The ceiling for EVERYTHING, chosen with headroom under the quota rather
# than pressed against it. At 7.37GB measured steady state this leaves
# ~1.1GB, which is roughly one spare copy of the largest entry -- enough
# to absorb a single overlapping generation during a dependency bump
# without evicting, which is the event that caused #901's incident.
TOTAL_BUDGET_GIB = 8.5

# Per job class, keyed by the rust-cache key with its trailing lockfile
# hash stripped. First measured on 2026-09-12 and rounded UP to the next
# 0.1GB, so these are ceilings rather than observations.
#
# RE-MEASURED 2026-09-27 (#1505), when two classes had crossed theirs:
# build-Darwin 0.94GB against 0.9, platform-Windows 1.64 against 1.6, and
# platform-Linux 1.69 was 11MB under 1.7. Every class held ONE generation
# on `main`, so this was growth, not a leaked copy. The lockfile since
# 2026-09-12 added four crates (async-compression, compression-codecs,
# compression-core, zlib-rs: #1497's gzip), moved 57 to new versions, and
# the toolchain was pinned to 1.98.1 (#1153). Not tower-http, which was
# already locked, nor refractor, which is an npm package and never enters
# a Rust cache. The whole steady state went 7.37 -> 7.73GB (+4.9%) in 15
# days; the fastest classes (Windows, build, test-rust) ~+10%.
#
# The rule now: the 2026-09-27 figure plus 20%, rounded UP to 0.1GB. At
# the fastest measured rate that is about a month of ordinary dependency
# churn before a ceiling asks to be re-decided, while the step these
# ceilings exist to catch -- a second cache root or uncleaned targets
# (#889), which roughly doubles an entry -- still fails at once. A tighter
# margin re-fires on routine bumps and trains people to wave it through.
#
# The ceilings deliberately sum past TOTAL_BUDGET_GIB: classes do not all
# peak together, and the total is its own check. These catch ONE class
# jumping; the total bounds the repository.
#
#   class                 2026-09-12  2026-09-27  ceiling
#   platform-Linux          1.65        1.69        2.1
#   mobile-android          1.52        1.53        1.9
#   platform-Windows        1.48        1.64        2.0
#   build-Darwin            0.86        0.94        1.2
#   test-rust-Darwin        0.61        0.67        0.9
#   mobile-ios-Darwin       0.54        0.54        0.7
#   lint-Darwin             0.40        0.42        0.5
#   supply-chain-Darwin     0.16        0.16        0.2
#   test-frontend-Darwin    0.15        0.16        0.2
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
    "v0-rust-platform-Linux-x64": 2.1,
    "v0-rust-mobile-android-Linux-x64": 1.9,
    "v0-rust-platform-Windows_NT-x64": 2.0,
    "v0-rust-build-Darwin-arm64": 1.2,
    "v0-rust-test-rust-Darwin-arm64": 0.9,
    "v0-rust-mobile-ios-Darwin-arm64": 0.7,
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


def _is_tag(ref: str) -> bool:
    """Is this cache entry pinned to a release tag?

    MEASURED, not assumed, and the reason this function exists at all.
    The Actions cache API does not report a tag's ref as `refs/tags/v6.0.0`;
    it reports

        refs/heads/refs/tags/v6.0.0

    -- the tag ref nested under `refs/heads/`. So the obvious
    `ref.startswith("refs/tags/")` is DEAD CODE that never matched once,
    and every tag-held entry fell through to `leftovers()` instead of
    being budgeted. That is how the guard reported 7.49GB "within budget"
    on a day the repository was actually holding 8.88GB against an 8.5GB
    ceiling: the 1.39GB it was not counting was real, resident and
    counting against the quota the whole time (#1107).

    Both spellings are accepted. The nested one is what the API returns
    today; the flat one is what the documentation implies, and pinning
    only the observed spelling would leave this silently broken again if
    GitHub ever normalised it.
    """
    tail = ref[len("refs/heads/") :] if ref.startswith("refs/heads/") else ref
    return tail.startswith("refs/tags/")


def _is_base(entry: dict) -> bool:
    ref = entry.get("ref", "")
    return ref == BASE_REF or _is_tag(ref)


# A merge-queue ref as the caches API reports it:
# `refs/heads/gh-readonly-queue/main/pr-1499-<sha>`. Matched on the
# segment rather than a full prefix so the flat `refs/heads/` spelling and
# any nesting the API adds (it nests tags, see `_is_tag`) both match.
QUEUE_SEGMENT = "gh-readonly-queue/"


def _is_queue(ref: str) -> bool:
    return QUEUE_SEGMENT in ref


# The only `save-if` a rust-cache step may carry (#1556). An exact string
# rather than a parse of the expression: the question is "did someone
# widen who saves", and any edit to this line is exactly that question.
MAIN_ONLY_SAVE_IF = "${{ github.ref == 'refs/heads/main' }}"

_RUST_CACHE_USE = re.compile(r"^\s*(?:-\s+)?uses:\s*Swatinem/rust-cache@")
# `actions/cache@` and `actions/cache/save@` both write; only
# `actions/cache/restore@` is read-only.
_ACTIONS_CACHE_WRITE = re.compile(r"^\s*(?:-\s+)?uses:\s*actions/cache(?:/save)?@")
_SAVE_IF = re.compile(r"^\s*save-if:\s*(.*?)\s*$")


def _step_lines(lines: list[str], at: int) -> list[str]:
    """The lines of the step whose `uses:` is on line `at`, `uses:` excluded.

    The step's keys sit at the column `uses` starts at; it continues
    through deeper-indented lines, blanks and comments, and ends at the
    first line indented LESS -- which is also where the next `- ` item
    begins.
    """
    key_col = lines[at].index("uses:")
    body: list[str] = []
    for nxt in lines[at + 1 :]:
        stripped = nxt.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if len(nxt) - len(nxt.lstrip()) < key_col:
            break
        body.append(nxt)
    return body


def save_policy(files: dict[str, str]) -> list[str]:
    """Findings for any cache step that may WRITE on a ref other than `main`.

    `files` maps a path (for the message) to its text. Pure, so the
    self-test can drive it with fixtures; `workflow_files()` supplies the
    real tree.
    """
    findings: list[str] = []
    rust_cache_steps = 0
    for path, text in sorted(files.items()):
        lines = text.replace("\r\n", "\n").split("\n")
        for i, line in enumerate(lines):
            if line.lstrip().startswith("#"):
                continue
            if _ACTIONS_CACHE_WRITE.match(line):
                findings.append(
                    f"{path}:{i + 1} uses a cache action that SAVES on whatever ref runs "
                    f"it. Use `actions/cache/restore`, or save from `main` only and "
                    f"extend `save_policy()` to recognise how (#1556)"
                )
            if not _RUST_CACHE_USE.match(line):
                continue
            rust_cache_steps += 1
            values = [m.group(1) for m in map(_SAVE_IF.match, _step_lines(lines, i)) if m]
            if not values:
                findings.append(
                    f"{path}:{i + 1} runs `Swatinem/rust-cache` with no `save-if`, and "
                    f"its default saves on EVERY ref -- pull requests, tags and the "
                    f"merge queue each write an entry only they can read (#901, #1556). "
                    f"Add `save-if: {MAIN_ONLY_SAVE_IF}`"
                )
            elif values[-1] != MAIN_ONLY_SAVE_IF:
                findings.append(
                    f"{path}:{i + 1} has `save-if: {values[-1]}`; it must be exactly "
                    f"`{MAIN_ONLY_SAVE_IF}`. An entry is scoped to the ref that writes "
                    f"it, so any other ref's save is read by nothing but that ref -- a "
                    f"merge-queue run's 1.69GB entry was never read once (#1556)"
                )
    # The floor, as everywhere in this file: finding no rust-cache step at
    # all is not a clean policy, it is a scan that looked in the wrong place.
    if rust_cache_steps == 0:
        findings.append(
            "no `Swatinem/rust-cache` step was found in the workflow files, so the "
            "save policy was not checked at all. .github/actions/setup/action.yml "
            "is where it lives; if it moved, point `workflow_files()` at it"
        )
    return findings


def workflow_files() -> dict[str, str]:
    """Every composite action and workflow in the tree, by repo-relative path."""
    root = pathlib.Path(__file__).resolve().parent.parent
    github = root / ".github"
    paths: list[pathlib.Path] = []
    for pattern in ("actions/*/action.yml", "actions/*/action.yaml", "workflows/*.yml", "workflows/*.yaml"):
        paths += sorted(github.glob(pattern))
    return {p.relative_to(root).as_posix(): p.read_text(encoding="utf-8") for p in paths}


def verdict(entries: list[dict]) -> tuple[list[str], list[str]]:
    """BUDGET findings, split by WHO CAN FIX THEM (#1107).

    Returns `(blocking, ambient)`.

    `blocking` is what the branch under test is answerable for: a new job
    class with no budget, or a class that has outgrown its ceiling. Both
    are consequences of a diff, and both are fixed by editing this file
    or the job.

    `ambient` is the repository's state at this moment -- duplicate live
    generations left by a dependency bump, or a base-ref total over
    budget. Real, worth printing, and NOT the fault of whoever happens to
    be running `make lint`. Failing on them blocks unrelated work for a
    reason the author cannot act on, and a check that cries wolf is one
    people learn to dismiss -- which is how a real regression gets
    through.

    That split is the whole of #1107's second question. The first (tags
    holding 2.84GB) is fixed in `setup/action.yml`.

    Judges the STEADY STATE -- `main` and tags -- because that is the part
    this repository controls and the part `save-if` now guarantees is the
    only part. Entries on other refs are reported separately by
    `leftovers()`: they are a draining resource nobody can fix from a
    branch, so failing on them would be the cry-wolf shape this project
    refuses elsewhere.

    Pure on purpose -- it takes the parsed API rows and makes every
    decision, so the self-test can drive it without a network or a token.
    """
    blocking: list[str] = []
    ambient: list[str] = []

    # THE FLOOR, first and before any arithmetic. No entries is not a tidy
    # cache; it is a guard that failed to look, and a 0.00GB "under
    # budget" would be the most reassuring possible way to report that
    # (#853). Every sibling guard asserts a floor for the same reason.
    #
    # Asserted on the RAW list, before the base-ref filter: "the API
    # returned nothing" and "main happens to hold nothing right now" are
    # different facts, and only the first is a broken measurement.
    if not entries:
        # BLOCKING even though it is not the branch's doing: a guard that
        # cannot see is not a guard, and passing it would be the most
        # reassuring possible way to report a broken check (#853).
        blocking.append(
            "no cache entries were measured at all. That is not an empty cache, "
            "it is a measurement that did not happen -- a budget check with "
            "nothing in it passes trivially and tells you nothing."
        )
        return blocking, ambient

    base = [e for e in entries if _is_base(e)]

    total = sum(e["size_in_bytes"] for e in base) / GIB
    if total > TOTAL_BUDGET_GIB:
        # AMBIENT: the sum of what the repository is holding right now.
        # A branch cannot evict an entry.
        ambient.append(
            f"the budgeted refs hold {total:.2f}GB, over the {TOTAL_BUDGET_GIB}GB budget "
            f"(GitHub's quota is {QUOTA_GIB}GB for the whole repository, and over it "
            f"entries are evicted least-recently-used -- which is how a 562s job "
            f"became 1716s in #901)"
        )

    # THE QUOTA, on EVERYTHING -- including the entries no budget covers.
    #
    # The budget is a ceiling someone chose; the quota is the one GitHub
    # enforces, and eviction obeys the second. An entry outside every
    # budgeted class still occupies the same 10GB: the `setup-ruby`
    # entries that `lewagon/wait-on-check-action` leaves on each release
    # tag are only ~11MB apiece, but `job_class()` returns None for them
    # and nothing was adding them up at all.
    #
    # Without this, a budget that excludes a real consumer keeps
    # reporting "within budget" while the repository evicts -- which is
    # precisely #1107's third question, and precisely what this guard
    # printed on the day it was filed.
    quota_used = sum(e["size_in_bytes"] for e in entries) / GIB
    if quota_used > QUOTA_GIB:
        # AMBIENT: over the real ceiling, and nothing a branch can do.
        ambient.append(
            f"the repository holds {quota_used:.2f}GB in total, over GitHub's "
            f"{QUOTA_GIB}GB quota. Eviction is happening NOW, least-recently-used, "
            f"so a run can evict the entry the next run needs (#901). This counts "
            f"every entry, including any no budget covers -- the quota does not "
            f"care which class an entry belongs to"
        )

    elif quota_used >= QUOTA_WARN_FRACTION * QUOTA_GIB:
        # AMBIENT: under the quota, but not by enough to absorb one
        # dependency bump, which writes a new generation of every class.
        largest = max(e["size_in_bytes"] for e in entries) / GIB
        ambient.append(
            f"the repository holds {quota_used:.2f}GB in total, "
            f"{100 * quota_used / QUOTA_GIB:.0f}% of GitHub's {QUOTA_GIB}GB quota. "
            f"Nothing is evicted yet, but the largest entry alone is {largest:.2f}GB, "
            f"and a dependency bump writes a new generation of every class at once "
            f"(#1556)"
        )

    # MERGE-QUEUE ENTRIES, which nothing reads (#1556). `save_policy()`
    # stops new ones being written; this is the live-data half, so a
    # regression that slipped past it -- or residue from before it -- is
    # named rather than folded anonymously into `leftovers()`.
    queued: dict[str, float] = {}
    for e in entries:
        ref = e.get("ref", "")
        if _is_queue(ref):
            queued[ref] = queued.get(ref, 0.0) + e["size_in_bytes"] / GIB
    if queued:
        where = ", ".join(f"{ref} ({gb:.2f}GB)" for ref, gb in sorted(queued.items()))
        ambient.append(
            f"merge-queue refs hold {sum(queued.values()):.2f}GB: {where}. A queue ref "
            f"is deleted when its merge lands, so nothing ever reads these. Queue runs "
            f"are restore-only since #1556: an entry written after that means "
            f"`save-if` regressed; one written before it is residue that drains after "
            f"seven idle days, or can be deleted by id through the caches API"
        )

    # Group by class so both "one entry grew" and "two generations are
    # live" are visible, since those need different fixes.
    by_class: dict[str, list[dict]] = {}
    for e in base:
        cls = job_class(e["key"])
        if cls is not None:
            by_class.setdefault(cls, []).append(e)

    for cls, rows in sorted(by_class.items()):
        # Does this class exist ONLY on a tag? Tags are budgeted because
        # they consume the quota, but nothing on a tag follows from the
        # diff under test: `save-if` stopped tags writing rust-cache
        # entries (#1204), so any that remain are residue from a release
        # cut before that landed, draining on the seven-day idle clock.
        # Routing them to `blocking` would fail every branch for a state
        # no branch caused and none can clear -- the cry-wolf shape this
        # guard's whole ambient/blocking split exists to avoid (#1107).
        tag_only = all(_is_tag(r.get("ref", "")) for r in rows)

        if cls not in CLASS_BUDGET_GIB:
            size = sum(r["size_in_bytes"] for r in rows) / GIB
            if tag_only:
                # AMBIENT: a stale release's entry, not a new job class.
                ambient.append(
                    f"`{cls}` has no budget ({size:.2f}GB measured) and exists only on "
                    f"a release tag. That is residue from a release cut before tags "
                    f"stopped saving (#1204) -- it counts against the quota until its "
                    f"seven-day idle window expires, and no branch can clear it sooner"
                )
                continue
            # BLOCKING: a job class only appears because a diff added
            # one, and the fix is in this file.
            blocking.append(
                f"`{cls}` has no budget ({size:.2f}GB measured). A new Rust job is "
                f"how the ceiling gets exceeded without anyone deciding to -- add "
                f"it to CLASS_BUDGET_GIB with a measured figure, and check the "
                f"total still fits"
            )
            continue

        budget = CLASS_BUDGET_GIB[cls]
        if len(rows) > 1:
            size = sum(r["size_in_bytes"] for r in rows) / GIB
            # AMBIENT: the old generation was left resident by a bump
            # that has already merged. It drains on its own; a branch
            # can neither cause nor cure it.
            # Name the refs rather than asserting `main`. Now that tags
            # are counted, "2 live generations" can mean one on `main`
            # and one pinned to a tag, which is a DIFFERENT fact with a
            # different remedy -- and the old wording stated the wrong
            # one confidently.
            where = ", ".join(sorted({r.get("ref", "?") for r in rows}))
            ambient.append(
                f"`{cls}` has {len(rows)} live generations totalling {size:.2f}GB "
                f"across {where}. Two generations of one class on a ref that is "
                f"restored from is the state that evicts (#901) -- either a "
                f"dependency bump left the old generation resident, or a release "
                f"tag is still holding one; the budget must fit 2x or the old one "
                f"must go"
            )
        for r in rows:
            size = r["size_in_bytes"] / GIB
            if size > budget:
                if _is_tag(r.get("ref", "")):
                    # AMBIENT: a tag's entry is frozen at whatever the
                    # release measured. No diff can shrink it.
                    ambient.append(
                        f"`{cls}` is {size:.2f}GB on {r['ref']}, over its {budget}GB "
                        f"ceiling ({r['key']}). It is pinned to a release tag, so it "
                        f"is frozen at what that release measured and drains on the "
                        f"idle clock rather than by anyone's edit"
                    )
                    continue
                # BLOCKING: either what this job caches grew -- which a
                # diff can do -- or the ceiling is wrong. Both are
                # decided here.
                blocking.append(
                    f"`{cls}` is {size:.2f}GB, over its {budget}GB ceiling "
                    f"({r['key']}). Either what it caches grew, or the ceiling was "
                    f"set too tight -- decide which, and move the number on purpose"
                )

    return blocking, ambient


def leftovers(entries: list[dict]) -> dict[str, float]:
    """{ref: GB} for caches held by refs other than the base, largest first.

    Reported, never failed on. Since `save-if` these should only be
    entries predating it, or ones a third-party action writes on its own
    (the release gate's `setup-ruby` bundler cache on each tag), and GitHub
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
    ap.add_argument(
        "--advisory",
        action="store_true",
        help=(
            "report over-budget findings as a warning and exit 0 (make lint: "
            "a local run measures main's cache, which no branch writes). The "
            "floor -- nothing measured -- still fails."
        ),
    )
    args = ap.parse_args()

    # The save policy first, and in every mode: it is read from the tree,
    # so it needs no network and follows from the diff under test (#1556).
    policy = save_policy(workflow_files())
    if policy:
        print("A cache step may write on a ref other than `main`:")
        for p in policy:
            print(f"  {p}")
        print()
        print("Failing on this in every mode, --advisory included: unlike the")
        print("cache's size, this is decided by the branch under test.")
        return 1

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

    return report(measured, args.advisory)


def report(measured: list[dict], advisory: bool) -> int:
    """Print what `measured` shows and return the exit code.

    Split from `main` so the self-test can drive the exit code in both
    modes without a network: `--advisory` changing what FAILS is the
    whole of #1505, and a mode that silently always returned 0 would pass
    every run while guarding nothing.
    """
    blocking, ambient = verdict(measured)
    total = sum(e["size_in_bytes"] for e in measured) / GIB
    base_total = sum(e["size_in_bytes"] for e in measured if _is_base(e)) / GIB
    held = leftovers(measured)

    # Usage against the QUOTA, first and unconditionally (#1556). The
    # budget below is a ceiling someone chose; the quota is the one GitHub
    # evicts at, and it counts every entry, budgeted or not.
    print(
        f"Actions cache: {total:.2f}GB of GitHub's {QUOTA_GIB}GB quota "
        f"({100 * total / QUOTA_GIB:.0f}%) across {len(measured)} entries; "
        f"{base_total:.2f}GB of that on `main` and release tags."
    )
    print()

    # Reported either way, because it is the number that explains a
    # surprising eviction even when the budget itself is fine.
    if held:
        print("Caches held by pull-request and other non-release refs (not")
        print("budgeted; they drain on merge or after seven idle days, but they")
        print(f"DO count against the {QUOTA_GIB}GB quota):")
        for ref, gb in held.items():
            print(f"  {gb:.2f}GB  {ref}")
        print()

    # AMBIENT first: printed whether or not anything blocks, because it
    # is the state that explains a surprising eviction -- and printed as
    # a NOTICE, because a branch cannot act on it (#1107).
    if ambient:
        print(
            f"The repository's cache is outside its budget or near its quota "
            f"({base_total:.2f}GB on `main` and release tags, {total:.2f}GB in total):"
        )
        for p in ambient:
            print(f"  {p}")
        print()
        print("NOT failing on this. It is the repository's state right now, not")
        print("anything this branch did: a branch cannot evict an entry or drain")
        print("a generation left by a bump that already merged. Blocking here")
        print("stops unrelated work for a reason its author cannot fix, and a")
        print("check that cries wolf is one people learn to dismiss -- which is")
        print("how a real regression gets through (#1107).")
        print()
        print("It still matters: over GitHub's")
        print(f"{QUOTA_GIB}GB quota, entries are evicted least-recently-used, so a")
        print("run can evict the entry the next run needs, which is how a 562s")
        print("job became 1716s (#901). If it persists past a few days, that is")
        print("a steady state to fix rather than a bump draining.")
        print()

    if blocking:
        # `measured` non-empty: the floor is not a state of the cache but a
        # measurement that did not happen, so it fails in every mode.
        if advisory and measured:
            print("WARNING -- `main`'s cache is over its budget:")
            for p in blocking:
                print(f"  {p}")
            print()
            print("Not failing this local run (#1505): it measured `main`'s cache,")
            print("which no branch writes, so nothing here follows from this")
            print("branch. The scheduled `cache-budget` workflow enforces it on")
            print("`main`. The fix -- a new ceiling or a trimmed job -- is still")
            print("decided in this file, by whoever takes it on.")
            return 0
        print("The cache is over its budget:")
        for p in blocking:
            print(f"  {p}")
        print()
        print("Failing on this: unlike the notice above, these are decided in")
        print("this file -- a job class with no declared budget, or one that")
        print("has outgrown its ceiling. Either what it caches grew, or the")
        print("ceiling is wrong; move the number on purpose.")
        return 1

    # Only claim "within budget" when it IS. Before #1107 this line
    # printed unconditionally whenever nothing BLOCKED, so a run that had
    # just reported an over-budget notice signed off with "within
    # budget: 8.88GB of 8.5GB" -- a self-contradicting summary, and the
    # last line is the one people read. Ambient findings are not
    # failures, but they are not a clean bill of health either.
    if ambient:
        print(f"Measured {len(measured)} entries, {total:.2f}GB in total against a")
        print(f"{QUOTA_GIB}GB quota. Not blocking this branch; see the notice above.")
        return 0

    print(f"The Actions cache is within budget: {base_total:.2f}GB of {TOTAL_BUDGET_GIB}GB")
    print(f"on `main` and release tags ({len(measured)} entries and {total:.2f}GB in")
    print(f"total; GitHub's quota is {QUOTA_GIB}GB).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
