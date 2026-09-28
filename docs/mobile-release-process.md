# Mobile companion: release process

How a `mobile-v*` tag becomes a TestFlight build and a Play internal-testing
build, and what a person still has to do around it. Design:
[Mobile Companion Design](superpowers/specs/2026-09-05-mobile-companion-design.md),
section *Release*. Workflow: `.github/workflows/mobile-release.yml`.

The desktop's release (`CONTRIBUTING.md`, "Cutting a release") is a
different pipeline with a different tag prefix. Nothing here touches it.

## Versioning

- The companion's version is its own, tagged `mobile-vX.Y.Z`, independent of
  the desktop's `vX.Y.Z`. Compatibility between the two is the wire
  protocol's integer version (returned by `/v1/hello`), not either app
  version.
- The tag is the single source of truth. The workflow stamps it into
  `src-mobile/tauri.conf.json` and `src-mobile/Cargo.toml` for the build
  (`scripts/stamp-mobile-version.py`); nothing is committed. The `0.1.0` in
  those files and in `gen/apple` is a placeholder that Tauri overwrites at
  build time.
- The **build number** is the workflow run number, and it is the same on
  both stores: `CFBundleVersion` on iOS, `versionCode` on Android. It is
  what the pairing walkthrough's run record asks for and what the job
  prints in its summary. Both stores require it to increase; a run number
  only ever increases for a given workflow file, so do not rename the file.
- Any tag on the same commit produces a new build number, so a re-tag after
  a failed upload is fine.

## Cutting a release

1. Make sure `main` is green and the mobile checks pass
   (`make lint-mobile test-mobile check-mobile-ios`; Android needs an NDK,
   see the Makefile).
2. Tag and push:

   ```
   git tag mobile-v0.2.0
   git push origin mobile-v0.2.0
   ```

3. Wait for the **Mobile release** workflow. Both jobs run the mobile checks
   first, then build, then upload. Failures stop before anything reaches a
   store; the pre-release is created only after both uploads succeed.
   The build number is in the job log ("Version 0.2.0, build 57") and in
   the pre-release notes.
4. **Raise the build mark (one-line PR).** Set the last line of
   `.github/mobile-build-high-water-mark` to that build number and merge it.
   The run's summary prints this as a required follow-up with the exact
   value. Until it merges, CI warns on every commit, and the next mobile
   release refuses to run (see
   [The build number never goes backwards](#the-build-number-never-goes-backwards)).
5. When the build appears in TestFlight and on the Play internal track
   (a few minutes of processing each), run
   [the pairing walkthrough](mobile-pairing-walkthrough.md) on a real iPhone
   and a real Android device, recording **that build number** in the run
   record.
6. Promote manually, from the App Store Connect and Play Console web UIs,
   only after a clean run against that exact build. Automation stops at the
   testing tracks on purpose.

The GitHub pre-release holds the `.ipa`, the `.aab`, and a `SHA256SUMS`
file for provenance. Users do not install from it.

## The public beta (TestFlight external testing)

Internal testing reaches only App Store Connect users you add by hand. A
**public link** lets anyone install the beta from a URL, which is what the
desktop's *Get the mobile companion* button and its QR code point at
(`TESTFLIGHT_JOIN_URL` in `src/components/GetCompanionPanel.tsx`).

The group is **live**: "HeadState beta", public link
`https://testflight.apple.com/join/HwV4gMHr`.

Setting it up is a console task, done once — recorded here because it has
to be repeated if the group is ever recreated:

1. **App Store Connect → the app → TestFlight**.
2. Under **External Testing**, create a group (e.g. "Public Beta").
   External, not Internal: internal groups cap at 100 App Store Connect
   users and cannot have a public link.
3. Add a build to the group.
4. **Submit it for Beta App Review.** External testing requires review;
   internal does not. It is lighter than an App Store submission but is a
   real review, and it needs a beta description, a feedback email, and
   review notes. Headstate pairs with a desktop rather than a service
   account, so the notes must say what a reviewer can exercise without a
   paired desktop and what they cannot. Export compliance is already
   answered by the build (see below).
5. Once approved, enable **Public Link** in the group's settings. A tester
   cap is optional.
6. Put the resulting `https://testflight.apple.com/join/<code>` URL into
   `TESTFLIGHT_JOIN_URL`. It is a public URL by design, not a secret, so
   it belongs in the repo rather than in a build secret. Until it is set,
   the settings panel says the invitation is not available yet rather than
   showing a code that resolves to nothing.

Later builds reach the group without re-review while the version does not
change materially; Apple re-reviews periodically.

**Builds expire after 90 days.** A group whose only build has expired
still serves its public link, and the link then installs nothing -- which
looks like a broken app rather than an expired build. Adding each new
mobile release to the group is what keeps the link honest, so do it as
part of cutting the release, not only when someone reports it broken.

If the link ever stops working -- expired, capped, or the group turned
off -- blank `TESTFLIGHT_JOIN_URL` rather than leaving it pointing at a
dead page. The panel then says the invitation is unavailable, which is
true, instead of sending someone to a page that fails after they have
already installed TestFlight.

A public link is genuinely public: anyone holding the URL can install, up
to the cap, with no per-tester approval.

Android has no equivalent yet — its store secrets are unconfigured
(#673).

## Export compliance

The app declares `ITSAppUsesNonExemptEncryption` **false** in
`src-mobile/Info.ios.plist`, and carries no compliance code. Answering in the
app is what keeps a build from stalling on **Missing Compliance**, which blocks
testers from installing until someone clears it by hand.

Why `false` rather than `true` is the next section, and it is not a preference:
`true` without a code Apple has issued is rejected at upload. Three builds were
lost to that.

### Why `false`

The App Encryption Documentation questionnaire in App Store Connect was
completed for this app and concluded that **no documentation needs to be
uploaded**, provided the app is not available in France.

`false` is not a judgement call weighed against `true`; the two are a **pair
with the code**. Apple issues an `ITSEncryptionExportComplianceCode` only for
documentation it has *reviewed*, and it refuses any upload that declares `true`
without a matching code:

```
Invalid Export Compliance Code. The export compliance key value [] in the
app's Info.plist doesn't match the key value of the app's export compliance
documentation.
```

Builds 10, 11 and 12 all hit exactly that — with the code key empty, then
absent, then absent again after the questionnaire was answered. The constant
across all three was `true`. With no documentation on file there is no code, so
`true` cannot be satisfied, and `false` is the only answer that matches what
App Store Connect actually holds.

What the app does with cryptography is unchanged and was what the questionnaire
was answered against:

- TLS 1.3 with mutual authentication to the paired desktop, over rustls,
- ECDSA P-256 and ML-DSA-65 signatures authenticating commands, and
- Stronghold, plus an AES-GCM key in the Keystore or Secure Enclave, protecting
  the app's own secrets at rest.

Re-run the questionnaire if that changes — a user-facing encryption feature, or
carrying third-party data. If it then requires documentation, the answer becomes
`true` **plus** the code Apple issues, set as `APPLE_EXPORT_COMPLIANCE_CODE`.
The build and its guard already handle that case.

Separately, and satisfied by nothing in this repository: a US exporter of 5D002
software owes BIS an **annual self-classification report**, due **1 February**
for the preceding calendar year.

### France

The app is **excluded from the French App Store**, in App Store Connect under
**Monetization > Pricing and Availability**. France controls the import of
encryption software, and an app that links its own cryptography needs a
declaration with ANSSI: a postal submission, in French, processed in about a
month. Excluding France avoids that filing, and the questionnaire's "no
documentation required" answer depends on it.

**Do not re-enable France without filing the ANSSI declaration first**, and note
that doing so also changes the export compliance answers recorded above.

### The BIS report, which nothing here does for you

Separately from Apple, a US exporter of 5D002 software owes BIS an **annual
self-classification report**, due **1 February** for the preceding calendar
year, sent to BIS and the ENC Encryption Request Coordinator. It lists the
product, the encryption it uses, and its classification. It is a report, not a
licence application, and no part of this repository files it.

**This is not legal advice, and the classification is worth confirming with
someone who does export compliance**, particularly given the post-quantum
signatures.

### The guard

"Collect the IPA" reads both keys back out of the built `.ipa` and fails if:

- no `APPLE_EXPORT_COMPLIANCE_CODE` is set and the app declares anything other
  than `false` with no code key, or
- a code **is** set and the app does not declare `true` carrying exactly it.

The two halves are checked together because either one alone is a rejected
upload.

It checks the artifact rather than the source because that is what Apple reads:
a key that failed to merge, or a reference that expanded to nothing, both look
correct in the repository.

### Revisit this if

the app gains a user-facing encryption feature, starts carrying third-party
data, or adopts a proprietary algorithm. Re-run the questionnaire; if it then
requires documentation, set `APPLE_EXPORT_COMPLIANCE_CODE` to the code Apple
issues and the build switches to `true` automatically. A proprietary algorithm
would additionally require a CCATS.

## Rehearsing without a tag

The workflow has a **Run workflow** button with a `dry_run` input, default
on. A dry run builds both apps **unsigned**, uploads them as workflow
artifacts (`mobile-ios`, `mobile-android`), and skips signing, both store
uploads, and the pre-release. It needs no secrets and does not need the
release to be enabled, so it is how to prove the pipeline works before
enabling it.

GitHub only shows the button for a workflow file that is on the default
branch, so a dry run cannot be triggered from a PR that adds or changes the
file; merge first, then dispatch.

A dispatch with `dry_run` off is a real release and is refused unless it is
run from a `mobile-v*` tag with the release enabled.

## Enabling the release (the first release gate)

Build jobs are gated on the repository variable `MOBILE_RELEASE_ENABLED`.
Until it is `true`, a `mobile-v*` tag does nothing (the jobs show as
skipped). The spec keeps store builds manual until the pairing walkthrough
has passed **twice with no findings**; when it has:

1. Settings > Secrets and variables > Actions > **Variables** > New
   repository variable.
2. Name `MOBILE_RELEASE_ENABLED`, value `true`.

Set it to anything else (or delete it) to disable again. No workflow edit is
needed in either direction.

### Android has a second gate

`ANDROID_RELEASE_ENABLED` gates the Android job **on its own**, because the
two platforms are provisioned independently: Apple's certificates and
Google's Play account are separate pieces of work, and one variable meaning
"mobile releases are on" cannot express that only one of them is done.

Leave it unset and Android is **skipped**, not failed, and iOS ships alone.
Set it to `true` once all five Android secrets below exist.

This distinction is not cosmetic. Before it existed, every `mobile-v*` tag
ran an Android job whose secrets did not exist; it failed Preflight, and
because `publish` needed both platforms, `publish` was skipped with it. The
result was **38 mobile tags and zero GitHub releases** — no artifacts, no
checksums, no changelog — while iOS was reaching TestFlight perfectly well
the whole time. The pipeline was red for a reason unrelated to the thing it
was successfully doing, which is exactly how it went unnoticed (#673).

A dry run is exempt from `MOBILE_RELEASE_ENABLED` but **not** from this one:
rehearsing a build whose signing secrets do not exist only reproduces the
failure the gate exists to prevent.

## The build number never goes backwards

`BUILD_NUMBER` is `github.run_number`, which becomes `CFBundleVersion` on
iOS and `versionCode` on Android. Both stores refuse a build number they
have already seen.

`run_number` is counted per workflow **file**, so renaming
`mobile-release.yml` restarts it at 1 and every upload after that is
rejected as a duplicate — recoverable only by burning version numbers until
the count climbs back past the highest already shipped.

Preflight compares against `.github/mobile-build-high-water-mark` and fails
in the first job with the cause named. **Raise that file whenever a build
reaches TestFlight or Play.**

You no longer have to remember to. The release run's summary names the
value as a required follow-up (step 4 above), and
`scripts/check-mobile-build-mark.py` compares the file with the `build<N>`
of the newest published mobile asset in two places:

- **Per commit** (CI's `lint` job and `make lint`): a lagging mark is a
  warning annotation naming the value to write. It does not fail.
- **At the next mobile release** (Preflight, with `--release`): a lagging
  mark fails the run before anything is built. Raise the mark on `main`,
  then tag a commit that has it.

That guard exists because the duplicate check could never catch this. It
tests `BUILD_NUMBER > HIGH`, and `run_number` climbs every run, so a mark
lagging by three still passes — run 29 cleared a mark of 25 as easily as 28.
Staleness was invisible to the file's only reader, which is how it drifted
before six consecutive releases without one of them failing (#787). The cost
was never a blocked release; it was this file naming the wrong number to
whoever is debugging a genuine duplicate rejection.

Why it warns per commit instead of failing (#1418): it used to fail `lint`,
and every open PR and the merge queue went red after each mobile release
until the one-line mark PR merged, for a reason unrelated to any of them.
The failure could not move to the `push` run on `main` or to a scheduled
job either: the desktop release gate reads every check-run attempt on a
commit, so one red attempt on `main` burns that commit for releases. A
mobile release attaches to no `main` commit, so that is where it is
enforced. A missing or unparseable mark file still fails everywhere.

Numbers being non-contiguous per version (0.1.7 → 9, 0.1.12 → 14) is this
system working, not drift: a rejected upload still consumes its number, and
re-tagging the same version is routine. Only a decrease is a fault (#635).

A mark *ahead* of the newest published asset is also fine, and the guard
says so rather than failing: the file is written before the upload, so it
records a number consumed by a run whose `publish` never finished. That is
why the guard compares against the assets instead of being replaced by
them — an asset exists only if `publish` succeeded, so assets can
under-report what a store has seen, and this file cannot.

## Secrets

Settings > Secrets and variables > Actions > **Secrets**. This workflow is
the only one that reads them; the CI workflow has no access to any of them.
The workflow checks that every secret it needs is present before building
and names the missing ones.

### iOS

| Secret | What it is |
|---|---|
| `IOS_DIST_CERT_P12_BASE64` | Base64 of a `.p12` export of an **Apple Distribution** certificate with its private key |
| `IOS_DIST_CERT_PASSWORD` | The password the `.p12` was exported with |
| `IOS_PROVISIONING_PROFILE_BASE64` | Base64 of an **App Store** provisioning profile for `com.pktstorm.headstate.companion` |
| `APPSTORE_API_KEY_ID` | The App Store Connect API key's Key ID |
| `APPSTORE_API_ISSUER_ID` | The Issuer ID shown above the keys table |
| `APPSTORE_API_PRIVATE_KEY` | The contents of the key's `.p8` file, including the BEGIN/END lines |
| `APPLE_EXPORT_COMPLIANCE_CODE` | **Not set, and not needed.** Apple issues a code only for *uploaded* documentation, which this app's questionnaire did not require; see [Export compliance](#export-compliance) |
| `APPLE_TEAM_ID` | The ten-character Apple team ID, used for signing and shared with the desktop release |

How to generate them:

- **Certificate.** In Keychain Access, Certificate Assistant > Request a
  Certificate From a Certificate Authority, saved to disk. At
  developer.apple.com > Certificates, create an *Apple Distribution*
  certificate from that request and download it; double-click to install.
  Back in Keychain Access, select the certificate **and** its private key,
  File > Export Items, format `.p12`, set a password. Then
  `base64 -i dist.p12 | pbcopy`.
  Not a *Developer ID Application* certificate (that is the desktop's, and
  cannot sign for the App Store) and not *Apple Development* (device
  testing only).
- **Provisioning profile.** developer.apple.com > Profiles > new, type
  *App Store Connect* (under Distribution), for the companion's App ID and
  the certificate above. Download, then
  `base64 -i profile.mobileprovision | pbcopy`. Regenerate it whenever the
  certificate is renewed: a profile is bound to the certificate it was made
  with.
- **API key.** App Store Connect > Users and Access > Integrations > App
  Store Connect API > Team Keys > generate, role *App Manager* (the least
  role that can upload builds). The Key ID and Issuer ID are on that page;
  the `.p8` downloads once and cannot be re-downloaded, so put it in the
  secret immediately: `pbcopy < AuthKey_XXXXXXXXXX.p8`.

`APPLE_TEAM_ID` is shared with the desktop release, which notarizes with it.
It is not really sensitive, since it ships inside every signed app, but it is an
account identifier and this repository is public. The build needs it explicitly:
Tauri does not infer the team from the certificate, and without it the Xcode
project has no signing team and the build fails with a message about the
Signing & Capabilities editor.

### Android

| Secret | What it is |
|---|---|
| `ANDROID_UPLOAD_KEYSTORE_BASE64` | Base64 of the **upload** keystore (`.jks`) |
| `ANDROID_KEYSTORE_PASSWORD` | The keystore's password |
| `ANDROID_KEY_ALIAS` | The alias of the upload key inside it (`upload` below) |
| `ANDROID_KEY_PASSWORD` | The key's password (a PKCS12 keystore, the default, requires this to equal the keystore password) |
| `PLAY_SERVICE_ACCOUNT_JSON` | The contents of a Google Cloud service account's JSON key |

How to generate them:

- **Upload keystore.** Per Tauri's Android signing guide:

  ```
  keytool -genkey -v -keystore upload-keystore.jks -keyalg RSA -keysize 2048 \
    -validity 10000 -alias upload
  ```

  Then `base64 -i upload-keystore.jks | pbcopy`. Keep the file somewhere
  safe too: losing it means requesting an upload-key reset from Google.
- **Play App Signing.** In the Play Console the app must be enrolled in
  Play App Signing (the default for new apps), so Google holds the *app
  signing key* and this keystore is only the *upload key*. That is what
  lets the upload key be rotated without a new listing, as the spec
  requires. Register the upload certificate there
  (`keytool -export -rfc -keystore upload-keystore.jks -alias upload -file upload.pem`).
- **Service account.** Google Cloud Console > IAM > Service Accounts > create
  (no roles needed in Cloud), then Keys > Add key > JSON; that file is the
  secret (`pbcopy < service-account.json`). In the Play Console, Users and
  permissions > invite the service account's email, grant it the app with
  *Release to testing tracks*. It needs nothing broader: promotion is manual.

The signing block Gradle needs is added to the generated Android project by
`scripts/android-release-signing.py`; the workflow runs it on every build,
and it is a no-op once the block is present (it will be committed with
`gen/android`). The keystore is written to `gen/android/keystore.properties`
for the build and deleted afterwards; that path is in `.gitignore` twice.

## What the workflow does, per platform

**iOS** (`macos-latest`, Xcode 26): run the mobile checks; create a
throwaway keychain and import the certificate; install the provisioning
profile, failing early if it is not for this app; export the identity, team,
profile name and compliance code as `HEADSTATE_*` build settings, which the
committed Xcode project references; `tauri ios build --ci --export-method
app-store-connect`; `xcrun altool --upload-package` with the API key; delete the
keychain, profile and key (also on failure).

The signing settings go through the project rather than the command line
because nothing else survives: the CLI's trailing `--` arguments go to Cargo,
and `XCODE_XCCONFIG_FILE` is not honoured through it. The project declares each
setting as a `$(HEADSTATE_*)` reference, and Xcode expands build settings from
the environment, so the workflow supplies the values without committing any of
them. Unset, as in a local build, they expand to empty and Xcode's own defaults
apply. `CODE_SIGN_STYLE` is the exception and is written literally, because the
CLI copies it into `ExportOptions.plist` as a raw string and an unexpanded
reference is rejected at export.

**Android** (`ubuntu-latest`, pinned NDK, the same toolchain steps as CI's
`mobile-android` job): run the mobile checks; generate `gen/android` if it
is not committed yet; write `keystore.properties`; `tauri android build
--ci --aab --target aarch64`; verify the bundle is signed; upload to the
`internal` track as a completed release through the Play Developer API
(`scripts/play-upload.py`); delete the keystore and credentials.

**Publish**: both artifacts and one `SHA256SUMS`, verified before upload,
attached to a GitHub pre-release for the tag with notes generated against
the previous `mobile-v*` tag.

## Rotating material

- Certificate expired or revoked: new certificate **and** new profile;
  update both secrets.
- Upload key compromised: Play Console > Setup > App signing > request
  upload key reset; new keystore; update the three Android secrets.
- API key or service account leaked: revoke it where it was created and
  replace the secret. Neither can promote a build, only upload one.
