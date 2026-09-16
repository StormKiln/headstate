---
name: release
description: Use when cutting or verifying a Headstate release - tag-driven, with the check-run precondition that a burned tag cannot be recovered from
---

# Release

Releases are tag-driven. `release.yml` stamps versions from `GITHUB_REF_NAME`,
so the in-repo `0.1.0` is correct and needs **no bump commit**.

## Before tagging: the precondition that matters

The gate sets `wait-for-duplicates: true` (`release.yml`), so it waits on
**every check-run attempt** on the commit — including superseded ones. A re-run
adds a passing attempt but never removes a failed one, so **one failed attempt
burns that commit for releases permanently.**

The trap: the default check-runs call returns only the latest attempt per job.

```bash
# WRONG -- latest attempt only; showed all-success on a commit the gate refused
gh api repos/OWNER/REPO/commits/SHA/check-runs

# RIGHT -- what the gate actually sees
gh api "repos/OWNER/REPO/commits/SHA/check-runs?per_page=100&filter=all" \
  -q '[.check_runs[]|.conclusion]|group_by(.)|map({(.[0]//"null"):length})|add'
```

Require `{"success": N}` with no other key. `gh pr checks` has the same blind
spot and is not sufficient.

This is not hypothetical: v5.20.0 was burned exactly this way. `f497c75` still
shows `success / failure / success` for `platform (windows-latest)`.

## Tagging

```bash
git tag -a vX.Y.Z <verified-sha> -m "Headstate vX.Y.Z

<what changed, and why it mattered>"
git push origin vX.Y.Z
```

**A tag push spawns a SECOND CI run** on a commit already verified on `main`,
so every release gets two independent chances to flake. Check `filter=all`
again after tagging.

## A burned tag is not recoverable

Do not re-run the release, delete the tag, or force anything past the gate.
Fix the cause, land it, and cut the **next patch version**. Leave the stranded
tag; the issue documents why.

## Verify the artifacts, never the green check

Reading a green check as success is this pipeline's characteristic bug class —
`release.yml` says so in its own comments. Check the artifacts:

```bash
# 1. PER-JOB conclusions. A `skipped` job is not a failure but is not a success.
gh run view "$RID" --json conclusion,jobs -q '"RUN: \(.conclusion)", (.jobs[]|"\(.conclusion)\t\(.name)")'

# 2. Version stamped in every bundle name AND inside the app
gh release view vX.Y.Z --json assets -q '.assets[].name'

# 3. latest.json: 4 signed platforms, and duplicate keys checked properly
python3 - latest.json <<'PY'
import json,sys,collections
raw=open(sys.argv[1]).read(); d=json.loads(raw)
pairs=[]
json.loads(raw, object_pairs_hook=lambda p: pairs.append([k for k,_ in p]) or dict(p))
print("version:", d["version"], "platforms:", len(d["platforms"]))
print("unsigned:", [k for k,v in d["platforms"].items() if not v.get("signature")] or "NONE")
print("dupes:", [x for ks in pairs for x in [[k for k,c in collections.Counter(ks).items() if c>1]] if x] or "NONE")
PY

# 4. Notarization on the .app INSIDE the DMG, not the DMG
MNT=$(hdiutil attach Headstate_X.Y.Z_universal.dmg -nobrowse -readonly | tail -1 | awk -F'\t' '{print $NF}')
spctl -a -vvv -t install "$MNT/Headstate.app"     # want: accepted, Notarized Developer ID
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$MNT/Headstate.app/Contents/Info.plist"
hdiutil detach "$MNT" -quiet
```

Two of those are weaker if done the obvious way. `json.loads` **silently keeps
the last of a duplicate pair**, so parsing and counting platforms passes over a
collision — `object_pairs_hook` is the real check. And Tauri notarizes the
**app bundle, not the disk image**, so validating only the DMG is the weaker
claim.

`allowed-conclusions: success` means `skipped` is not allowed either; that cost
a release at v5.4.0.

## Mobile

See `docs/mobile-release-process.md` — do not restate it here. The build
high-water mark is guarded by `scripts/check-mobile-build-mark.py`.
