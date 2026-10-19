# 10.9.1 closeout field testing

This is the current checklist for #1737 (enterprise review reliability) and #1487 (native/phone transcript performance), tracked by [epic #1911](https://github.com/StormKiln/headstate/issues/1911). It replaces the operational instructions in the historical 9.2 guide. PR Stats was removed; there is no Stats step here.

## Versions and what is still unverified

Use [desktop 10.9.1](https://github.com/StormKiln/headstate/releases/tag/v10.9.1) once the release is published. Until then, 10.9.0 is the delivered baseline. The matching companion remains **10.9.0, build 53**: this patch changes developer verification and documentation, not shared frontend, mobile code or the pairing protocol. Its [published IPA](https://github.com/StormKiln/headstate/releases/tag/mobile-v10.9.0) and Apple upload are verified; record whether TestFlight actually offers build 53. Upload acceptance does not prove tester availability. Do not label an older companion's results as build 53.

Record the actual desktop version, companion version/build, macOS/iOS versions and phone model for every session. Use the owner's iPhone 17 Pro; its results do not establish an older iPhone's performance floor. Native/WebKit proxies, synthetic enterprise tests and successful exports do not complete field acceptance.

## Enterprise laptop: one ordinary review session

1. In **Settings → General**, enable **Write a detailed timing log (for diagnosing slowness)**. If it was already enabled before restart, switch it off and back on locally to establish a fresh capture boundary for mounted observations. Existing files remain retained.
2. Open **To Review → All repositories**. Observe at least two ordinary refresh/recovery cycles without repeated forced refreshes. Record approximate local times, non-identifying inventory/Ready counts and the exact freshness/footer wording. Record the approximate number of repositories/members/PRs; the desired enterprise workload is 50+/50+/150+, but never substitute invented counts for the actual session.
3. Open Ready PRs, including ones with queue/review/terminal changes. Perform an approval only when normally authorized and wanted, observe readiness/merge controls, return to the list, and check whether newly accepted facts reconcile into membership. One reviewer's approval is not necessarily aggregate approval. If recovery needs Refresh, record one normal attempt and its result rather than repeatedly clicking it.
4. Leave a comment expanded or an unsent draft present through background refresh; navigate away/back, then restart in a separate phase. Note content loss, collapsed disclosures, changed focus, request storms, persistent disabled actions or misleading freshness. Retained data can remain qualified while useful; optional advisory age alone is not an inventory failure.
5. Near the end of each short phase, use **Settings → General → Measurement report → Save measurement report…**. Preserve the original export. This control's status updates on mount, local toggle and export completion, not continuous polling.
6. Turn capture off when finished. Send the separate measurement report privately with the result template below. **Show the log** opens the general log, which can contain private company/repository/PR identifiers; do not attach that log or screenshots to public issues. No database dump or extra provider queries are needed.

Absence of mounted events is not proof that a view was never shown. A local acknowledged toggle establishes capture linkage; native/remote preference changes can leave stale links that are rejected and counted as loss. Phone capture has an independent identity.

## iPhone 17 Pro: separate short transcript phases

1. Verify the installed TestFlight version/build, pair normally and record LAN/tailnet/other connection category without addresses. If build 53 is unavailable, report that delivery observation and leave its acceptance pending. Enable desktop capture separately for host work.
2. In phone General settings, under **This phone**, enable **Record phone measurements**. This switch is independent of desktop capture and is also available from the unpaired settings surface.
3. Use preserved, designated synthetic sessions. Record only their generic fixture labels and approximate sizes. Do not overwrite personal sessions or generate private company content for testing. Benchmark payloads are not automatically installed as phone sessions; use the existing fixture preparation workflow in [the native harness guide](../scripts/transcript-native/README.md).
4. In separate phases: open newest content; navigate to beginning/middle/end; load older pages and scroll; choose **Latest**; remain parked while the synthetic session grows; background/foreground the phone. Scrubbing to EOF should stay detached until Latest is explicitly selected. Note position jumps, delayed content, errors, and whether memory/residency remains bounded where actually measured.
5. After each phase use **Share recent measurement report…** under **This phone**. Confirm that the destination really saved the file. Preparation or a displayed share sheet is not successful transfer; cancellation is not a captured report. **Retry phone measurement status** retries the local control/status operation, not provider data refresh.
6. Export the host report separately, retain both originals and disable phone recording when done. Sharing retained data works while recording is off. Never subtract clocks from different devices to infer end-to-end latency.

## Inspect reports and measure the remaining budgets

Run `make analyze-measurements INPUT=/absolute/report.jsonl`, or `python3 scripts/measurement-report.py /absolute/report.jsonl --json` for structured output. The analyzer is offline and does not rewrite inputs. Exit 0 means structurally valid without detected qualifications, **not acceptance**; exit 1 means usable but qualified; exit 2 means invalid/truncated/unreadable or a parser bound. Preserve qualifications and unknowns.

Phone exports contain a bounded recent suffix (8 MiB including metadata); omitted prefixes are counted. Desktop retention is normally eight 16 MiB segments, not a guaranteed duration. Export promptly between short phases. Keep role/epoch/capture populations separate. Historical epochs have unknown build attribution unless evidence actually establishes it; report header version describes the exporting process, not every retained epoch. The analyzer deliberately avoids echoing arbitrary build strings, so include installed versions separately in the result template.

For #1487, consult [the budget table](transcript-performance.md) and distinguish:

- **Usability observations:** content/position continuity and whether capture/export succeeds on the exact build.
- **Existing proxies:** native read time, client wait, DOM-ready/double-rAF opportunity, owned-process RSS/footprint. These are useful diagnostics, not compositor presentation or transcript-attributed memory.
- **Remaining measured acceptance:** actual WKWebView presentation, scrolling/frame behavior, attributed WebContent memory/CPU, older-page residency, phone transfer/decode and highlighting cost. Use Safari Web Inspector/Instruments when available and identify the measured process/layer. Host RSS is not renderer memory; total WebContent memory is not transcript allocations. Mark inaccessible metrics unmeasured, never zero or passing.

The native probe now rejects malformed sample shapes, non-integral/boolean/non-finite memory, out-of-range byte counts and invalid PID identities. A valid probe remains `measured-proxies`, with `nativeBudgetAcceptance: unmeasured`.

## Result to send back

- Desktop version; companion version/build actually installed; OS versions; phone model.
- Approximate phase start/end times, connection category and generic workload/fixture counts.
- Steps taken, expected behavior, observed behavior and whether a normal recovery action helped.
- Separate original desktop/phone measurement exports, provided privately.
- Instruments/Web Inspector measurements if collected, including process/layer, units and method; otherwise say unmeasured.

Do not include company names, repository/PR identities, addresses, pairing secrets or transcript text in public issue comments. Reproduced defects receive their own linked issues. #1737/#1487 stay open until their actual evidence supports closure; #1754 separately remains open until the spontaneous native crash has an established cause and verified correction.
