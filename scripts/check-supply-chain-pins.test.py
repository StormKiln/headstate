#!/usr/bin/env python3
"""Proof that the supply-chain pin guard can fail, and on what.

Each rule is driven on synthetic input in both directions -- the shape
that must pass and the one-character variant that must not -- because a
guard whose failure mode is a silent pass needs something watching it
fail. The real files are checked by the guard itself; this pins the
judgement.

Run: python3 scripts/check-supply-chain-pins.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("guard", HERE / "check-supply-chain-pins.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []


def check(name: str, ok: bool):
    if not ok:
        failures.append(name)


SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"

# ---- uses: ---------------------------------------------------------------
check("a SHA with a version comment passes",
      guard.unpinned_uses(f"      - uses: actions/checkout@{SHA} # v7.0.1\n") == [])
check("a local action is exempt",
      guard.unpinned_uses("      - uses: ./.github/actions/setup\n") == [])
check("a floating tag is rejected",
      guard.unpinned_uses("      - uses: actions/checkout@v7\n") != [])
check("a SHA without the version comment is rejected",
      guard.unpinned_uses(f"      - uses: actions/checkout@{SHA}\n") != [])
check("a short SHA is rejected",
      guard.unpinned_uses(f"      - uses: actions/checkout@{SHA[:12]} # v7\n") != [])
check("`uses:` inside a step map (no dash) is still read",
      guard.unpinned_uses("        uses: lewagon/wait-on-check-action@v1.9.1\n") != [])

# ---- cargo install -------------------------------------------------------
check("--locked --version passes",
      guard.unpinned_cargo_installs("        run: cargo install cargo-deny --locked --version 0.20.2\n") == [])
check("--locked alone is rejected",
      guard.unpinned_cargo_installs("        run: cargo install cargo-deny --locked\n") != [])
check("--version alone is rejected",
      guard.unpinned_cargo_installs("        run: cargo install cargo-deny --version 0.20.2\n") != [])
check("a commented-out install is ignored",
      guard.unpinned_cargo_installs("      # cargo install cargo-deny\n") == [])
check("prose mentioning cargo install in a comment is ignored",
      guard.unpinned_cargo_installs("      # We used to `cargo install foo` here.\n") == [])

# ---- packageManager ------------------------------------------------------
good = '{"packageManager": "yarn@4.18.0+sha224.' + "a" * 56 + '"}'
check("yarn with a sha224 passes", guard.package_manager_problem(good) is None)
check("a bare version is rejected",
      guard.package_manager_problem('{"packageManager": "yarn@4.5.1"}') is not None)
check("a truncated digest is rejected",
      guard.package_manager_problem('{"packageManager": "yarn@4.18.0+sha224.' + "a" * 55 + '"}') is not None)
check("a missing field is rejected", guard.package_manager_problem('{}') is not None)
check("sha512 with the right length passes",
      guard.package_manager_problem('{"packageManager": "yarn@4.18.0+sha512.' + "0" * 128 + '"}') is None)

# ---- npmMinimalAgeGate ---------------------------------------------------
check("7d is seven days", guard.gate_days("npmMinimalAgeGate: 7d\n") == 7)
check("1w is seven days", guard.gate_days("npmMinimalAgeGate: 1w\n") == 7)
check("a bare number is minutes", guard.gate_days("npmMinimalAgeGate: 1440\n") == 1)
check("quoted values are read", guard.gate_days('npmMinimalAgeGate: "3d"\n') == 3)
check("unset is None", guard.gate_days("nodeLinker: node-modules\n") is None)
check("zero is zero", guard.gate_days("npmMinimalAgeGate: 0\n") == 0)

# ---- dependabot cooldown -------------------------------------------------
dependabot = """\
updates:
  - package-ecosystem: github-actions
    directory: "/"
  - package-ecosystem: npm
    directory: "/"
    cooldown:
      default-days: 7
  - package-ecosystem: cargo
    cooldown:
      default-days: 3
"""
check("the npm cooldown is read", guard.dependabot_cooldown_days(dependabot, "npm") == 7)
check("the cargo cooldown is its own", guard.dependabot_cooldown_days(dependabot, "cargo") == 3)
check("an ecosystem without one is None",
      guard.dependabot_cooldown_days(dependabot, "github-actions") is None)

# ---- requirements.txt ----------------------------------------------------
hashed = "Pillow==12.3.0 \\\n    --hash=sha256:" + "a" * 64 + " \\\n    --hash=sha256:" + "b" * 64 + "\n"
check("a ==pin with hashes passes", guard.unhashed_requirements(hashed) == [])
check("a range without hashes is rejected", guard.unhashed_requirements("Pillow>=10\n") == ["Pillow>=10"])
check("a ==pin without hashes is rejected", guard.unhashed_requirements("Pillow==12.3.0\n") == ["Pillow==12.3.0"])
check("comments and blank lines are skipped",
      guard.unhashed_requirements("# a comment\n\n" + hashed) == [])
check("two requirements are judged separately",
      guard.unhashed_requirements(hashed + "requests>=2\n") == ["requests>=2"])

if failures:
    print("supply-chain pin guard tests FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print("supply-chain pin guard tests: all pass")
