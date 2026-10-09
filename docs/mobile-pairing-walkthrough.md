# Mobile companion: pairing walkthrough

For remote access over cellular or another network, follow [the Tailscale setup guide](tailscale-remote-access.md). Keep the same pairing when adding a remote address; do not unpair to repair network reachability.


The manual test that gates every mobile release. It exercises the parts of
the companion that no automated test can reach: a real phone, its secure
hardware, its biometric prompt, the platform's local-network permission,
and a person comparing two fingerprints on two screens.

Design: [Mobile Companion Design](superpowers/specs/2026-09-05-mobile-companion-design.md)
(see *Pairing*, *Step-up for destructive commands*, *Reachability*,
*Testing*, and *Release*).

Desktop labels below are the ones Settings ships on its **Phone** topic
(`PairPhonePanel`, `PairingRequestModal`, `PairedDevicesList`), and the
banner lines are `src/components/ConnectionBanner.tsx`'s.

Phone-side labels are `PairingScreen` and the Phone topic's mobile half
(`PairedDesktopPanel`). Both landed in the *Mobile Release 1* epic
(#590); before it, every phone-side step here described a screen that
did not exist, so this walkthrough could not be run past step 1.2 -- and
since two clean runs are what enable store builds, the release gate was
blocked on it. If a step below names a control you cannot find, that is
a finding: record it rather than working around it.

## The release rule

- Run this walkthrough once on a **real iPhone** and once on a **real
  Android device** before every mobile release. Simulators and emulators
  do not count: they have no Secure Enclave, no Keystore-backed biometric
  gate, and iOS does not show the local-network prompt in the simulator.
- Store builds (TestFlight and the Play internal-testing track) are
  produced manually until this walkthrough has passed **twice with no
  findings**. After two clean runs the `mobile-release` job is enabled and
  every `mobile-v*` tag produces both builds.
- Promotion from TestFlight and internal testing to the public stores is a
  manual step taken only after a clean run against **that exact build
  number**.

Keep every completed run. Copy this file, fill in the header, tick the
boxes, and commit the copy under [`docs/walkthroughs/`](walkthroughs/)
named `YYYY-MM-DD-<platform>.md`. Runs are evidence, not paperwork: a run
with a finding is more useful than one without.

Getting a build onto a device is the part with no CI job behind it:
`make ios-device` (Xcode, where you select your signing team) and
`make android-device` (adb). [`docs/walkthroughs/README.md`](walkthroughs/README.md)
covers both, and what else is worth recording while a device is in hand.

## Run record

Fill in before starting. Every field is required.

| Field | Value |
|---|---|
| Date | |
| Desktop Headstate version (About box) | |
| Desktop OS and version | |
| Mobile build number (TestFlight / Play build, or local build hash) | |
| Device model | |
| Device OS version | |
| Tester | |
| Network (same wifi / overlay / both) | |
| Result (clean / findings) | |

Findings go in the last section. A run is *clean* only if every box that
applies to the platform is ticked and the findings section is empty.

## Hardware capability, per device

The spec's one remaining open question is which Secure Enclave and
KeyMint generations actually provide ML-DSA-65 (#670). Apple documents
`SecureEnclave.MLDSA65` by OS version only and names no hardware floor,
so the answer can only come from devices. Add a row whenever a run
happens on a device not already listed -- one line, and the question
closes itself over time.

| Device | SoC / KeyMint | OS | Post-quantum key |
|---|---|---|---|
| iPhone 16 Pro | A18 Pro | iOS 26.6.1 | yes |

The value is what step 1.3 reported and what **Paired devices** shows: a
*post-quantum* badge means `SecureEnclave.MLDSA65.PrivateKey` generated,
and its absence means it threw and the pairing fell back to ECDSA P-256
alone. Both are valid outcomes; only the recording matters.

**The row worth chasing is an A13-A15 iPhone on iOS 26.** iOS 26 needs
A13 or later, so if one of those generates a key, the OS floor and the
silicon floor coincide and nothing further is needed. A18 Pro working
tells us the newest hardware works, which was never in doubt.

## Before you start

Use synthetic data throughout. Names and addresses in this document are
placeholders: `octocat's laptop`, `Octocat's phone`, `192.0.2.10` (RFC 5737
documentation range), `100.64.0.7` (CGNAT range, standing in for an overlay
address). Never paste a real repository, hostname, or address into a
committed run record.

- [ ] Desktop Headstate is installed, signed in, and showing at least one
      pull request. A throwaway checkout of `octocat/hello-world` is a fine
      source.
- [ ] Desktop Settings, **Phone**, has **Allow phone connections** *off*
      and **Paired devices** reading *No phones paired yet.* If a device
      from an earlier run is listed, revoke it first so the run starts
      from nothing.
- [ ] If the desktop was upgraded from 5.0 (protocol 1) rather than
      installed fresh: this version moves both TLS certificates to
      ML-DSA-65 (protocol 2, #521). The upgrade clears **Paired devices**
      on its own and the desktop makes a new identity -- a new
      fingerprint -- the first time phone connections are enabled, so
      **every phone must pair again**; a 5.0 companion sees the new
      desktop as unreachable, and a 5.0 desktop shows this companion the
      *Update Headstate on your desktop* banner. Expect the desktop log
      to say the pre-5.1 identity was replaced, once.
- [ ] A throwaway git worktree exists on the desktop for the destructive
      step, for example a worktree of `octocat/hello-world` on a branch named
      `walkthrough-scratch`. It must contain nothing you want to keep.
- [ ] Note the desktop's poll interval in Settings; the write step changes
      it and the walkthrough restores it.
- [ ] The phone has a biometric enrolled (Face ID, Touch ID, or Android
      fingerprint / face) and a device passcode set.
- [ ] The phone is on the same wifi as the desktop. If you also test over an
      overlay network, do the same-wifi run first.
- [ ] The companion app is installed fresh, or has been unpaired
      (*Settings*, *Forget desktop*) so it opens on the pairing screen.
- [ ] Desktop and phone clocks agree to within a few seconds. Destructive
      requests are rejected beyond sixty seconds of skew, and a clock
      finding is not a signature finding.

## 1. Pair

- [ ] 1.1 On the desktop open Settings, **Phone**, and press **Pair a
      phone**. The listener turns on if it was off (the **Allow phone
      connections** box ticks itself) and a QR code appears with the
      desktop's fingerprint beside it under *Fingerprint (SHA256)* and an
      *Expires in* countdown from two minutes.
- [ ] 1.2 On the phone tap **Scan** and scan the QR. The phone reports the
      desktop name from the payload (`octocat's laptop`) and shows a
      fingerprint.
- [ ] 1.3 The desktop shows a dialog titled **Pair Octocat's phone?** with
      the same fingerprint and one of two lines: *Offered a post-quantum
      signing key (ML-DSA-65) as well as ECDSA.* or *No post-quantum
      signing key offered; this phone will sign with ECDSA only.* Record
      which in the findings section, whatever it says; a phone without
      ML-DSA is expected on Android 16 and older, and on any device whose
      keystore lacks it. The dialog counts down *Denied automatically in*
      from two minutes and offers **Deny** and **Approve**.
- [ ] 1.4 Compare the fingerprint on both screens **in groups of four**
      hex characters, start to end, reading each group aloud from one
      screen and checking it on the other. Do not stop after the first
      group. Both screens must render the same grouping (`ab12 cd34 ef56 …`).
- [ ] 1.5 **Approve** on the desktop. The phone moves to the PR list and
      the connection banner reads *octocat's laptop · reachable · last
      poll …* with a green dot. Tapping the banner opens Settings on the
      **Phone** topic.
- [ ] 1.6 Desktop Settings, **Phone**, **Paired devices**, lists
      `Octocat's phone` with *Paired <today's date>*, its fingerprint in
      groups of four, and a *post-quantum* badge when step 1.3 said the
      key was offered. The same line reads *Never connected* or *Last
      seen …*: the desktop does not yet write `last_seen` (follow-up from
      #543), so *Never connected* is the expected reading until that
      lands, not a finding.
- [ ] 1.7 Negative: start a second **Pair a phone**, photograph or
      screenshot the QR, and let the countdown expire (the code blanks to
      *Expired* and the panel says *This code has expired. Generate a new
      one to try again.*). Scan the stale copy. The phone shows a clear
      "pairing expired" error and stays on the pairing screen; the
      desktop shows no approval dialog and adds no row.

## 2. One read command

- [ ] 2.1 The PR list on the phone matches the desktop's list: same
      repositories, same PR numbers, same CI and mergeable states.
- [ ] 2.2 Pull to refresh on the phone (`refresh_now`). The desktop's
      last-poll time advances and the phone's banner shows the new time
      within a few seconds; the phone did not talk to GitHub itself.
- [ ] 2.3 Open one PR's detail on the phone (`get_pr_detail`). Title, checks,
      reviews, and threads match the desktop.
- [ ] 2.4 No biometric prompt appeared for any of the above.

## 3. One write command

Use a write that changes desktop state but not GitHub, so the run is safe to
repeat: the poll interval.

- [ ] 3.1 On the phone open Settings and change the poll interval
      (`set_poll_interval`) to a value different from the one noted in
      *Before you start*.
- [ ] 3.2 Desktop Settings shows the new interval without a restart.
- [ ] 3.3 No biometric prompt appeared. Writes carry no signature by
      design; a prompt here is a finding.
- [ ] 3.4 Restore the original interval from the phone and confirm the
      desktop follows.

## 4. One destructive command, with the biometric prompt

- [ ] 4.1 On the phone open Cleanup, Worktrees, and select the throwaway
      worktree from *Before you start*. Confirm it is the only one selected.
- [ ] 4.2 Tap Remove. **The platform biometric prompt appears** (Face ID,
      Touch ID, or the Android biometric sheet) before anything is
      deleted. The prompt names Headstate.
- [ ] 4.3 Negative first: **cancel** the prompt. The phone shows the
      command as cancelled, the worktree still exists on the desktop, and
      the desktop's cleanup log has no new entry.
- [ ] 4.4 Tap Remove again and pass the biometric. The worktree disappears
      from the desktop's Cleanup view and from disk.
- [ ] 4.5 The desktop posts a native notification for the removal that
      names the device (`Octocat's phone`).
- [ ] 4.6 The desktop's cleanup log (Cleanup, *Log*) records the removal
      with the same detail a local removal gets.
- [ ] 4.7 Android only: lock the phone, unlock with the passcode rather than
      a biometric, and repeat a destructive command on a second throwaway
      worktree. The keystore access control must still prompt; passcode
      fallback is acceptable, silent success is a finding.

## 5. Revoke on the desktop

- [ ] 5.1 Leave the phone app open in the foreground on the PR list.
- [ ] 5.2 Desktop Settings, **Phone**, **Paired devices**, **Revoke** next
      to `Octocat's phone`, then **Revoke** again in the *Revoke Octocat's
      phone?* dialog. The row disappears and the list reads *No phones
      paired yet.*
- [ ] 5.3 Any open event stream from the phone closes: within a few seconds
      the phone's banner leaves *reachable*.

## 6. Confirm the phone is refused

- [ ] 6.1 Pull to refresh on the phone. The request is refused at the TLS
      handshake, not with an HTTP error: the phone shows its `revoked`
      state (banner: *octocat's laptop revoked this phone · pair again*)
      and **returns to the pairing screen**, with a message saying the
      desktop no longer recognises it.
- [ ] 6.2 Background the app, wait a minute, and bring it back. It stays on
      the pairing screen; it does not retry into a loop of errors.
- [ ] 6.3 Desktop **Paired devices** is still empty and the desktop log
      shows the refused handshake without a stack trace or a crash.
- [ ] 6.4 The desktop's *Allow phone connections* toggle is still on;
      revoking a device does not stop the listener.

## 7. Re-pair, including the same-name replacement

- [ ] 7.1 Repeat step 1 in full. The phone offers the same device name
      (`Octocat's phone`). Pairing succeeds and the PR list loads.
- [ ] 7.2 Same-name replacement: without revoking, on the phone choose
      *Forget desktop* and pair again with the same name. Press
      **Approve**: the dialog changes to **Replace the existing pairing
      for Octocat's phone?**, says that replacing revokes the old one and
      keeping both leaves two devices with the same name, and offers
      **Cancel**, **Keep both**, and **Replace**.
- [ ] 7.3 Choose **Replace**. **Paired devices** shows exactly one
      `Octocat's phone` row, its fingerprint is the phone's current one,
      and the old fingerprint no longer appears.
- [ ] 7.4 Repeat 7.2 and choose **Keep both**. **Paired devices** shows two
      rows with the same name and different fingerprints (both paired
      today, so tell them apart by fingerprint). Revoke the one whose
      fingerprint the phone no longer shows and confirm the newer phone is
      still connected.
- [ ] 7.5 Run one read and one destructive command against the re-paired
      device to confirm the new keys work end to end (steps 2.2 and 4.4).

## 8. iOS: local-network permission and Bonjour (iPhone only)

The Bonjour checks verify the plist entries, which the simulator does not
enforce and no CI job can.

- [ ] 8.1 On a fresh install, the **first** scan-and-connect shows the iOS
      "would like to find and connect to devices on your local network"
      prompt, with Headstate's own explanation text
      (`NSLocalNetworkUsageDescription`). Allow it.
- [ ] 8.2 Negative: reinstall, deny the prompt, and attempt to pair. The
      phone shows a message pointing at Settings, Privacy, Local Network,
      rather than a generic timeout. Re-enable it there and continue.
- [ ] 8.3 With the desktop on the same wifi, change the desktop's LAN
      address (reconnect wifi or toggle it) and reopen the app. The phone
      finds the desktop again through mDNS (`_headstate._tcp`) rather than
      only the addresses stored at pairing. Confirm in the desktop log that
      the connection came from the new address.
- [ ] 8.4 In the iOS Console (or a Bonjour browser on the same network)
      confirm `_headstate._tcp` is the only service type the app resolves,
      and that the TXT record carries the port and the first sixteen hex
      characters of the desktop fingerprint. Any other service type in the
      app's `NSBonjourServiceTypes` is a finding.

## 8a. Android: the Wi-Fi multicast lock (Android phone only)

Android's Wi-Fi driver filters inbound multicast unless the app holds a
`WifiManager.MulticastLock`, so without one an mDNS browse returns
nothing and looks exactly like "the desktop is not on this network"
(#610). Nothing in CI can catch that: there is no Android
instrumentation test, and a browse that finds nothing is a valid result.
This section is the only check the lock gets.

- [ ] 8a.1 With the desktop on the same wifi, change the desktop's LAN
      address (reconnect wifi or toggle it) and reopen the app. The phone
      finds the desktop again through mDNS rather than only the addresses
      stored at pairing. This is step 8.3 for Android, and it is the
      whole point of the lock: before the fix it failed here silently.
- [ ] 8a.2 In `adb logcat` during that reconnect, confirm
      `multicast lock acquired` is followed by `multicast lock released`.
      A missing release is a finding: the lock is a battery drain the
      user cannot see.
- [ ] 8a.3 Negative: during a browse (the few seconds after a failed
      reconnect), `adb shell dumpsys wifi | grep -i multicast` lists a
      held lock tagged `headstate-mdns`. After the browse ends it is
      gone. A lock still listed a minute later is a finding.
- [ ] 8a.4 Confirm no line matching `could not take the Wi-Fi multicast
      lock` appears in logcat. If one does, record it verbatim — that
      warning exists so this failure can never again be silent, and it
      names the reason.

## 9. Desktop closed: the unreachable banner

- [ ] 9.1 With the phone connected, quit desktop Headstate.
- [ ] 9.2 Within a short interval the phone's banner reads *octocat's
      laptop is unreachable · last seen …* with an amber dot.
- [ ] 9.3 The PR list is still rendered from the cached snapshot, marked
      stale, with every action disabled. Tapping an action does nothing and
      shows no biometric prompt.
- [ ] 9.4 Relaunch the desktop with the listener on. The phone reconnects
      on its own, the banner returns to *reachable*, and the list
      refreshes without re-pairing.
- [ ] 9.5 Overlay only, if testing one: move the phone off the wifi to
      mobile data with the overlay up. The phone fails the LAN address and
      succeeds on the overlay address (`100.64.0.7`) without user action.

## 10. Protocol mismatch: the "update Headstate on your desktop" banner

*Mark "when available" until a desktop build with a lower protocol version
than the phone requires exists to test against.*

- [ ] 10.1 Run a desktop build whose `/v1/hello` reports a protocol version
      lower than the phone's minimum.
- [ ] 10.2 The phone refuses to use it and shows the banner *Update
      Headstate on your desktop · this app needs protocol N, octocat's
      laptop has M*, naming both versions.
- [ ] 10.3 No command can be issued from that state; the banner is an
      *Update* link to the desktop's latest release rather than a button
      into pairing settings.
- [ ] 10.4 The reverse holds: a desktop on a newer protocol version accepts
      the older phone, with no banner.

## Findings

Record everything that did not match the checklist, one item per line, with
the step number. Include the ML-DSA line from step 1.3 even on a clean run.
If there are no findings, write `none`.

```
1.3  Post-quantum signing key: <offered / not offered> (<reason if shown>)
```

## Sign-off

| | |
|---|---|
| Result | clean / findings |
| Counts toward the two clean runs for enabling store builds | yes / no |
| Tester | |
