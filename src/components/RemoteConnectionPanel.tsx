import { useRef, useState } from "react";
import { addConnectionAddress, checkDesktopConnection, getConnectionAddresses, getRemoteConnectionAddresses } from "@/api/remoteConnection";
import { Button } from "./ui/button";
import { RemoteSetupGuide } from "./RemoteSetupGuide";

type Operation = "addresses" | "add" | "check" | "copy";

// Match the native command's fixed, privacy-safe messages exactly. Never use
// substring matching or render an unknown error: transport details can include
// private hostnames. Unrecognized/new native failures take the generic fallback.
function connectionAdvice(error: unknown): string | undefined {
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : undefined;
  switch (message) {
    case "The endpoint did not match your paired desktop's identity.":
      return "The address did not match your paired desktop’s identity. Check that the address belongs to the original paired desktop. If you replaced the desktop, establish a new pairing by scanning and confirming its Headstate code.";
    case "The desktop did not authorize this phone.":
      return "The desktop did not authorize this phone. Check this phone’s access in the desktop’s paired-device list. If access was removed, an approved new pairing is required.";
    case "The secure connection failed. Check that this phone is still paired.":
      return "The secure connection failed. Check that this phone is still listed on the desktop and that both devices have current Headstate versions.";
    case "The connection check timed out. Check your private network and try again.":
      return "The connection check timed out. Check that the desktop is awake, Headstate is running and both devices are connected to the approved network, then try again. A timeout does not identify which check failed.";
    case "Enter a bare IP address or full DNS name, without a port or URL.":
      return "Enter an IP address or a full DNS name, without a URL scheme, port, path or brackets. Copy the address from the desktop’s private-network app.";
    case "The desktop returned an incompatible connection response.":
    case "The desktop uses an incompatible Headstate protocol. Update both apps.":
      return "The desktop’s connection response is incompatible. Update Headstate on both devices, then check again.";
    default:
      return undefined;
  }
}


export function RemoteConnectionPanel({ mode }: { mode: "phone" | "desktop" }) {
  const [address, setAddress] = useState("");
  const [addresses, setAddresses] = useState<string[] | null>(null);
  const [busy, setBusy] = useState<Operation | null>(null);
  const inFlight = useRef(false);
  const [notice, setNotice] = useState<{ error: boolean; text: string } | null>(null);
  const phone = mode === "phone";

  // Native commands own timeouts and coalescing. No timer here can start a
  // second operation while a native enrollment or probe is still running.
  async function run(operation: Operation, action: () => Promise<void>, failure: string) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(operation);
    setNotice(null);
    try {
      await action();
    } catch (error) {
      const advice = operation === "add" || operation === "check" ? connectionAdvice(error) : undefined;
      const text = advice === undefined ? failure : operation === "add"
        ? `Address not added. ${advice} Your existing pairing and addresses are retained.`
        : advice;
      setNotice({ error: true, text });
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  return (
    <section className="mt-5 space-y-3" aria-label={phone ? "Remote connection" : "Use away from home"}>
      <h3 className="text-sm font-medium">{phone ? "Remote connection" : "Use away from home"}</h3>
      <RemoteSetupGuide />
      <Button variant="outline" disabled={busy !== null} className="min-h-11" onClick={() => void run("addresses", async () => {
        setAddresses(await (phone ? getConnectionAddresses() : getRemoteConnectionAddresses()));
      }, "Could not read addresses. You can still copy the desktop address from its Tailscale app and enter it on the phone.")}>
        {busy === "addresses" ? "Reading addresses…" : phone ? "Show saved addresses" : "Show desktop addresses"}
      </Button>
      {addresses !== null ? (
        addresses.length === 0 ? <p className="text-xs text-[#8b949e]">No addresses found. Copy the desktop’s address from its Tailscale app.</p> :
        <ul className="space-y-2">{addresses.map((value) => <li key={value} className="flex items-center justify-between gap-2">
          <code className="select-all break-all text-xs">{value}</code>
          <Button variant="outline" className="min-h-11" aria-label={`Copy ${value}`} disabled={busy !== null} onClick={() => void run("copy", async () => {
            await navigator.clipboard.writeText(value);
            setNotice({ error: false, text: "Address copied." });
          }, "Could not copy the address. Select the address above and copy it manually.")}>Copy</Button>
        </li>)}</ul>
      ) : null}
      {phone ? <>
        <label className="block space-y-1.5">
          <span className="text-sm">Desktop address</span>
          <input className="w-full rounded border border-[#30363d] bg-[#161b22] px-3 py-2 text-base" value={address} onChange={(event) => setAddress(event.target.value)} disabled={busy !== null} maxLength={253} autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder="100.x.x.x or desktop.tailnet.ts.net" />
        </label>
        <p className="text-xs text-[#8b949e]">Enter a host only, without https://, a port or a path. An address is saved only after it verifies against your paired desktop.</p>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" className="min-h-11" disabled={busy !== null || address.trim() === ""} onClick={() => void run("add", async () => {
            const candidate = address.trim();
            await addConnectionAddress(candidate);
            // Reload only on a future explicit request; never display a guessed
            // persistence result or trigger network/provider refreshes here.
            setAddresses(null);
            setAddress("");
            setNotice({ error: false, text: "Address verified and saved. Your pairing and cached content are unchanged." });
          }, "Address not added. Check the address, keep the paired desktop awake with Headstate running, and connect both devices to the approved network. Your existing pairing and addresses are retained.")}>{busy === "add" ? "Verifying…" : "Verify and add address"}</Button>
          <Button variant="outline" className="min-h-11" disabled={busy !== null} onClick={() => void run("check", async () => {
            await checkDesktopConnection();
            setNotice({ error: false, text: "Desktop connection verified. Open a cached review and check for desktop updates; provider data was not refreshed by this check." });
          }, "Desktop connection could not be verified. Check the desktop and VPN connection, then use Connection troubleshooting above. Your pairing and cached content are retained.")}>{busy === "check" ? "Checking…" : "Check connection"}</Button>
        </div>
      </> : <p className="text-xs text-[#8b949e]">Copy the desktop address that your private network provides, then verify it on the paired phone in Settings → Desktop → Remote connection.</p>}
      {notice ? <p role={notice.error ? "alert" : "status"} className={`text-xs ${notice.error ? "text-[#ff7b72]" : "text-[#8b949e]"}`}>{notice.text}</p> : null}
    </section>
  );
}
