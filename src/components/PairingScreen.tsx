import { useEffect, useState } from "react";
import { Camera, ClipboardPaste, Loader2, Settings, QrCode } from "lucide-react";
import {
  type CameraPermission,
  askForCamera,
  cameraPermission,
  openCameraSettings,
  scanPairingCode,
  usePairFromQr,
} from "@/api/pairing";
import { describePairingFailure, type PairingFailure } from "@/lib/pairingError";
import { Button } from "./ui/button";

/// The phone's first screen, and the only one it has until it is paired.
///
/// The companion drives a desktop; until it knows which desktop, there
/// is nothing else it can usefully show. This mounts ABOVE `AuthGate`
/// on the mobile build, which matters: `AuthGate` gates on
/// `get_auth_state`, and on the phone that command is forwarded to a
/// desktop, so while unpaired it rejects and the user landed on the
/// desktop's "Headstate needs the GitHub CLI · brew install gh" screen.
/// A phone has no Homebrew and no shell, so that screen was both wrong
/// and inescapable.
///
/// Two ways in, because one of them can be taken away: the camera is
/// the fast path, and the paste field is what remains when the camera
/// is denied, broken, or covered. `parse_qr` on the Rust side does not
/// care where the string came from, so paste costs nothing but a
/// textarea and is the difference between a recoverable state and a
/// reinstall.

/// A phone's own name for itself, offered to the desktop so its paired
/// device list says something better than "My phone".
///
/// The user's OWN name for the device -- the one in iOS Settings -- is
/// what belongs here and is not reachable: since iOS 16
/// `UIDevice.current.name` returns the model unless the app holds
/// `com.apple.developer.device-information.user-assigned-device-name`,
/// which must be applied for (#634). Until that is granted this guesses
/// from the platform, which at least beats "My phone" and matches what
/// `pairing.rs::default_device_name` would send if the field were left
/// blank.
function defaultDeviceName(): string {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/iPad/i.test(ua)) return "iPad";
  if (/iPhone|iPod/i.test(ua)) return "iPhone";
  if (/Android/i.test(ua)) return "Android phone";
  return "My phone";
}

/// Steps that own the whole screen. `scanning` is not one: the scanner
/// is the OS's own full-screen UI, so during a scan this component is
/// simply covered.
type Mode = "choose" | "paste";

/// `revokedBy` names the desktop that withdrew this phone's access, when
/// that is why we are here. Absent on a first run.
export function PairingScreen({ revokedBy }: { revokedBy?: string } = {}) {
  const [mode, setMode] = useState<Mode>("choose");
  const [camera, setCamera] = useState<CameraPermission | null>(null);
  const [pasted, setPasted] = useState("");
  const [deviceName, setDeviceName] = useState(defaultDeviceName);
  const [failure, setFailure] = useState<PairingFailure | null>(null);
  const [scanning, setScanning] = useState(false);
  const pair = usePairFromQr();

  // Asked once, on mount, WITHOUT requesting: knowing the camera is
  // already denied lets the screen lead with the paste field instead of
  // offering a Scan button that can only fail.
  useEffect(() => {
    let live = true;
    cameraPermission().then(
      (p) => live && setCamera(p),
      // A plugin that is not there at all (a desktop build that somehow
      // reached this screen, or a simulator without a camera) reads the
      // same as denied for our purposes: offer paste.
      () => live && setCamera("denied"),
    );
    return () => {
      live = false;
    };
  }, []);

  const submit = (payload: string) => {
    setFailure(null);
    pair.mutate(
      { payload, deviceName: deviceName.trim() || undefined },
      { onError: (e) => setFailure(describePairingFailure(e)) },
    );
  };

  const startScan = async () => {
    setFailure(null);
    // Request only when the user has asked to scan. Prompting on mount
    // asks for a camera before saying what it is for, which is how
    // people learn to tap Don't Allow.
    const state = camera === "granted" ? "granted" : await askForCamera();
    setCamera(state);
    if (state !== "granted") return;
    setScanning(true);
    try {
      submit(await scanPairingCode());
    } catch (e) {
      // A cancelled scan is a decision, not a failure: the user backed
      // out of the OS scanner, and telling them "pairing failed" for
      // that is noise.
      const message = e instanceof Error ? e.message : String(e);
      if (!/cancel/i.test(message)) setFailure(describePairingFailure(e));
    } finally {
      setScanning(false);
    }
  };

  const busy = pair.isPending || scanning;

  return (
    // Its own scroller: the document never scrolls (#1583).
    <div className="flex h-full flex-col overflow-y-auto bg-[#0d1117] text-[#e6edf3]">
      <div className="mx-auto flex w-full max-w-md flex-1 flex-col gap-6 px-6 pt-[max(2rem,env(safe-area-inset-top))] pb-[max(2rem,env(safe-area-inset-bottom))]">
        <header className="space-y-2">
          <h1 className="text-xl font-semibold">
            {revokedBy === undefined ? "Pair with your desktop" : "Pair again"}
          </h1>
          {/* Why we are back here, before anything else. Without it a
              revoked phone looks like an app that forgot its pairing,
              rather than a desktop that withdrew it -- and the user has
              no reason to expect scanning again to behave differently. */}
          {revokedBy !== undefined ? (
            <p
              role="status"
              className="rounded border border-[#d29922]/30 bg-[#d29922]/10 p-3 text-sm text-[#d29922]"
            >
              {revokedBy} no longer recognises this phone. Someone removed it from that
              desktop’s paired devices, so it has been signed out. Scan a new code there
              to pair again.
            </p>
          ) : null}
          {/* What the app IS, on the first screen. The desktop's
              equivalent screen learned this lesson already: the one
              statement of scope used to live in a branch most people
              never saw. */}
          {/* What the app IS, and the prerequisite, before any
              instruction. The GitHub sign-in sentence used to live here:
              accurate, but it answers a question nobody is asking while
              trying to pair, and it pushed the thing that actually
              matters -- you need the desktop app first -- below the
              fold (#634). */}
          <p className="text-sm text-[#8b949e]">
            Headstate Companion drives Headstate on your computer — reviewing pull
            requests and cleaning up worktrees, artifacts and Docker from your phone.
          </p>
        </header>

        {/* Said plainly, and before the steps. Someone who has not
            installed the desktop app is otherwise being told to open
            menus in software they do not have. */}
        <p className="rounded border border-[#30363d] bg-[#161b22] p-3 text-sm text-[#e6edf3]">
          <span className="font-medium">Set up your computer first.</span> This app does
          nothing on its own — install Headstate on your Mac, Windows or Linux machine and
          leave it running, then pair this phone to it.
        </p>

        <ol className="space-y-1 text-sm text-[#8b949e]">
          <li>1. On your desktop, open Headstate → Settings → Phone.</li>
          <li>2. Turn on “Allow phone connections” and choose “Pair a phone”.</li>
          <li>3. Scan the code it shows.</li>
        </ol>

        <label className="space-y-1.5">
          <span className="text-sm text-[#8b949e]">This phone’s name</span>
          <input
            value={deviceName}
            onChange={(e) => setDeviceName(e.target.value)}
            aria-label="This phone’s name"
            placeholder={defaultDeviceName()}
            // 16px: anything smaller and iOS zooms the viewport on
            // focus and never zooms back out.
            className="w-full rounded border border-[#30363d] bg-[#161b22] px-3 py-2 text-base text-[#e6edf3] placeholder:text-[#8b949e]"
          />
          <span className="text-xs text-[#8b949e]">
            How this phone appears in your desktop’s paired devices list.
          </span>
        </label>

        {failure !== null ? <PairingFailureNotice failure={failure} /> : null}

        {mode === "choose" ? (
          <div className="space-y-3">
            {camera === "denied" ? (
              <CameraDenied onOpenSettings={() => void openCameraSettings()} />
            ) : (
              <Button
                onClick={() => void startScan()}
                disabled={busy}
                // 44pt minimum: the primary action on a touch screen.
                className="min-h-11 w-full text-base"
              >
                {scanning ? (
                  <Loader2 className="size-4 animate-spin" aria-hidden="true" />
                ) : (
                  <Camera className="size-4" aria-hidden="true" />
                )}
                {scanning ? "Scanning…" : "Scan pairing code"}
              </Button>
            )}
            <Button
              variant="outline"
              onClick={() => {
                setFailure(null);
                setMode("paste");
              }}
              disabled={busy}
              className="min-h-11 w-full text-base"
            >
              <ClipboardPaste className="size-4" aria-hidden="true" />
              Enter the code instead
            </Button>
          </div>
        ) : (
          <PasteForm
            value={pasted}
            onChange={setPasted}
            onSubmit={() => submit(pasted)}
            onBack={() => {
              setFailure(null);
              setMode("choose");
            }}
            busy={busy}
          />
        )}

        <p className="mt-auto text-xs text-[#8b949e]">
          Both devices need to be on the same network, or connected through the same
          VPN. Nothing is sent to any server in between.
        </p>
      </div>
    </div>
  );
}

/// The dead end iOS creates: once the camera is denied, the app can
/// never ask again, and nothing in the UI used to say so. Without this
/// the Scan button simply stopped working, which reads as a broken app
/// rather than a setting the user can change.
function CameraDenied({ onOpenSettings }: { onOpenSettings: () => void }) {
  return (
    <div className="space-y-3 rounded border border-[#d29922]/30 bg-[#d29922]/10 p-3">
      <p className="text-sm text-[#d29922]">
        Headstate cannot use the camera. To scan a code, allow camera access in
        Settings → Privacy → Camera, then come back. You can also enter the code by
        hand below.
      </p>
      <Button
        variant="outline"
        onClick={onOpenSettings}
        className="min-h-11 w-full text-base"
      >
        <Settings className="size-4" aria-hidden="true" />
        Open Settings
      </Button>
    </div>
  );
}

function PasteForm({
  value,
  onChange,
  onSubmit,
  onBack,
  busy,
}: {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  onBack: () => void;
  busy: boolean;
}) {
  return (
    <div className="space-y-3">
      <label className="space-y-1.5">
        <span className="text-sm text-[#8b949e]">Pairing code</span>
        <textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          aria-label="Pairing code"
          rows={5}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          placeholder='{"v":2,"name":"…"}'
          className="w-full rounded border border-[#30363d] bg-[#161b22] px-3 py-2 font-mono text-base text-[#e6edf3] placeholder:text-[#8b949e]"
        />
        <span className="text-xs text-[#8b949e]">
          Your desktop can show the code as text beneath the QR image.
        </span>
      </label>
      <Button
        onClick={onSubmit}
        disabled={busy || value.trim() === ""}
        className="min-h-11 w-full text-base"
      >
        {busy ? (
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <QrCode className="size-4" aria-hidden="true" />
        )}
        {busy ? "Pairing…" : "Pair"}
      </Button>
      <Button
        variant="ghost"
        onClick={onBack}
        disabled={busy}
        className="min-h-11 w-full text-base"
      >
        Back
      </Button>
    </div>
  );
}

/// A failure, with its next step. `retryable` decides the tone: a
/// fingerprint mismatch and an expired code both stop the flow, but only
/// one of them is worth trying again unchanged.
function PairingFailureNotice({ failure }: { failure: PairingFailure }) {
  return (
    <div role="alert" className="space-y-1 rounded border border-[#f85149]/30 bg-[#f85149]/10 p-3">
      <p className="text-sm font-medium text-[#f85149]">{failure.title}</p>
      <p className="text-sm text-[#e6edf3]">{failure.detail}</p>
      {/* Collapsed by default: when the advice above is right, this is
          noise. When it is wrong -- "check both devices are on the same
          network" to someone whose devices are on the same network --
          it is the only way to learn anything without a debugger
          (#633). `<details>` rather than a state hook so it costs
          nothing and works with VoiceOver's native affordance.

          `select-all` and `break-all`: the point is to get this into an
          issue, and an IPv6 address does not wrap on a phone. */}
      {failure.technical !== "" && failure.technical !== failure.detail ? (
        <details className="pt-1">
          <summary className="cursor-pointer text-xs text-[#8b949e]">Show details</summary>
          <p className="mt-1 select-all break-all font-mono text-xs text-[#8b949e]">
            {failure.technical}
          </p>
        </details>
      ) : null}
    </div>
  );
}
