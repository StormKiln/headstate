# Headstate 10: remote companion access over a tailnet

**Status:** Implementation authorized 2026-10-09 under [epic #1762](https://github.com/StormKiln/headstate/issues/1762), with linked work #1763–#1768 and #1760. Physical-device acceptance is tracked explicitly; automated protocol evidence does not establish real cellular/relay behavior.

**Goal:** An already-paired phone can use its desktop over cellular or another Wi-Fi network, with the same authentication, cached state, and action safeguards as local access.

**Architecture:** Use the official Tailscale clients and a user- or organization-managed tailnet as an optional network path. Preserve Headstate's existing pinned mutual TLS connection, desktop-held provider credentials, command surface, and authorization. Keep ordinary LAN access working independently.

**Stack:** Existing React/TanStack Query frontend, Tauri desktop/mobile, Rust/reqwest/rustls companion transport, official Tailscale applications.

**Design baseline:** `docs/superpowers/specs/2026-09-05-mobile-companion-design.md`, especially Reachability. The proposal below extends onboarding and address lifecycle; it does not replace that security design.

## Recommendation and alternatives

| Approach | Assessment |
| --- | --- |
| Official Tailscale apps plus Headstate setup guidance and connection checks | Recommended for 10.0. Smallest additional trust and maintenance surface; works with an existing approved tailnet. Requires a separate app/sign-in and compatible VPN policy. |
| Embed Tailscale networking into Headstate | Defer. Introduces another networking runtime, key/state lifecycle, distribution and mobile integration work. Do not assume embedding removes mobile restrictions or makes commercial service use free. |
| Operate a Headstate rendezvous/relay service | Separate product decision if users require zero VPN setup. Adds hosting, availability, abuse prevention and security operations. Unnecessary for the requested paired-device feature. |

Headscale/self-hosted WireGuard can remain advanced compatible-network options. They are not simpler onboarding defaults: someone must operate the control plane or arrange reachability. Do not build a second managed-network product inside Headstate 10.

## Cost: free for personal use, not a universal enterprise promise

As checked on 2026-10-07, Tailscale's Personal plan is $0, supports six users and unlimited user devices, and is explicitly for non-commercial use. A personal phone and desktop fit. Standard is advertised at $8/user/month; business users should use their organization's approved plan. A personal email address is not a justification for representing commercial use as free. Pricing should be linked and date-qualified rather than hard-coded as a permanent product promise. [Tailscale pricing](https://tailscale.com/pricing)

The recommended architecture needs no Headstate-hosted relay, extra router, exit node, or public certificate subscription. Headstate would not sell/provision Tailscale accounts or collect Tailscale credentials. Ordinary internet/mobile-data costs still apply.

## What exists already, and what remains unproven

Inspected at main commit `84ce8279b06251eb6031fc50ae73b7192b2fa41e`:

- `src-tauri/src/remote/listener.rs`: existing service port **TCP 41919**.
- `src-tauri/src/remote/gate.rs`: opt-in phone listener, currently bound to the IPv6 wildcard with dual-stack intent. Tailnet support should not require another server or a broader bind.
- `src-tauri/src/remote/pairing.rs`: interface-address enumeration includes overlay addresses in the pairing QR; loopback and IPv6 link-local are filtered.
- `src-mobile/src/client.rs`: pinned TLS identity, multiple stored addresses, 250 ms staggered path selection, one-second TCP connect timeout, preferred-path reuse, and commands sent to one resolved address. Preserve those single-dispatch protections.
- `src-mobile/src/companion.rs`: persists the last working address and preserves paired-desktop state. It does not constitute a complete remote-address enrollment/update flow.
- `src/components/PairedDesktopPanel.tsx`: currently identifies the desktop and allows forgetting it; add address management without requiring destructive unpairing.
- `src/components/PairingScreen.tsx`: currently mentions a shared VPN but says nothing is sent through an intermediate server. Correct this for encrypted relay paths.
- `docs/mobile-pairing-walkthrough.md`: has an optional overlay test, not a complete Tailscale setup/acceptance procedure.

These are code observations, not proof that every desktop Tailscale distribution exposes usable addresses to `if_addrs`, or that cellular/relayed connections work within the current deadlines. Verify those points first.

## User experience to ship

Add **Settings → Phone → Use away from home** on desktop and **Settings → Desktop → Remote connection** on the phone. Keep the guide readable while disconnected. Offer “Set up Tailscale” and “I already use Tailscale/an approved private network.”

The guide should walk through:

1. **Choose the network.** For personal use, create a Tailscale account/tailnet through the official website. For work, use the organization's approved tailnet and ask its administrator when enrollment is restricted.
2. **Connect the desktop.** Install the official Tailscale client for macOS, Windows or Linux, sign in, and connect. Keep Headstate running and enable “Allow phone connections.” Show the address Headstate will advertise, with copy/manual-entry fallback when detection is unavailable.
3. **Connect the phone.** Install Tailscale from the official store, sign in to the same tailnet (or accept the administrator's invitation), approve the OS VPN configuration, and connect. Explain that device approval may be required. [Official iOS installation](https://tailscale.com/docs/install/ios)
4. **Keep or establish Headstate pairing.** New users scan the existing pairing QR and confirm on desktop. Already-paired users add/update remote addresses while keeping their pinned identity, device permissions and cached data. Connecting Tailscale alone never authorizes a phone in Headstate.
5. **Check access.** Run an authenticated Headstate connection check to the selected remote endpoint, then verify the event stream and a cached read. A generic ping is insufficient. For restricted policies, provide the administrator with the needed source phone/device and destination desktop **TCP 41919**. Apply rules within the existing policy; never replace it wholesale. [Tailscale grants](https://tailscale.com/docs/features/access-control/grants)
6. **Prove off-LAN use.** Turn Wi-Fi off on the phone, leave cellular and Tailscale connected, then open Headstate and run the same check. Merely being on different Wi-Fi names or seeing a green Tailscale device does not prove end-to-end app access.
7. **Optional automatic connection.** Link to Tailscale's VPN On Demand settings; document cellular “Always” and an appropriate Wi-Fi rule as user choices. Do not silently change VPN settings. [VPN On Demand](https://tailscale.com/docs/features/client/ios-vpn-on-demand)

Show short states such as “Connected,” “Connecting,” and “Desktop unreachable”; put detailed troubleshooting behind a disclosure. A timeout alone cannot distinguish sleep, firewall, wrong tailnet or disconnected VPN. Label possible causes as checks, not diagnoses. Do not label a route “Tailscale” merely because its IP is in the shared CGNAT range.

## Ordered implementation work

### 1. Establish feasibility and platform behavior

- [ ] On an actual paired iPhone and macOS desktop, test LAN, cellular, and another Wi-Fi network using official Tailscale clients. Include an already-paired phone where Tailscale was installed afterward.
- [ ] Repeat desktop compatibility on Windows/Linux and both supported macOS client distribution variants as applicable; record whether address enumeration, listener binding and firewall behavior work.
- [ ] Measure a cold tunnel, VPN On Demand startup and a relayed path; validate the current one-second TCP connect limit instead of assuming it is sufficient or increasing every command timeout.
- [ ] Confirm the TLS handshake, permissions, event stream and existing remote action flow work through the overlay. Use synthetic data/repos for mutations.

**Deliverable:** A compatibility/latency matrix with actual results and concrete blockers. This determines whether documentation plus address lifecycle is sufficient.

### 2. Add safe endpoint enrollment and repair

Primary seams: desktop `remote/pairing.rs`; mobile `pairing.rs`, `client.rs`, `companion.rs`; desktop `PairPhonePanel.tsx`; phone `PairedDesktopPanel.tsx`; matching API/wire contracts.

- [ ] Add a connection-address update flow for an existing pairing. Recommended: desktop displays an address-only QR plus fingerprint, or the phone accepts a manually entered Tailscale IP/optional full MagicDNS name.
- [ ] Treat imported addresses as candidates, not identities. Verify against the already-pinned desktop certificate and authenticated hello before promoting/persisting a working candidate. Never replace the pin from an address-update QR or DNS answer.
- [ ] Validate host-only input, port and bounded candidate counts; reject URL paths, userinfo, malformed addresses and unbounded QR payloads. Retain prior usable addresses and the existing desktop ownership boundary.
- [ ] Support adding the overlay after pairing, stale addresses, desktop/node re-enrollment and hostname changes. Failed verification must retain the prior pairing and cache; identity replacement requires the normal explicit pairing procedure.
- [ ] Preserve old pairing records and protocol compatibility. Add a capability/version fence only if a wire change actually requires it.

**Deliverable:** Existing users gain remote access without forgetting their desktop. Tests cover hostile/mismatched endpoint input, certificate mismatch, revocation, multiple address families and restart persistence.

### 3. Make path changes reliable without query amplification

Primary seams: `src-mobile/src/client.rs`, `companion.rs`, mobile event/lifecycle handlers and existing shared query/cache hooks.

- [ ] Coalesce connection establishment across screens/background tasks; keep a bounded candidate set, shared deadline and jittered backoff. Preserve successful-path reuse. Avoid continuous tailnet discovery or per-row connection probes.
- [ ] On network change or confirmed path failure, invalidate only reachability evidence, safely revalidate candidate paths, and resume one event subscription. Do not flush PR content, drafts, disclosures or cached inventories.
- [ ] Never race or automatically replay a possibly-executed mutation across paths. If an approval/merge response is lost, represent the outcome as unconfirmed and reconcile authoritative state before offering another write. Preserve existing step-up/replay guards.
- [ ] Connection diagnostics use hello/transport checks and locally cached state; they must not call GitHub/GitLab just to prove connectivity. Reconnection performs bounded shared catch-up, not one provider refresh per visible PR.
- [ ] Keep transport reachability, data freshness and action eligibility separate. A live tunnel does not prove fresh mergeability; a transient tunnel failure does not erase accepted data.

**Deliverable:** Deterministic LAN/cellular handoff, packet-loss and ambiguous-write tests; verify actual request counts under a 150+ PR synthetic workload. Adjust connection timing only where feasibility measurements support it.

### 4. Ship onboarding, troubleshooting and truthful diagnostics

Primary seams: new focused setup-guide component, `SettingsDialog.tsx`, `PairPhonePanel.tsx`, `PairedDesktopPanel.tsx`, `PairingScreen.tsx`, `ConnectionBanner.tsx`, and the pairing walkthrough.

- [ ] Implement the seven-step guide above with personal/work paths, installation links, address copy/update and an actual authenticated test button.
- [ ] Report observable stages: address resolution, TCP reachability, TLS identity, authorization, hello compatibility and event-stream health. Separate observed failure from suggested checks.
- [ ] Explain desktop sleep/app exit, expired Tailscale authentication, wrong tailnet, device approval, ACL/firewall restrictions, local-network permission and VPN conflict. Provide a manual path when Tailscale CLI/status access is unavailable.
- [ ] Record bounded timings, address class, candidate ordinal, reconnect count and failure category. Routine shareable logs omit private hostnames, addresses, account/repo/PR names, pairing tokens and keys. Detailed addresses may be shown locally on request.
- [ ] Update privacy copy: “Your phone and desktop communicate over an encrypted connection. Your private network may relay encrypted traffic.” Do not claim direct routing or no intermediaries without evidence. DERP forwards encrypted packets and does not decrypt them. [DERP documentation](https://tailscale.com/docs/reference/derp-servers)

**Deliverable:** Accessible, offline-readable setup and actionable diagnostics, with frontend/native tests and updated synthetic screenshots.

### 5. Validate and release desktop/mobile together

- [ ] Test Wi-Fi → cellular → other Wi-Fi, cold start, sleep/wake, desktop restart, VPN off/on, On Demand, dropped event stream, revoked phone, expired node authentication, denied policy/firewall, changed endpoint, wrong certificate, MagicDNS unavailable, IPv4/IPv6 and relayed operation.
- [ ] Prove cached content survives a disconnect and reconnect; no repeated approvals/merges, no extra event subscriptions, no duplicate provider refresh storms, and no LAN regression without Tailscale installed.
- [ ] Run full repository lint, frontend/Rust/mobile gates, security-focused review of endpoint/identity handling, and the physical-device matrix. A synthetic tunnel test alone is insufficient evidence for iOS VPN behavior.
- [ ] Publish compatible desktop and companion versions using existing release gates and artifact verification. Keep synthetic data. Document supported/tested combinations and any remaining owner acceptance explicitly.

**Tracking:** [epic #1762](https://github.com/StormKiln/headstate/issues/1762) links #1763–#1768 and CI work #1760. Review added #1769 (provider-independent hello), #1770 (legacy QR compatibility), and #1771 (retaining learned fallback addresses). The epic carries merged commits and release evidence.

## Blockers and complications

| Concern | Consequence and handling |
| --- | --- |
| Enterprise policy and cost | Work use is not universally eligible for the free plan. MDM may forbid installation or personal-tailnet enrollment. Prefer an existing approved network; do not suggest bypassing controls. |
| Another required phone VPN | Tailscale documents a single-active-VPN restriction on iOS/Android. An enforced corporate VPN can block this design for that user; validate the actual managed device. Desktop VPN routes/firewalls can also conflict. [VPN interoperability](https://tailscale.com/docs/reference/faq/other-vpns) |
| Sleeping/offline desktop | The phone still depends on the running desktop and its credentials. Tailscale is not a wake service. Explain power requirements; do not promise closed-lid availability or silently prevent sleep. |
| Mobile background execution | Existing notifications are best-effort local notifications from OS-scheduled refresh, without APNs. A tailnet does not make the companion continuously runnable or guarantee instant background alerts. Foreground remote interaction is the 10.0 scope. |
| Discovery | Do not rely on LAN Bonjour across the tailnet. Persist remote candidates and allow repair; MagicDNS is hostname resolution, not Headstate service discovery. Tailscale's mDNS feature request remains open. [Upstream issue](https://github.com/tailscale/tailscale/issues/1013) |
| Relay/cold-radio latency | An available VPN is not a LAN-speed guarantee. Test bounded connection establishment and preserve caches. Relays are valid connection paths, not automatic errors. [Connection types](https://tailscale.com/docs/reference/connection-types) |
| Key expiry or node removal | Reauthentication or new endpoints may be needed. Guide recovery without automatically disabling expiry or changing Headstate trust. [Key expiry](https://tailscale.com/docs/features/access-control/key-expiry) |
| Address collisions and observability | Shared CGNAT ranges and another VPN may conflict. Tailscale client/CLI availability differs by platform; provide manual entry and never infer an installed/connected state from an IP alone. |

## Avoid unnecessary complexity

Use the existing direct TCP listener with its pinned mutual TLS. No exit node, subnet router, public port forwarding, Tailscale Funnel, HTTPS-terminating Serve proxy, public DNS certificate, Headstate account system, OAuth tailnet administration, or stored Tailscale API/auth key is required. TLS termination in front of the listener would interfere with Headstate's existing certificate identity model.

Do not broaden the listener or disable host firewalls for setup. The current listener can accept on multiple interfaces; existing pairing authentication remains essential even when tailnet access is allowed. Tailnet-only binding can be considered separately if users need it, but is not a prerequisite and must not break local fallback.

The recommended launch scope is **user-managed private networking with excellent Headstate guidance and recovery**, not automatic network provisioning. The two consequential product assumptions are that foreground operation is sufficient and the desktop remains available. If either is false, that calls for a separate availability/notification architecture rather than more Tailscale configuration.

## Implementation decisions and evidence

- Manual address entry and desktop interface-address copy satisfy enrollment; a second address-only QR format is deferred. Existing pairing QR v2 remains supported.
- Safe hello discovery is shared across concurrent cold commands. Commands are dispatched once; connection recovery does not replay them. Successful paths remain reusable, and confirmed transport failure retires reachability evidence. Foreground resume and connection failures drive recovery; no continuous Tailscale polling or new OS VPN controller is introduced.
- Diagnostics report observed failure categories, bounded timings and candidate family/ordinal without raw endpoint errors. An unreachable result does not reliably distinguish DNS, firewall and TCP failures, so the UI lists checks instead of inventing a cause. Hello success does not claim that provider data or the event stream is current.
- The one-second TCP timeout remains unchanged pending actual cold-tunnel measurements. New explicit checks have an eight-second overall deadline. Real cellular, relay and VPN On Demand behavior remains owner-device acceptance, following the user's existing arrangement to test released builds and share logs. It is not marked passed by automated tests.
- See [remote-access setup and acceptance record](../../tailscale-remote-access.md) for supported configuration and pending physical checks.
