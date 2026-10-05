import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  checkPermissions,
  openAppSettings,
  requestPermissions,
  scan,
  Format,
} from "@tauri-apps/plugin-barcode-scanner";
import { retireSourceOwnership } from "./sourceRefreshHooks";
import { call } from "./transport";

/// The phone's side of pairing: scanning a desktop's QR, pasting one,
/// and forgetting a desktop again.
///
/// The Rust half of all this shipped with #514 and has been complete
/// since: `pair_from_qr` validates the payload, pins the desktop's
/// certificate, generates the hardware-backed key and stores the
/// pairing; `unpair` forgets it. What was missing was any caller at
/// all -- the commands appeared in `remote.ts`'s `CLIENT_COMMANDS` set
/// and nowhere else, so a freshly installed phone had no way to pair
/// and fell through to the desktop's "install the GitHub CLI" screen.
///
/// Everything here is mobile-only by construction: `pair_from_qr` and
/// `unpair` are the companion's OWN commands, not the desktop's, and
/// the barcode scanner is a mobile plugin. The desktop build must not
/// import this module; `PairingScreen` is what guards that, behind
/// `IS_MOBILE_BUILD`.

/// What the camera can be in, as far as the pairing screen cares.
///
/// `prompt` covers Tauri's `prompt` and `prompt-with-rationale`: both
/// mean "asking will show the system sheet", which is the only
/// distinction the UI needs. `denied` is the one that matters -- on iOS
/// a denied camera can never be re-prompted by the app, so the only way
/// forward is Settings or the paste fallback, and the screen has to say
/// so rather than silently failing to open a camera.
export type CameraPermission = "granted" | "prompt" | "denied";

function normalise(state: string): CameraPermission {
  if (state === "granted") return "granted";
  if (state === "denied") return "denied";
  return "prompt";
}

/// The camera's current state, without asking for it.
export async function cameraPermission(): Promise<CameraPermission> {
  return normalise(await checkPermissions());
}

/// Ask for the camera, returning what the user decided.
///
/// Safe to call when already granted -- Tauri answers from the existing
/// grant without a sheet.
export async function askForCamera(): Promise<CameraPermission> {
  return normalise(await requestPermissions());
}

/// Send the user to the OS settings page for this app.
///
/// The only route out of `denied` on iOS. Re-exported rather than having
/// the component import the plugin directly, so that every barcode
/// plugin import in the app is in this file and the desktop bundle can
/// be checked for them.
export async function openCameraSettings(): Promise<void> {
  return await openAppSettings();
}

/// Scan one QR code and return its contents.
///
/// `windowed: false` gives the OS's own full-screen scanner UI, which
/// already handles focus, torch and framing better than anything worth
/// rebuilding, and which the user recognises. Only QR is accepted: the
/// desktop encodes the pairing payload as one, and admitting the other
/// dozen formats only widens what a mis-scan can hand to `parse_qr`.
export async function scanPairingCode(): Promise<string> {
  const result = await scan({ windowed: false, formats: [Format.QRCode] });
  return result.content;
}

/// Hand a QR payload to the companion, which does the actual pairing.
///
/// `payload` is whatever the camera read or the user pasted; Rust's
/// `parse_qr` is the validator, and this deliberately does not
/// pre-validate. A second copy of that parsing here would be a second
/// thing to keep in step with the desktop's QR format, and it would
/// reject payloads with a message the Rust side never wrote.
///
/// Resolves with the desktop's name.
function pairFromQr(payload: string, deviceName?: string): Promise<string> {
  return call<string>("pair_from_qr", { payload, deviceName });
}

/// Forget the paired desktop.
function unpair(): Promise<void> {
  return call<void>("unpair");
}

/// Pair, then make every cached answer from the previous desktop
/// unreachable.
///
/// The cache reset is the point of wrapping this in a mutation. Pairing
/// with a different desktop while the query cache still holds the last
/// one's pull requests would render that desktop's data under this
/// desktop's name until each query happened to refetch.
export function usePairFromQr() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ payload, deviceName }: { payload: string; deviceName?: string }) =>
      pairFromQr(payload, deviceName),
    onSuccess: async () => {
      retireSourceOwnership(client);
      await client.resetQueries();
    },
  });
}

/// Forget the desktop, and drop everything cached from it.
///
/// `resetQueries` rather than `invalidateQueries`: an invalidated query
/// refetches, and there is nothing left to refetch from. Reset clears
/// the data outright, which is also what makes this the right thing to
/// call when a desktop has REVOKED the phone -- the cached snapshot
/// must not outlive the permission to have it.
export function useUnpair() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: unpair,
    onSuccess: async () => {
      retireSourceOwnership(client);
      await client.resetQueries();
    },
  });
}
