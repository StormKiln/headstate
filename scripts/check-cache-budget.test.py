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
    """Assert whether this state BLOCKS a branch.

    Since #1107 `verdict` returns `(blocking, ambient)`, and only
    `blocking` fails a run. `should_pass=True` therefore means "does not
    block" -- it does NOT mean "found nothing", which is what `ambient`
    exists to record. Cases that produce an ambient notice assert it
    explicitly with `ambient_only` below, so "tolerated" can never be
    confused with "invisible".
    """
    blocking, _ambient = guard.verdict(entries)
    passed = not blocking
    if passed != should_pass:
        want = "pass" if should_pass else "fail"
        failures.append(f"{name}\n    expected {want}, got blocking={blocking!r}")


def ambient_only(name: str, entries: list[dict]) -> None:
    """Assert this state is REPORTED but does not block (#1107).

    The distinction the whole split exists for: a branch cannot evict a
    cache entry or drain a generation left by a bump that already
    merged, so blocking on one stops unrelated work for a reason its
    author cannot fix. Asserting `ambient` is non-empty is what keeps
    "not blocking" from silently becoming "not noticing".
    """
    blocking, ambient = guard.verdict(entries)
    if blocking:
        failures.append(f"{name}\n    expected no blocking findings, got {blocking!r}")
    if not ambient:
        failures.append(f"{name}\n    expected an ambient notice, got none")


checks("the measured steady state passes", MEASURED, True)

# THE TOTAL, on the base ref. Two extra generations on `main` is the
# dependency-bump case the budget has to fit or reject.
ambient_only(
    "a base-ref total over budget is reported, not blocked on (#1107)",
    MEASURED
    + [
        entry("v0-rust-platform-Linux-x64-05562ec6-aaaaaaaa", 1.65),
        entry("v0-rust-platform-Windows_NT-x64-cca5e066-aaaaaaaa", 1.48),
    ],
)

# THE PER-JOB-CLASS CEILING, and the reason the guard is not just a total.
# `platform-Windows` appearing twice is precisely what #901 measured, and
# here the TOTAL is still under budget -- 7.37 + 1.48 = 8.85GB -- so only
# the per-class check can catch it. A total-only guard would have called
# this healthy on the very run that produced the incident.
ambient_only(
    "two live generations on the base ref are reported while the total is still under",
    MEASURED + [entry("v0-rust-platform-Windows_NT-x64-cca5e066-bbbbbbbb", 1.48)],
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

# ...AND IN THE SPELLING THE API ACTUALLY USES, which is the whole of
# #1107's third question.
#
# The assertion above passes for the wrong reason: it uses the idealised
# `refs/tags/v5.14.0`, and `_is_base` matched that. The live API returns
# the tag ref NESTED under refs/heads:
#
#     refs/heads/refs/tags/v6.0.0
#
# which `startswith("refs/tags/")` never matched, so every real tag entry
# fell out of the budget and into `leftovers()`. The guard then reported
# 7.49GB "within budget" while the repository held 8.88GB against an
# 8.5GB ceiling -- 1.39GB uncounted, resident, and consuming the quota.
#
# Testing only the tidy spelling is what let that ship, so both are
# pinned here.
NESTED_TAG = "refs/heads/refs/tags/v6.0.0"
if guard.leftovers([entry("v0-rust-lint-Darwin-arm64-141c753f-1ef731a7", 0.4, NESTED_TAG)]):
    failures.append(
        "a tag in the API's real ref shape must be budgeted, not filed as a leftover"
    )
if not guard._is_base({"ref": NESTED_TAG}):
    failures.append(f"_is_base must recognise the API's real tag shape {NESTED_TAG!r}")

# A tag entry must COUNT toward the total, which is the consequence that
# actually matters: an entry nobody adds up still occupies the quota.
_blocking, _amb = guard.verdict(MEASURED + [entry("v0-rust-lint-Darwin-arm64-141c753f-99999999", 1.3, NESTED_TAG)])
if not any("over the" in a and "budget" in a for a in _amb):
    failures.append(
        "a tag-held entry pushing the total over budget must raise an ambient notice"
    )

# But a tag-only class must NOT block. Nothing on a tag follows from the
# diff under test and no branch can delete it, so blocking would be the
# cry-wolf shape the ambient/blocking split exists to prevent.
ambient_only(
    "an unbudgeted class that exists ONLY on a tag is reported, not blocked on",
    MEASURED + [entry("v0-rust-build-Windows_NT-x64-cca5e066-eeeeeeee", 0.67, NESTED_TAG)],
)

# The same unbudgeted class on `main` still BLOCKS. This is the guard
# against "fix the noise by making everything ambient": if tag-routing
# had been written as a blanket downgrade, this case would go quiet too.
checks(
    "the same unbudgeted class on `main` still blocks",
    MEASURED + [entry("v0-rust-build-Windows_NT-x64-cca5e066-eeeeeeee", 0.67)],
    False,
)

# THE QUOTA, counted over EVERY entry including ones no job class covers.
# The `setup-ruby` entries that wait-on-check-action leaves on each
# release tag return None from `job_class()`, so no per-class rule sees
# them -- but GitHub evicts on the quota, which does not care.
# Constructed so ONLY the quota rule can fire: the unclassified entry sits
# on a PULL-REQUEST ref, so it is outside the budgeted set entirely and the
# base-ref budget stays under its 8.5GB ceiling. The repository is
# nonetheless over the 10GB quota, and nothing but the quota rule sees it.
# Asserting on the distinctive wording rather than the word "quota" (which
# the budget notice also contains) is what makes this test able to fail.
_over_quota = MEASURED + [
    entry("setup-ruby-bundler-cache-v6-ubuntu-24.04-x64", 3.0, "refs/pull/9/merge")
]
_base_gb = sum(e["size_in_bytes"] for e in _over_quota if guard._is_base(e)) / GB
_all_gb = sum(e["size_in_bytes"] for e in _over_quota) / GB
if _base_gb > guard.TOTAL_BUDGET_GIB:
    failures.append(
        f"the quota fixture must keep the BUDGET satisfied to isolate the quota "
        f"rule, but the base ref holds {_base_gb:.2f}GB"
    )
if _all_gb <= guard.QUOTA_GIB:
    failures.append(f"the quota fixture must exceed the quota, got {_all_gb:.2f}GB")

_blocking, _amb = guard.verdict(_over_quota)
if not any("in total, over GitHub's" in a for a in _amb):
    failures.append(
        "an unclassified entry pushing the repository over quota must be reported "
        f"even while the budget is satisfied; got {_amb!r}"
    )
if _blocking:
    failures.append(f"an unclassified entry must not block, got {_blocking!r}")

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


# ---- THE BLOCKING HALF still blocks (#1107) ----
#
# The split is only worth having if the branch-caused findings still
# fail. A guard that reclassified everything as ambient would print
# tidily and stop catching anything, which is the failure mode of
# "make the check less annoying".

# A new Rust job class with no declared budget. A diff adds one; the fix
# is in `CLASS_BUDGET_GIB`.
checks(
    "an unbudgeted job class BLOCKS, because a diff introduced it",
    MEASURED + [entry("v0-rust-brandnewjob-Linux-x64-05562ec6-cccccccc", 0.4)],
    False,
)

# A class that has outgrown its ceiling. Either what it caches grew --
# which a diff can do -- or the ceiling was set too tight.
checks(
    "a class over its ceiling BLOCKS, because a diff can cause it",
    [
        e
        for e in MEASURED
        if guard.job_class(e["key"]) != "platform-Linux"
    ]
    + [entry("v0-rust-platform-Linux-x64-05562ec6-dddddddd", 9.0)],
    False,
)

# The floor. A measurement that did not happen is not ambient state --
# it is a broken guard, and passing it would be the most reassuring
# possible way to report that (#853).
checks("an empty measurement BLOCKS rather than passing trivially", [], False)


# ---- 2026-09-27's re-measurement (#1505) ----
#
# The figures the ceilings were re-decided on, one generation per class on
# `main`, as the API returned them. The new ceilings must call this
# healthy; the old ones did not (build-Darwin and platform-Windows were
# over), which is the state that turned every local `make lint` red.
MEASURED_0927 = [
    entry("v0-rust-platform-Linux-x64-6ff13d87-84ed4606", 1.689),
    entry("v0-rust-mobile-android-Linux-x64-6ff13d87-15890f27", 1.531),
    entry("v0-rust-platform-Windows_NT-x64-2113753f-704831f1", 1.635),
    entry("v0-rust-build-Darwin-arm64-2eab217e-84ed4606", 0.941),
    entry("v0-rust-test-rust-Darwin-arm64-2eab217e-84ed4606", 0.670),
    entry("v0-rust-mobile-ios-Darwin-arm64-2eab217e-15890f27", 0.537),
    entry("v0-rust-lint-Darwin-arm64-2eab217e-84ed4606", 0.416),
    entry("v0-rust-supply-chain-Darwin-arm64-2eab217e-84ed4606", 0.159),
    entry("v0-rust-test-frontend-Darwin-arm64-2eab217e-84ed4606", 0.155),
]
checks("the 2026-09-27 steady state passes the re-decided ceilings", MEASURED_0927, True)

# The rule the ceilings were set by: measured + 20%, rounded UP to 0.1GB.
# Pinned so a later edit that moves one number without re-measuring shows
# up here as a disagreement with its own comment.
for e in MEASURED_0927:
    cls = guard.job_class(e["key"])
    gb = e["size_in_bytes"] / GB
    want = -(-round(gb * 1.2 * 10, 6) // 1) / 10
    if abs(guard.CLASS_BUDGET_GIB[cls] - want) > 1e-9:
        failures.append(
            f"`{cls}`'s ceiling is {guard.CLASS_BUDGET_GIB[cls]}GB, but its 2026-09-27 "
            f"figure {gb:.3f}GB + 20% rounds up to {want}GB"
        )


# ---- WHERE it fails: `--advisory` (#1505) ----
#
# `report()` is `main()` after the measurement, so the exit code can be
# pinned in both modes without a network. Output is swallowed; the codes
# are what the Makefile and the scheduled workflow act on.
import contextlib
import io


def exit_code(entries: list[dict], advisory: bool) -> int:
    with contextlib.redirect_stdout(io.StringIO()) as out:
        code = guard.report(entries, advisory)
    exit_code.last = out.getvalue()
    return code


OVER = [e for e in MEASURED_0927 if "build-Darwin" not in e["key"]] + [
    entry("v0-rust-build-Darwin-arm64-2eab217e-84ed4606", 5.0)
]
if exit_code(OVER, advisory=False) != 1:
    failures.append("a class over its ceiling must FAIL the enforcing run (scheduled job)")
if exit_code(OVER, advisory=True) != 0:
    failures.append("a class over its ceiling must only WARN under --advisory (make lint)")
elif "WARNING" not in exit_code.last or "build-Darwin" not in exit_code.last:
    failures.append(
        "--advisory must still PRINT the over-ceiling finding, naming the class; "
        f"got {exit_code.last!r}"
    )
# The floor is a broken measurement, not a cache state: it fails in BOTH
# modes, or `--advisory` would be a way to pass a guard that cannot see.
if exit_code([], advisory=True) != 1:
    failures.append("an empty measurement must fail even under --advisory")
if exit_code([], advisory=False) != 1:
    failures.append("an empty measurement must fail the enforcing run")
if exit_code(MEASURED_0927, advisory=False) != 0:
    failures.append("the healthy 2026-09-27 state must pass the enforcing run")


# ---- WHO MAY WRITE (#1556) ----
#
# `save_policy()` reads the tree, not the API. The merge queue saved from
# #906 on, because #906 argued it should, and one of its entries --
# 1.69GB, never read -- took the repository to 94% of the quota. Each
# case below is a way that could come back.

MAIN_ONLY = guard.MAIN_ONLY_SAVE_IF


def rust_cache_step(save_if: str | None, then: str = "") -> str:
    lines = [
        "runs:",
        "  using: composite",
        "  steps:",
        "    - uses: Swatinem/rust-cache@0000000000000000000000000000000000000000 # v2",
        "      with:",
        "        workspaces: src-tauri",
        "        # save-if: ${{ true }}   <- a comment, never a setting",
    ]
    if save_if is not None:
        lines.append(f"        save-if: {save_if}")
    return "\n".join(lines) + "\n" + then


def policy_blocks(name: str, files: dict[str, str], should_block: bool) -> None:
    got = guard.save_policy(files)
    if bool(got) != should_block:
        want = "a finding" if should_block else "no finding"
        failures.append(f"{name}\n    expected {want}, got {got!r}")


# The tree as committed must satisfy its own policy -- the half that
# fails if the fix is reverted.
policy_blocks("the committed workflows save on `main` only", guard.workflow_files(), False)
if not any("rust-cache" in t for t in guard.workflow_files().values()):
    failures.append("workflow_files() did not find the setup action's rust-cache step")

policy_blocks("main-only save-if passes", {"a.yml": rust_cache_step(MAIN_ONLY)}, False)
policy_blocks(
    "the pre-#1556 merge-queue save-if is rejected",
    {"a.yml": rust_cache_step(
        "${{ github.ref == 'refs/heads/main' || github.event_name == 'merge_group' }}"
    )},
    True,
)
policy_blocks(
    "a rust-cache step with NO save-if is rejected (its default saves everywhere)",
    {"a.yml": rust_cache_step(None)},
    True,
)
# A save-if in the NEXT step must not be credited to this one, or a step
# boundary bug would pass a step that saves on every ref.
policy_blocks(
    "a later step's save-if does not satisfy an earlier rust-cache step",
    {"a.yml": rust_cache_step(
        None,
        "    - uses: some/other@0000000000000000000000000000000000000000\n"
        f"      with:\n        save-if: {MAIN_ONLY}\n",
    )},
    True,
)
# The `- name:` then `uses:` layout, which workflows use more than the
# composite action does.
policy_blocks(
    "a named step's rust-cache is found and checked",
    {"w.yml": (
        "jobs:\n  j:\n    steps:\n      - name: cache\n"
        "        uses: Swatinem/rust-cache@0000000000000000000000000000000000000000\n"
        "        with:\n          save-if: ${{ true }}\n"
    )},
    True,
)
policy_blocks(
    "CRLF line endings are read the same",
    {"a.yml": rust_cache_step(MAIN_ONLY).replace("\n", "\r\n")},
    False,
)
policy_blocks(
    "actions/cache (which saves) is rejected",
    {"a.yml": rust_cache_step(MAIN_ONLY),
     "w.yml": "steps:\n  - uses: actions/cache@0000000000000000000000000000000000000000\n"},
    True,
)
policy_blocks(
    "actions/cache/save is rejected",
    {"a.yml": rust_cache_step(MAIN_ONLY),
     "w.yml": "steps:\n  - uses: actions/cache/save@0000000000000000000000000000000000000000\n"},
    True,
)
policy_blocks(
    "actions/cache/restore is read-only and passes",
    {"a.yml": rust_cache_step(MAIN_ONLY),
     "w.yml": "steps:\n  - uses: actions/cache/restore@0000000000000000000000000000000000000000\n"},
    False,
)
# THE FLOOR: no rust-cache step found is a scan that looked in the wrong
# place, not a clean policy.
policy_blocks("finding no rust-cache step at all is a finding", {"w.yml": "on: push\n"}, True)
policy_blocks("finding no files at all is a finding", {}, True)

# The live-data half: an entry on a queue ref is NAMED, and does not
# block -- a branch cannot delete it.
QUEUE_REF = "refs/heads/gh-readonly-queue/main/pr-1-0000000000000000000000000000000000000000"
ambient_only(
    "a merge-queue entry is reported by name, not blocked on",
    MEASURED_0927 + [entry("v0-rust-platform-Linux-x64-6ff13d87-84ed4606", 1.689, QUEUE_REF)],
)
_b, _amb = guard.verdict(MEASURED_0927 + [entry("v0-rust-platform-Linux-x64-6ff13d87-84ed4606", 1.689, QUEUE_REF)])
if not any("merge-queue refs hold" in a and QUEUE_REF in a for a in _amb):
    failures.append(f"a merge-queue entry must be named in an ambient notice; got {_amb!r}")
_b, _amb = guard.verdict(MEASURED_0927 + [entry("v0-rust-lint-Darwin-arm64-1-2", 0.4, "refs/pull/1/merge")])
if any("merge-queue" in a for a in _amb):
    failures.append("a pull-request ref must not be reported as a merge-queue ref")

# NEAR THE QUOTA, the other half of #1556's ask. 2026-09-27 exactly: the
# 7.76GB steady state plus the 1.69GB queue entry is 9.45GB -- under the
# quota and under no other rule's notice before this, yet one bump away
# from evicting. It must be reported; the steady state alone must not.
_near = MEASURED_0927 + [entry("v0-rust-platform-Linux-x64-6ff13d87-84ed4606", 1.689, "refs/pull/1/merge")]
_b, _amb = guard.verdict(_near)
if not any("% of GitHub's" in a for a in _amb):
    failures.append(f"9.45 of 10GB must be reported as near the quota; got {_amb!r}")
_b, _amb = guard.verdict(MEASURED_0927)
if any("% of GitHub's" in a for a in _amb):
    failures.append(f"the 7.76GB steady state must not be reported as near the quota; got {_amb!r}")

# And the report states usage against the quota on EVERY run, including a
# clean one, which is where nobody would otherwise see the number.
exit_code(MEASURED_0927, advisory=False)
if "of GitHub's 10.0GB quota (" not in exit_code.last:
    failures.append(f"a clean report must still state usage against the quota; got {exit_code.last!r}")


if failures:
    print("check-cache-budget.py self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print("check-cache-budget.py self-test: the budget guard rejects what it should.")
