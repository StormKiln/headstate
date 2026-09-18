.PHONY: dev build test test-rust test-ui lint lint-rust lint-ui lint-deps fmt icons \
	mobile-frontend lint-mobile test-mobile check-mobile-ios check-mobile-android \
	deny-mobile deny-stepup ios-init android-init icons-mobile ios-device android-device \
	deny test-race check-intel

# ---- Mobile companion (src-mobile) ---------------------------------------
#
# A separate crate with its own lockfile; see src-mobile/Cargo.toml for
# why. None of these targets is part of `lint` or `test` above: the
# desktop gates stay exactly what CI runs, and the mobile ones need the
# iOS or Android toolchain (#518 gives them their own CI job).
#
# `TAURI_APP_PATH` is load-bearing on every `yarn tauri` call. Yarn runs
# package scripts from the workspace root, and from there the Tauri CLI
# finds `src-tauri` first -- so without it `yarn tauri ios init` sets up
# the DESKTOP crate for iOS. This was observed, not inferred.
#
# The shared frontend, built for the phone. `tauri ios build` runs this
# itself through `beforeBuildCommand`; it is here for anyone driving
# xcodebuild directly. `cargo check` and `cargo test` do NOT need it:
# the crate was verified to compile with `dist/` absent.
mobile-frontend:
	VITE_TARGET=mobile yarn build

# `--workspace`: src-mobile is a small workspace whose members are the
# in-repo plugins under src-mobile/plugins; without the flag only the
# app crate is linted and tested.
lint-mobile:
	cd src-mobile && cargo fmt --check
	cd src-mobile && cargo clippy --workspace --all-targets -- -D warnings
	# No Kotlin is compiled anywhere -- not by this target, not in CI, which
	# generates the Android Studio project and never runs Gradle. Tauri
	# dispatches on the LITERAL @Command method name, so a name that does not
	# match what Rust invokes fails on a device and nowhere else (#698).
	# Not prefixed with `cd src-mobile`: each recipe line is its own shell,
	# so this one starts at the repo root like the rest.
	python3 scripts/check-plugin-commands.py

test-mobile:
	cd src-mobile && cargo test --workspace

# Proves the phone-only dependencies (reqwest on rustls/aws-lc-rs, rcgen)
# cross-compile: aws-lc-sys builds C and assembly for the target, which
# a host `cargo check` never exercises.
check-mobile-ios:
	rustup target add aarch64-apple-ios
	cd src-mobile && cargo check --target aarch64-apple-ios

# Needs an Android NDK: aws-lc-sys looks for `aarch64-linux-android-clang`
# and fails without one (observed). Run through `yarn tauri android`
# tooling or with NDK_HOME set.
check-mobile-android:
	rustup target add aarch64-linux-android
	cd src-mobile && cargo check --target aarch64-linux-android

deny-mobile:
	cd src-mobile && cargo deny check

# The step-up crate was the one with a lockfile and no advisory check at
# all (#1008) -- `fmt`, `clippy` and `test` ran against it, `cargo deny`
# never did. `scripts/check-lockfile-agreement.py` fails if a crate with a
# lockfile is missing from this set, so the next one cannot be added
# silently.
deny-stepup:
	cd crates/headstate-stepup && cargo deny check

# Install and run on a REAL device, for the pairing walkthrough
# (docs/mobile-pairing-walkthrough.md). The walkthrough refuses
# simulators and emulators, correctly: they have no Secure Enclave, no
# Keystore-backed biometric gate, and iOS does not show the
# local-network prompt in the simulator -- so three of the things the
# run exists to check cannot be checked there.
#
# `--open` hands off to Xcode rather than building headless. Signing a
# development build needs a team, and the committed project carries none
# (`DEVELOPMENT_TEAM` is absent by design -- it is personal to whoever
# builds, and the release workflow injects its own). Xcode's Signing &
# Capabilities tab is where a person selects theirs, once, and the
# setting stays in their local checkout.
#
# `--host` goes with `--open`, per Tauri's own help: a device cannot
# reach `localhost`, so the dev server has to be served on the public
# network address. Vite already listens on 0.0.0.0 for this to work.
#
# Not a release path. Store builds come from `mobile-release.yml` on a
# `mobile-v*` tag; see docs/mobile-release-process.md.
ios-device:
	TAURI_APP_PATH=src-mobile yarn tauri ios dev --open --host

# The Android equivalent. `tauri android dev` installs over adb, so a
# device with USB debugging on and `adb devices` listing it is all that
# is needed -- no signing team, and no Play Console.
android-device:
	TAURI_APP_PATH=src-mobile yarn tauri android dev

# Regenerates gen/apple. The generated project is committed; re-run only
# when Tauri's template changes, and review the diff.
ios-init:
	TAURI_APP_PATH=src-mobile yarn tauri ios init --ci

# Same for gen/android. Needs an Android SDK with ANDROID_HOME and
# NDK_HOME set; the `mobile-android` CI job is the machine that has one.
android-init:
	TAURI_APP_PATH=src-mobile yarn tauri android init --ci

# The companion's icons, from the same master as the desktop. `yarn tauri
# icon` emits every platform's variant; the phone keeps the iOS and
# Android sets plus the 1024px source, and the desktop's icons are not
# touched.
icons-mobile:
	yarn tauri icon src-tauri/icons/icon-master.png -o src-mobile/icons
	cd src-mobile/icons && rm -f 128x128.png 128x128@2x.png 32x32.png 64x64.png \
		icon.icns icon.ico Square*.png StoreLogo.png

dev:
	yarn tauri dev

build:
	yarn tauri build

test: test-rust test-ui

# The shared step-up crate is a PATH DEPENDENCY of src-tauri, not a
# workspace member, so `cd src-tauri && cargo test` compiles it and runs
# none of its tests. It has to be named, or the one place canonical
# bytes, the header grammar and `verify` now live would be the one place
# nothing tests (#695). It is listed first because everything downstream
# of it is meaningless if the protocol itself is broken.
test-rust:
	cd crates/headstate-stepup && cargo test
	cd src-tauri && cargo test

test-ui:
	yarn vitest run

# ---- Parity with CI (#853) -----------------------------------------------
#
# Three things CI ran that no `make` target could, so a developer could
# not reproduce a CI failure locally even knowing which check had failed.
# Deliberately NOT added to `lint` or `test`: each is slow or installs a
# toolchain, and the cheap-guards rule the `lint-deps` comment states
# ("answer a question in a second") is what keeps those targets worth
# running. These are the ones you run when CI is red, or before a release.
#
# `deny-mobile` already existed for the phone crate; the desktop's
# `src-tauri/deny.toml` had no target at all, which is why a supply-chain
# failure was only ever reachable by pushing.
deny:
	cd src-tauri && cargo deny check

# The race check, matching CI's `Race check` step exactly (ci.yml):
# three runs of the library tests at eight threads.
#
# THE COSTLY OMISSION of the three. CI repeats the suite three times
# precisely because one green run does not prove a race is absent, and
# before this there was no local way to ask -- so a state-touching change
# looked fine locally and failed from a job that had run the same tests
# twice more. #834's flake was found exactly this way.
#
# `--lib` and the iteration count are CI's, not a guess: matching them is
# the point, since a local run that differed would not reproduce what CI
# saw.
test-race:
	cd src-tauri && for i in 1 2 3; do \
		cargo test --lib -- --test-threads=8 || { echo "FAILED ON ITERATION $$i"; exit 1; }; \
	done

# Mutation testing, ONE MODULE AT A TIME and never in CI (#893).
#
# `--in-place` is load-bearing, not a preference. cargo-mutants normally
# copies the crate directory to a temp dir, and two tests read files
# OUTSIDE it -- `github/model.rs:553` pulls in `src/lib/derive.ts` and
# `github/query.rs:739` pulls in `README.md`, both via `include_str!` with
# `../../../`. In a copied tree those reads fail and the baseline never
# builds:
#
#   error: couldn't read `src/github/../../../src/lib/derive.ts`
#   FAILED   Unmutated baseline
#
# Which is worth pausing on: the cross-language mirror tests that FIXED
# #850 are what broke the tool best suited to finding more #850s. There is
# no config option to copy extra paths, so `--in-place` -- testing in the
# source tree -- is the fix. It restores the tree afterwards; only
# `mutants.out/` is left behind, and that is gitignored.
#
# NOT IN CI, deliberately. 3272 mutants crate-wide, each run rebuilding
# the dependency tree; a single file's shard did not finish in 3 minutes.
# Against a CI budget whose critical path is ~9 minutes this is not a
# gate, it is an audit you run on purpose.
#
# A MISSED mutant is a test that cannot fail -- which is exactly what this
# repo shipped in #850 (constants asserted against one side) and #868 (six
# tests skipping a documented lock). Measured on the first real run:
# `src/tray.rs` alone has three.
#
# Usage: make mutants FILE=src/health/runaway.rs
FILE ?= src/tray.rs
mutants:
	cd src-tauri && cargo mutants --in-place --file $(FILE)

# The Intel target the release also builds.
#
# The release ships a UNIVERSAL binary while every test job builds native
# arm64, so an arch-gated link failure (plausible with bundled SQLite or
# octocrab's crypto backends) would first appear at TAG time, after
# version stamping, mid-release. `cargo check` rather than a second build,
# for the reason ci.yml gives: it catches the compile and
# link-configuration failures that differ by target without doubling the
# time.
check-intel:
	rustup target add x86_64-apple-darwin
	cd src-tauri && cargo check --target x86_64-apple-darwin --all-targets

lint: lint-rust lint-ui lint-deps

# Guards that answer a question in a second which would otherwise be
# answered by a job that takes minutes. `tauri build` refuses to bundle
# when an @tauri-apps/* package and its Rust crate disagree on
# major/minor, and that check lives inside the bundle -- so before this
# target existed, the mismatch passed lint and both test jobs and failed
# from the slowest one in CI (#555).
lint-deps:
	python3 scripts/check-tauri-versions.test.py
	python3 scripts/check-tauri-versions.py
	# The compiler and Node version the build is verified against. CI
	# resolved `stable` at run time, so a new stable with one more Clippy
	# lint turned `-D warnings` red on untouched branches and -- under
	# `strict_required_status_checks_policy` -- blocked every open PR at
	# once, reproducible nowhere locally (#1153). A source read with no
	# network, so it belongs in the target whose comment promises answers
	# in a second.
	python3 scripts/check-toolchain-pins.test.py
	python3 scripts/check-toolchain-pins.py
	python3 scripts/android-release-signing.test.py
	# Same class of guard: the mobile jobs are required checks that skip
	# their own steps when nothing mobile changed, and the way that wiring
	# breaks is a job reporting green having compiled nothing. Cheap to
	# ask here, invisible until a phone release otherwise.
	python3 scripts/check-mobile-gate.py
	# A committed symlink escaping the repo resolves only on the machine
	# that made it. One cost nine of ten CI jobs, a varying error message
	# that read as a flaky fetch, and a wrong fix (#813, #811) -- and it
	# passes every local run, because locally the target exists. The
	# cheapest place to ask is here, before anything installs.
	python3 scripts/check-symlinks.test.py
	python3 scripts/check-symlinks.py
	# Three crates, three lockfiles, and until #1008 one of them was
	# advisory-checked by nothing. The failure that named the class:
	# `supply-chain` PASSED and `mobile-android` FAILED the same rustls
	# advisory on the same commit, because an advisory fixed in one
	# lockfile was believed fixed repo-wide. This asks the cheap
	# structural question -- does every crate with a lockfile have an
	# advisory check -- rather than the expensive one cargo-deny answers
	# per crate.
	python3 scripts/check-lockfile-agreement.test.py
	python3 scripts/check-lockfile-agreement.py
	# The gate guard's own self-test, added with the guard's `GATED_JOBS`
	# derivation (#853). It was the one guard in this target with none,
	# which is an uncomfortable gap for a guard whose failure mode is a
	# silent pass.
	python3 scripts/check-mobile-gate.test.py
	# The rename half of the same problem. A required check that never
	# reports blocks every merge forever, and the ruleset cannot be
	# bypassed from a branch -- so the cheapest place to learn that a job
	# rename was a lockout is before the push, not from nine PRs that
	# will never go green (#887). A source read with no network, so it
	# belongs in the target whose comment promises answers in a second.
	python3 scripts/check-required-contexts.test.py
	python3 scripts/check-required-contexts.py
	# A `run:` step with no `shell:` runs under PowerShell on Windows,
	# where bash syntax (a heredoc, `$(...)`) is a parse error. CI-only
	# until now (ci.yml), so the author of such a step ran `make lint`
	# green and learned about it from a Windows job -- the v2.0.1-rc.1
	# rehearsal failure this script's docstring says it exists to
	# prevent. No dependencies, so there is no reason it was not here
	# (#848, #853).
	# The self-test runs first, like the two guards above. #892 recorded
	# this script as already having one; it did not, and writing it
	# immediately surfaced a live bug -- the platform test matched
	# `windows-latest` inside a COMMENT, so any job merely discussing
	# Windows had every later `run:` step reported. Eleven false positives
	# in the macos-only `lint` job, which is #853's cry-wolf failure
	# happening inside a guard.
	#
	# #899 fixed that by stripping to the left of `#`, which closed the
	# comment route but left the predicate a substring test over lines --
	# so the label in a VALUE still voted, and a macos-only job with
	# `SKIPPED_RUNNER: windows-latest` in its `env:`, or the label in a
	# step `name:`, was still reported. Measured by running both versions
	# over the same fixtures: they agree on every true positive and differ
	# on exactly those two, with #899's wrong (#900). The runner set now
	# comes from `runs-on` and the matrix it resolves through, so prose
	# cannot vote at all, and the self-test pins BOTH directions -- the
	# false positives and a real windows-latest job with a bash body,
	# because a fix that quietened the noise by checking less would be
	# worse than the bug.
	python3 scripts/check-workflow-shells.test.py
	python3 scripts/check-workflow-shells.py
	# actionlint, and it does NOT replace the script above it. That was
	# checked rather than assumed (#892 claims it supersedes it, which is
	# wrong): given a `windows-latest` job whose `run:` declares no
	# `shell:` and whose body is a heredoc plus `${VAR#prefix}`, actionlint
	# exits 0 -- it assumes bash and shellchecks the body as bash, so the
	# one thing that matters here, that the runner will NOT use bash, is
	# the thing it does not ask. The script above catches it. They overlap
	# in appearance only.
	#
	# What actionlint adds instead is the rest of the class: `uses:` that
	# cannot resolve, expression typos in `${{ }}`, unknown `runs-on`
	# labels, and shellcheck over every `run:` body -- defects that are
	# invisible until CI runs, which is what #848 and #853 cost.
	#
	# Skipped with a note when absent rather than failing: `brew install
	# actionlint` (shellcheck comes with it) is a real install, and this
	# target's comment promises answers in a second, not a toolchain. CI
	# installs it, so the gate is there; this is the local feedback loop.
	# No GitHub Action and so no new pinned SHA -- it is one binary.
	@if command -v actionlint >/dev/null 2>&1; then \
		actionlint; \
	else \
		echo "actionlint not installed; skipping (brew install actionlint). CI runs it."; \
	fi
	# The mobile build high-water mark drifts because nothing reads it
	# except a Preflight check that only catches a DECREASE -- so a mark
	# lagging by three still passes, and it went stale before six
	# consecutive releases (#787). The self-test runs first: this guard's
	# "cannot look" path exits 0 on purpose, so a bug that always took it
	# would leave the mark unguarded while printing something reassuring.
	#
	# NOT passed --require here. Unlike everything above it, this one
	# needs the network and a `gh` token, and `lint-deps` is the target
	# whose comment promises answers in a second. Locally it reports what
	# it found and skips when it cannot look; ci.yml passes --require,
	# where a token exists and an unreachable API is worth seeing.
	python3 scripts/check-mobile-build-mark.test.py
	python3 scripts/check-mobile-build-mark.py
	# The Actions cache budget (#901). Over GitHub's 10GB quota entries are
	# evicted least-recently-used, so a measured cache HIT can become a miss
	# on the same key minutes later -- which turned a 562s job into 1716s and
	# means CI timings are not reproducible. This is the check to run when a
	# timing looks wrong before concluding a change caused it.
	#
	# Needs the network, like the mark check above, and skips when it cannot
	# look. NOT in ci.yml's `lint`, unlike its siblings: the cache is a
	# shared, draining resource rather than a property of the branch under
	# test, so a gate would fail pull requests for a state their authors
	# cannot fix. See the script's docstring.
	python3 scripts/check-cache-budget.test.py
	python3 scripts/check-cache-budget.py
	python3 scripts/check-supply-chain-pins.test.py
	python3 scripts/check-supply-chain-pins.py
	# The leak guard, LAST in this target: it is the only check here that
	# scans commit messages, so it is the only one whose failure means an
	# amend or an interactive rebase rather than an edit. Running it
	# locally at all is the point of #833 -- it was CI-only, so the
	# feedback arrived after the content was already pushed to a public
	# remote, which is one step too late for a guard whose whole purpose
	# is to stop sensitive strings reaching one.
	#
	# Runnable locally only since #848 gitignored the vendored Tauri iOS
	# API; before that this line would have exited 2 for every developer
	# who had ever built for iOS.
	#
	# Local checks are advisory, CI remains the gate (ci.yml) -- this
	# closes the feedback gap, it does not move the gate.
	./scripts/check-privacy.sh

# The shared step-up crate again: a path dependency is compiled by the
# desktop's clippy but its own tests are not, and `cargo fmt --check`
# from src-tauri never looks outside that package. Named for the same
# reason it is named in test-rust.
lint-rust:
	cd crates/headstate-stepup && cargo fmt --check
	cd crates/headstate-stepup && cargo clippy --all-targets -- -D warnings
	cd src-tauri && cargo fmt --check
	cd src-tauri && cargo clippy --all-targets -- -D warnings

lint-ui:
	yarn tsc -b --force
	yarn eslint .
	yarn knip
	# The focus ring is CSS the test suite structurally cannot see: jsdom
	# applies no stylesheets, `?raw` on a .css file returns empty because
	# @tailwindcss/vite claims it, and tests avoid node:fs. Deleting the
	# rule un-fixes every button in the app with a green suite (#694).
	./scripts/check-focus-css.sh

fmt:
	cd crates/headstate-stepup && cargo fmt
	cd src-tauri && cargo fmt

# Requires Pillow: pip install -r scripts/requirements.txt
#
# `yarn tauri icon` also emits Windows/iOS/Android icon variants this
# macOS-only app never uses, and its ICNS encoder is non-deterministic --
# re-running against unchanged source art re-packs icon.icns with different
# compressed-stream bytes even though every image inside is pixel-identical.
# Restore the 1024 master over icon.png (as before), prune the unused
# variants, and restore the committed icon.icns bytes when its *content*
# (not raw bytes) matches what's already committed -- so a second run of
# this target leaves `git status` clean.
icons:
	python3 scripts/make-icons.py
	yarn tauri icon src-tauri/icons/icon.png
	cp src-tauri/icons/icon-master.png src-tauri/icons/icon.png
	rm -rf src-tauri/icons/android src-tauri/icons/ios
	# icon.ico is KEPT: tauri_build embeds it as a Windows resource, and
	# without it the build script fails before compiling any app code.
	# Deleting it was correct while this was macOS-only and is not now.
	rm -f src-tauri/icons/StoreLogo.png
	rm -f src-tauri/icons/Square*.png src-tauri/icons/64x64.png
	python3 scripts/make-icons.py --restore-icns-if-unchanged
