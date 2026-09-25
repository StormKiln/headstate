#!/usr/bin/env python3
"""The mobile build high-water mark must not lag the builds that shipped.

`.github/mobile-build-high-water-mark` records the highest build number
that has reached a store. `mobile-release.yml`'s Preflight reads it and
refuses a run whose `BUILD_NUMBER` is not above it.

Why this guard exists at all, given Preflight already reads the file:
Preflight only catches a DECREASE. `BUILD_NUMBER` is `github.run_number`,
which climbs by one every run, so a mark that lags by three still passes
`BUILD_NUMBER > HIGH` comfortably -- run 29 clears a mark of 25 exactly
as easily as it clears 28. Staleness is therefore INVISIBLE to the thing
that reads the file. That is why it survived six consecutive chore PRs
(#714, #731, #740, #768, #786, #867) without one release ever failing:
nothing was looking. #787 describes the mark as "stale before every
release", and the gap is real, but its claim that a stale mark "stops the
next release, loudly" is wrong -- that is true only of the rename case
the mark was originally written for (a reset to 1), not of drift.

What a lagging mark actually costs: the file is the number Preflight
NAMES when an upload is refused as a duplicate. Someone debugging a
rejected build reads "already shipped: 25" while the store has seen 29,
and goes looking for the wrong collision. The record is the product here,
so a wrong record is the defect, even though no job turned red.

Why the assets are the right thing to compare against, and not the
record itself: `publish` uploads `Headstate-Companion-<version>-build<N>.ipa`
to the run's pre-release, so the asset name states the build number that
run consumed, written by the run that consumed it. That is a fact nobody
has to remember to update. It is NOT a replacement for the committed file
-- the file is written BEFORE the upload and so survives a later job
failing, while an asset only exists if `publish` finished -- which is
exactly why this is a comparison and not a derivation (#787 option 2
would have dropped the file; that trades a record that cannot
under-report for one that can).

Absent is not zero. If the Releases API cannot be reached, or no mobile
pre-release carries a parseable `build<N>` asset, this guard says it
CANNOT TELL and exits non-zero on request -- it never concludes "the
highest shipped build is 0", which would silently bless any mark at all.
That inversion is this codebase's characteristic bug class, so the
no-evidence path is spelled out rather than left to a falsy default.

Network-dependent, so unlike its neighbours in `lint-deps` this guard is
OPT-IN: with no `--require` it reports what it found and exits 0 when it
could not look. CI passes `--require`, where a token exists and an
unreachable API is a real failure worth seeing.

Why a CI guard rather than #787 option 1, the release workflow committing
the mark back: it cannot. The `main` ruleset is `enforcement: active`
with a `pull_request` rule, `bypass_actors: []` and
`current_user_can_bypass: never`, so no token or App can push to main --
such a step would fail AFTER TestFlight had taken the build. Nor can a
bot open the PR instead: `can_approve_pull_request_reviews` is false, and
`GITHUB_TOKEN` pushes do not trigger workflows, so its nine required
checks (listed in ci.yml above `platform`) would never start and the PR
could never merge. Read from the API, the way #853 established for this
repo -- the branches/protection endpoint 404s here and says nothing about
rulesets, so it is not the thing to check.

This runs in `lint`, which IS one of those nine required contexts, so the
check genuinely blocks rather than merely warning -- #787 proposed a
warning, and six ignored chore PRs are what a warning is worth here.

That last paragraph was right about warnings and wrong about WHERE to
block (#1418). Failing `lint` on a stale mark failed every open PR, and
the merge queue, from the moment a mobile release published until a
one-line PR raised the mark -- a failure unrelated to any PR it landed
on, which in one cycle cost four branches a rebase and meant nothing
could merge until the mark PR did. Nor could the failure move to the
`push` run on main or a scheduled job: the desktop release gate waits
on EVERY check-run attempt on a commit (`wait-for-duplicates`), so one
red attempt on a main commit burns it for releases, and a scheduled
run's check-runs attach to main's head. The failure had to leave the
per-commit checks altogether, not move between them.

So the split is now: warn per commit, enforce at the next mobile
release.

  - Per commit (`lint`, locally and in CI): a mark that LAGS the newest
    shipped build is a `::warning::` annotation naming the one-line fix,
    and exits 0.
  - At release (`--release`, run by mobile-release.yml before the
    build): the same lag is an `::error::` and exits 1. That run
    attaches to no main commit, so it burns nothing, and it is the one
    place staleness guards something -- a release cannot go ahead with
    a record that names the wrong number. The release workflow also
    prints the mark change as a required follow-up in its summary, and
    docs/mobile-release-process.md lists it as a numbered step, so the
    warning is not the only thing asking. Six ignored PRs showed a
    warning alone is not enough; a warning backed by a release that
    will refuse to run is a different thing.
  - Always a hard failure: a missing or unparseable mark file. That is
    not a lag the next release corrects but a record nobody can read,
    and Preflight's shell parser would refuse it too.
  - A mark AHEAD of every asset still passes, per commit and at
    release, for the reason given in `main()`: a consumed number with
    no asset behind it is still consumed. (#1418 listed "ahead" as a
    hard failure to keep; it never was one, and making it one would
    reject the legitimate record of a run whose `publish` failed after
    the store took the build. A mark absurdly far ahead is caught
    anyway: Preflight refuses every `BUILD_NUMBER` at or below it.)

`--require` is unchanged and independent of `--release`: it decides only
what "could not look" means.

Usage:
  check-mobile-build-mark.py            advisory; skips if it cannot look
  check-mobile-build-mark.py --require  a failure to look is a failure
  check-mobile-build-mark.py --release  a lagging mark is a failure
"""

import json
import pathlib
import re
import shutil
import subprocess
import sys

MARK_FILE = pathlib.Path(".github/mobile-build-high-water-mark")

# `publish` names the IPA `Headstate-Companion-<version>-build<N>.ipa`
# (mobile-release.yml, "Upload the IPA"). Anchored on `build<N>` right
# before the extension so a version containing the word cannot match.
ASSET_BUILD = re.compile(r"-build(\d+)\.(?:ipa|aab)$")

# How many of the newest mobile releases to fetch assets for. The answer
# wanted is a MAX and `gh release list` is newest-first, so the newest
# release almost always carries it; the extras cover a re-tag publishing
# out of build order.
PROBE = 5

# Matches mobile-release.yml's own parser, which is the contract: strip
# comment lines, then strip all whitespace. Kept deliberately identical
# so this guard cannot accept a file Preflight would reject.
def read_mark() -> int:
    if not MARK_FILE.exists():
        sys.exit(f"{MARK_FILE} does not exist")
    body = "".join(
        line for line in MARK_FILE.read_text().splitlines(keepends=True)
        if not line.startswith("#")
    )
    value = "".join(body.split())
    if not value.isdigit():
        sys.exit(f"{MARK_FILE} does not contain a build number: {value!r}")
    return int(value)


def shipped_builds() -> tuple[dict[str, int], str | None]:
    """Map mobile tag -> highest build number among its assets.

    Returns ({}, reason) when the question could not be asked, never
    ({}, None) -- an empty map with no reason would read as "nothing has
    shipped", which is the absent-is-not-zero inversion.
    """
    if shutil.which("gh") is None:
        return {}, "the `gh` CLI is not installed"
    try:
        out = subprocess.run(
            ["gh", "release", "list", "--limit", "100", "--json",
             "tagName,isPrerelease"],
            capture_output=True, text=True, timeout=60, check=True,
        ).stdout
    except subprocess.CalledProcessError as e:
        err = (e.stderr or "").strip().splitlines()
        return {}, f"`gh release list` failed: {err[-1] if err else e}"
    except (subprocess.TimeoutExpired, OSError) as e:
        return {}, f"`gh release list` could not run: {e}"

    try:
        releases = json.loads(out)
    except json.JSONDecodeError as e:
        return {}, f"`gh release list` returned unparseable JSON: {e}"

    tags = [r["tagName"] for r in releases
            if str(r.get("tagName", "")).startswith("mobile-v")]
    if not tags:
        return {}, "no mobile-v releases are published"

    # `gh release list` is newest-first, and the answer wanted is a MAX,
    # so only the newest few need their assets fetched -- one `gh release
    # view` per release would otherwise grow without bound (81 releases
    # already, 9 of them mobile). PROBE is comfortably more than the
    # number of mobile releases that could be published out of build
    # order by a re-tag, and the floor below catches it being too small
    # rather than letting a short read pass as an answer.
    builds: dict[str, int] = {}
    for tag in tags[:PROBE]:
        try:
            assets = json.loads(subprocess.run(
                ["gh", "release", "view", tag, "--json", "assets"],
                capture_output=True, text=True, timeout=60, check=True,
            ).stdout)["assets"]
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired,
                OSError, json.JSONDecodeError, KeyError) as e:
            return {}, f"could not read the assets of {tag}: {e}"
        found = [int(m.group(1)) for a in assets
                 if (m := ASSET_BUILD.search(a.get("name", "")))]
        if found:
            builds[tag] = max(found)

    if not builds:
        return {}, (
            f"none of the newest {len(tags[:PROBE])} mobile-v releases "
            f"carry a `-build<N>` asset (of {len(tags)} published)"
        )
    return builds, None


def main(argv: list[str]) -> int:
    require = "--require" in argv[1:]
    release = "--release" in argv[1:]
    mark = read_mark()
    builds, why = shipped_builds()

    if why is not None:
        # Cannot say. Never "therefore 0".
        print(f"Cannot determine the highest shipped build: {why}.")
        print(f"The committed mark is {mark}; it was NOT checked.")
        if require:
            print("\nRun with no --require to treat this as advisory.")
            return 1
        return 0

    tag, highest = max(builds.items(), key=lambda kv: kv[1])
    print(f"Highest build number in a published mobile asset: {highest} ({tag})")
    print(f"Committed mark: {mark}")

    if mark < highest:
        fix = f"set the last line of {MARK_FILE} to {highest} (a one-line PR)."
        print()
        print(f"{MARK_FILE} says {mark}, but build {highest} already shipped")
        print(f"as an asset of {tag}. The mark is STALE.")
        print()
        print("BUILD_NUMBER is github.run_number and climbs every run, so")
        print("it clears a stale mark as easily as a current one; the")
        print("duplicate check alone would never notice (#787). What a stale")
        print(f"mark costs is the record: Preflight names {mark} when an")
        print(f"upload is refused as a duplicate, and the store has seen {highest}.")
        print()
        if release:
            # The one run where failing guards something and burns
            # nothing: a mobile release attaches to no main commit.
            print("A mobile release will not go ahead with a stale record.")
            print("Raise the mark on main, then tag a commit that has it.")
            print(f"::error file={MARK_FILE}::Mobile build mark {mark} lags "
                  f"shipped build {highest} ({tag}). Fix: {fix}")
            return 1
        # Per commit it only warns (#1418): failing here reddened every
        # open PR and the merge queue after each mobile release, and the
        # next mobile release's Preflight enforces it instead.
        print("Not failing this run: staleness is enforced by the next")
        print("mobile release's Preflight (--release), not per commit.")
        print(f"::warning file={MARK_FILE}::Mobile build mark {mark} lags "
              f"shipped build {highest} ({tag}); the next mobile release "
              f"will refuse to run until it is raised. Fix: {fix}")
        return 0

    if mark > highest:
        # Correct and expected: the file is written BEFORE the upload, so
        # a run whose `publish` failed raises the mark with no asset to
        # show for it. A consumed number with no asset is still consumed.
        print()
        print(f"The mark is ahead of the newest asset, which is fine: it is")
        print(f"written before the upload, so build {mark} may have been")
        print("consumed by a run whose `publish` did not finish.")
        return 0

    print("The mark matches the newest shipped build.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
