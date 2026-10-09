import { ExternalLink } from "./ExternalLink";

/** Static instructions remain available with no desktop, VPN or internet. */
export function RemoteSetupGuide() {
  return (
    <div className="space-y-3 text-xs text-[#8b949e]">
      <details>
        <summary className="min-h-11 cursor-pointer py-3 text-sm text-[#e6edf3]">Set up Tailscale</summary>
        <p className="mb-3">
          For personal, non-commercial use, see the <ExternalLink href="https://tailscale.com/pricing">Personal plan and current pricing</ExternalLink>.
          For work, use your organization’s approved tailnet and plan; ask your administrator before installing or enrolling devices.
        </p>
        <ol className="list-decimal space-y-3 pl-5">
          <li>Install the <ExternalLink href="https://tailscale.com/download">official Tailscale app</ExternalLink> on your desktop and phone.</li>
          <li>Sign in to the same tailnet on both devices and connect. Accept your administrator’s invitation and wait for device approval if required. On the phone, approve the OS VPN configuration.</li>
          <li>Keep the desktop awake and Headstate running. In Settings → Phone, enable “Allow phone connections”. A sleeping or closed-lid desktop may be unavailable.</li>
          <li>In the desktop’s Tailscale app, copy its 100.x address or full MagicDNS name (ending in .ts.net). Desktop settings can show local interface addresses; an address alone does not prove Tailscale is connected.</li>
          <li>Already paired? On the phone, use Settings → Desktop → Remote connection to verify and add the address. Your pairing and cached content stay in place. For a new pairing, scan the desktop’s Headstate QR and confirm the matching fingerprint there. Pair locally first, or scan a fresh QR over the connected private network when its addresses are reachable.</li>
          <li>On the phone, choose “Check connection”. This verifies the paired desktop connection; it does not refresh provider data. Then open a cached review and check that desktop updates arrive.</li>
          <li>Turn phone Wi-Fi off, leave cellular and Tailscale connected, and check again. This proves access away from your local network. Optional: configure <ExternalLink href="https://tailscale.com/docs/features/client/ios-vpn-on-demand">VPN On Demand</ExternalLink>, choosing cellular “Always” and a Wi-Fi rule that suits you.</li>
        </ol>
      </details>
      <details>
        <summary className="min-h-11 cursor-pointer py-3 text-sm text-[#e6edf3]">I already use Tailscale or an approved private network</summary>
        <p>Connect both devices to that network. Keep Headstate running with phone connections enabled, then copy the desktop’s private address or full DNS name into “Desktop address” on the paired phone. Connecting the network does not authorize a phone in Headstate; first-time users still need to scan and confirm a Headstate pairing code.</p>
      </details>
      <details>
        <summary className="min-h-11 cursor-pointer py-3 text-sm text-[#e6edf3]">Connection troubleshooting</summary>
        <p>A failed check does not identify the cause. Check desktop sleep or app exit, VPN connection and expired sign-in, the correct tailnet, pending device approval, and the address. If MagicDNS is unavailable, try the desktop’s current Tailscale IP.</p>
        <p className="mt-2">Ask your administrator to check existing policy and the desktop firewall for access from this phone to the desktop on TCP 41919. Check local-network permission for LAN access and whether another required phone VPN conflicts. Use an approved network; do not bypass your organization’s controls.</p>
        <p className="mt-2">No public port forwarding, Funnel, exit node or subnet router is needed. Background alerts remain best effort; keep Headstate open on the phone for interactive use.</p>
      </details>
      <p>Your phone and desktop communicate over an encrypted connection. Your private network may relay encrypted traffic.</p>
    </div>
  );
}
