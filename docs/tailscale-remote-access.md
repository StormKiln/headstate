# Use Headstate away from your desktop

Headstate can connect a paired phone to its desktop over a private network such as Tailscale, even on cellular or another Wi-Fi network. Install the official Tailscale app on both devices. Headstate keeps its existing device pairing and encrypted connection; joining a tailnet does not authorize a phone to use Headstate.

Your desktop must be awake, online, running Headstate, and have **Allow phone connections** enabled. The phone does not connect directly to GitHub or GitLab. Keep ordinary local-network access available as a fallback.

## Choose an account

For personal, non-commercial use, Tailscale offers a free Personal plan. For work, use your organization's approved tailnet and plan; do not assume business use is free. Review [current pricing](https://tailscale.com/pricing) and ask your administrator about device approval and VPN policy. Headstate does not request Tailscale passwords, API keys or auth keys.

## Connect both devices

1. Install [Tailscale on your desktop](https://tailscale.com/download). Sign in to your account or approved organization and connect.
2. Install [Tailscale for iOS](https://tailscale.com/docs/install/ios), sign in to the same approved tailnet, approve the iOS VPN configuration, and connect. Complete any administrator device approval.
3. In desktop Headstate, open **Settings → Phone** and enable **Allow phone connections**. If the phone is not paired, use **Pair a phone**, scan the code in Headstate on the phone, compare the fingerprint, and approve on the desktop. Pairing initially on the same LAN is convenient.
4. For an already-paired phone, keep the pairing. Find the desktop's Tailscale IP address in Tailscale, or use its full MagicDNS hostname. Desktop Headstate can show interface addresses, but cannot determine from an IP alone whether Tailscale owns it. If it does not list the VPN address, copy it from Tailscale.
5. In phone Headstate, open **Settings → Desktop → Remote connection**. Enter the desktop's bare IP address or full hostname, without `https://`, a path, or a port. Add the address. Headstate checks the existing paired desktop's identity before saving the address. A failed check leaves the existing pairing and cached data intact.
6. Run **Check connection**. Then open a cached PR and confirm normal updates resume. This checks Headstate itself, rather than relying on Tailscale's device indicator or a ping.
7. Turn Wi-Fi off on the phone, leaving cellular data and Tailscale connected. Repeat the connection check and open a PR. For a write test, use a synthetic test PR and confirm on the desktop/provider that it happened once.

If your tailnet restricts access, ask the administrator to allow the approved phone to reach the desktop on **TCP 41919** within the existing policy. See [Tailscale grants](https://tailscale.com/docs/features/access-control/grants). Do not replace an organization's access policy wholesale or disable its firewall.

## Optional automatic connection

[Tailscale VPN On Demand](https://tailscale.com/docs/features/client/ios-vpn-on-demand) can connect automatically on cellular and selected Wi-Fi networks. Choose rules appropriate for your devices. Headstate does not change VPN settings. A managed or competing VPN may prevent this configuration; consult the administrator before changing it.

## When a connection fails

A timeout does not establish which of these conditions caused it. Check:

- The desktop is awake, online, and Headstate is running with phone connections enabled.
- Both Tailscale clients are connected to the intended tailnet; device approval and authentication have not expired.
- The saved address is the desktop's current VPN address or full hostname. LAN discovery does not replace a remote address.
- Tailnet policy and the desktop firewall allow TCP 41919 from the phone.
- Another VPN or device-management policy is not blocking Tailscale. iOS generally permits one active VPN tunnel; see [VPN compatibility](https://tailscale.com/docs/reference/faq/other-vpns).

If the desktop identity cannot be verified, check that you entered the correct desktop address. Do not bypass the fingerprint check. If the desktop removed the phone, authorize it with a new pairing rather than treating a new network address as permission.

If a command times out after submission, it may have executed. Check its resulting state before submitting it again. Connection recovery must not replay an approval, merge, or other action automatically.

## Privacy and limitations

Tailscale may connect directly or carry encrypted traffic through a relay. Relays do not decrypt Headstate's pinned mutual-TLS traffic. Do not describe this as traffic never passing through an intermediate server. See [connection types](https://tailscale.com/docs/reference/connection-types).

No public port forwarding, Funnel, exit node, subnet router, or HTTPS-terminating proxy is needed. A proxy that replaces the desktop certificate breaks Headstate's identity check.

Tailscale does not keep a sleeping desktop available. iOS background refresh and notifications remain subject to operating-system scheduling; remote access does not promise instant background notifications. Foreground interaction is the supported workflow.

## Acceptance record

Automated loopback tests establish protocol behavior, identity checks, retained caches, bounded recovery, and action dispatch. They do not establish real VPN, relay, cellular, firewall, or OS background behavior.

Owner-device checks should record app versions and platform, whether the route was LAN/cellular/other Wi-Fi, whether Tailscale reports a direct or relayed path, cold versus warm tunnel behavior, and the outcome of connection/read/event/action checks. Use synthetic data and omit private account, repository, endpoint and device identifiers from shared reports.

| Environment | Required check | Status |
| --- | --- | --- |
| iPhone + macOS | Existing pairing, add VPN after pairing, cellular and Wi-Fi handoff | Owner-device verification pending |
| macOS client variants | Address visibility and listener reachability | Owner-device verification pending |
| Windows / Linux desktop | Address visibility, firewall and phone access | Owner-device verification pending |
| Cold tunnel / relay | Bounded connection, retained content and no duplicate action | Owner-device verification pending |

## Reusable synthetic preview

Run `yarn vite --host 127.0.0.1 --port 1437` and open `/harness/remote-setup.html` for desktop settings, or append `?mode=phone` for the phone controls. This renders the real setup components with synthetic IPC responses and never uses personal account or provider data. It is a layout/screenshot fixture, not a successful network test. Keep it for future screenshots.

### Automated 10.0.0 verification

The local release candidate passed 3,491 desktop Rust tests (58 ignored), 5,227 shared frontend tests (one skipped), and 242 companion core tests (one ignored), plus 43 companion plugin tests. These counts describe software checks, not physical VPN acceptance.

Regression coverage includes a reachable endpoint after the first eight legacy QR candidates, certificate mismatch and authorization failures, 150 concurrent cold reads sharing one greeting, no automatic command replay, cancellation of stalled subscriber connection attempts, retained cached data, and a learned LAN address changing while remote enrollment awaits verification. A desktop hello test verifies zero provider requests when the viewer is unknown and reuse of a viewer learned by a normal provider operation.

The synthetic preview was inspected in desktop and phone-sized layouts. Actual iPhone cellular/relay testing and native clipboard/VPN/OS interactions remain in the owner-device matrix above.
