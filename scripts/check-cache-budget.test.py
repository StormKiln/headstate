#!/usr/bin/env python3
"""Proof that the cache-budget guard can fail, and on what.

The guard's whole job is to turn a ceiling someone CHOSE into a number CI
checks, so the failure that matters is the silent one: a guard that could
not read the API, or read an empty list, and printed something reassuring.
#901 asked for a budget "asserted rather than discovered through a
3x-slower run", and a budget that cannot fail is still discovered.

So the cases below drive `verdict()` -- the pure function that takes
measured entries and returns findings -- rather than the network. Three
things are pinned:

1. Under budget passes, over budget fails. The arithmetic, which is the
   easy half.

2. PER-JOB-CLASS ceilings, not just the total. A single job class that
   doubles is the shape #901 actually observed (`platform-Windows` twice),
   and it can happen while the total is still under 10GB -- so a total-only
   guard would miss the thing that caused the incident.

3. THE BASE-REF SCOPE, which is the subtle one. The budget judges `main`
   and tags only. A branch's own entries are a draining resource nobody
   can fix from a branch -- GitHub reclaims them on merge or after seven
   idle days -- so failing on them would block pull requests for a state
   their authors cannot change, which is the unmergeable shape #887 exists
   to prevent. They are REPORTED by `leftovers()` instead, because they
   still count against the quota and still explain a surprising eviction.
   Both halves are pinned: a foreign-ref duplicate must not fail, and must
   not be silently dropped either.

4. THE FLOOR. An empty measurement must FAIL, not pass. A guard handed
   nothing to check has not found a tidy cache; it has failed to look, and
   this repository has shipped that exact false clean before (#853) --
   which is why `check-mobile-gate.py` carries `KNOWN_GATED` and
   `check-required-contexts.py` asserts its list length.

Run: python3 scripts/check-cache-budget.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("budget_guard", HERE / "check-cache-budget.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []

GB = 1024**3


def entry(key: str, gb: float, ref: str = "refs/heads/main") -> dict:
    return {"key": key, "size_in_bytes": int(gb * GB), "ref": ref}


# One entry per job class at the sizes measured when the budget was set,
# totalling 7.37GB. This is the state the guard must call healthy.
MEASURED = [
    entry("v0-rust-platform-Linux-x64-05562ec6-1ef731a7", 1.65),
    entry("v0-rust-mobile-android-Linux-x64-05562ec6-860d4a2f", 1.52),
    entry("v0-rust-platform-Windows_NT-x64-cca5e066-1ef731a7", 1.48),
    entry("v0-rust-build-Darwin-arm64-141c753f-1ef731a7", 0.86),
    entry("v0-rust-test-rust-Darwin-arm64-141c753f-1ef731a7", 0.61),
    entry("v0-rust-mobile-ios-Darwin-arm64-141c753f-860d4a2f", 0.54),
    entry("v0-rust-lint-Darwin-arm64-141c753f-1ef731a7", 0.40),
    entry("v0-rust-supply-chain-Darwin-arm64-141c753f-1ef731a7", 0.16),
    entry("v0-rust-test-frontend-Darwin-arm64-141c753f-1ef731a7", 0.15),
]


def checks(name: str, entries: list[dict], should_pass: bool) -> None:
    problems = guard.verdict(entries)
    passed = not problems
    if passed != should_pass:
        want = "pass" if should_pass else "fail"
        failures.append(f"{name}\n    expected {want}, got problems={problems!r}")


checks("the measured steady state passes", MEASURED, True)

# THE TOTAL, on the base ref. Two extra generations on `main` is the
# dependency-bump case the budget has to fit or reject.
checks(
    "a base-ref total over budget fails",
    MEASURED
    + [
        entry("v0-rust-platform-Linux-x64-05562ec6-aaaaaaaa", 1.65),
        entry("v0-rust-platform-Windows_NT-x64-cca5e066-aaaaaaaa", 1.48),
    ],
    False,
)

# THE PER-JOB-CLASS CEILING, and the reason the guard is not just a total.
# `platform-Windows` appearing twice is precisely what #901 measured, and
# here the TOTAL is still under budget -- 7.37 + 1.48 = 8.85GB -- so only
# the per-class check can catch it. A total-only guard would have called
# this healthy on the very run that produced the incident.
checks(
    "one job class with two live generations on the base ref fails while the total is still under",
    MEASURED + [entry("v0-rust-platform-Windows_NT-x64-cca5e066-bbbbbbbb", 1.48)],
    False,
)

# THE BASE-REF SCOPE. The same duplicate, but on a pull request's ref, must
# NOT fail: `save-if` stops those being written, GitHub reclaims the ones
# that exist, and no branch author can delete another branch's cache. A
# guard that failed here would block every pull request on a state none of
# them caused -- the #887 shape, bought with a performance guard.
checks(
    "the same duplicate on a foreign ref does NOT fail the budget",
    MEASURED + [entry("v0-rust-platform-Windows_NT-x64-cca5e066-bbbbbbbb", 1.48, "refs/pull/1/merge")],
    True,
)

# ...but it must still be REPORTED, or the guard would hide the very thing
# that explains an eviction while the budget looks healthy. Tolerated is
# not the same as invisible.
held = guard.leftovers(MEASURED + [entry("v0-rust-platform-Windows_NT-x64-cca5e066-bbbbbbbb", 1.48, "refs/pull/1/merge")])
if "refs/pull/1/merge" not in held:
    failures.append("a foreign-ref cache is tolerated but must still be reported by leftovers()")
elif abs(held["refs/pull/1/merge"] - 1.48) > 0.01:
    failures.append(f"leftovers() mis-sized the foreign ref: {held['refs/pull/1/merge']:.2f}GB, expected 1.48GB")
if any(r.startswith("refs/heads/main") for r in held):
    failures.append("leftovers() must not report the base ref as a leftover")

# A tag run saves deliberately (release.yml waits on a tag's CI), so tag
# refs are steady state and ARE budgeted, not leftovers.
if guard.leftovers([entry("v0-rust-lint-Darwin-arm64-141c753f-1ef731a7", 0.4, "refs/tags/v5.14.0")]):
    failures.append("a tag ref is steady state and must not be reported as a leftover")

# A single job class that GREW past its own ceiling, one entry only. This
# is the `cache-targets`/second-root direction: no duplication, just a
# bigger entry, which is what #889 would have done.
checks(
    "a single job class that grew past its ceiling fails",
    [entry("v0-rust-platform-Windows_NT-x64-cca5e066-1ef731a7", 4.0)] + MEASURED[3:],
    False,
)

# THE FLOOR. Nothing measured is not a clean cache, it is a guard that did
# not look -- and the guard must say so rather than printing a reassuring
# 0.00GB.
checks("an empty measurement fails rather than passing vacuously", [], False)

# A job class the budget has never heard of must fail too, rather than
# being silently unbudgeted. A new Rust job is exactly how the ceiling
# gets exceeded without anyone deciding to exceed it, and an unknown class
# slipping through is the vacuous pass wearing a new job's name.
checks(
    "an unbudgeted job class fails rather than going unchecked",
    MEASURED + [entry("v0-rust-platform-FreeBSD-x64-deadbeef-1ef731a7", 1.9)],
    False,
)

# Non-rust-cache entries (a yarn or pip cache added later) are not this
# guard's business and must not be mistaken for an unbudgeted Rust job --
# but they DO count against the quota, so they stay in the total.
checks(
    "a non-rust-cache entry counts toward the total without tripping the class check",
    MEASURED + [entry("node-modules-abc123", 0.3)],
    True,
)


if failures:
    print("check-cache-budget.py self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print("check-cache-budget.py self-test: the budget guard rejects what it should.")
