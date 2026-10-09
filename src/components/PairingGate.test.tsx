import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/// The regression this file exists for: a freshly installed phone
/// reached the DESKTOP's "Headstate needs the GitHub CLI · brew install
/// gh" screen and could not get past it.
///
/// The path was `main.tsx` -> `AuthGate` -> `get_auth_state`, which on
/// the mobile build is forwarded over `remote_call` and rejects with
/// "not paired with a desktop" while unpaired. So the auth check failed
/// on every fresh install, and the remediation it offered -- Homebrew
/// and a shell -- exists on neither iOS nor Android. Pairing was
/// unreachable, so the app was unreachable.

const connection = vi.hoisted(() => ({ current: { state: "unpaired" } as unknown }));
const dismissed = vi.hoisted(() => ({ count: 0 }));

// The transport is the seam: `connection_state` is the companion's own
// command, and on the mobile build `useConnectionState` polls it.
vi.mock("../api/transport", () => ({
  call: (name: string) => {
    if (name === "connection_state") return Promise.resolve(connection.current);
    return Promise.reject(new Error(`not paired with a desktop (${name})`));
  },
  listen: () => Promise.resolve(() => {}),
}));

vi.mock("../splash", () => ({
  dismissSplash: () => {
    dismissed.count += 1;
  },
  initSplash: () => {},
}));

// The scanner plugin is native-only; importing it under jsdom would
// fail before any assertion could run.
vi.mock("@tauri-apps/plugin-barcode-scanner", () => ({
  scan: () => Promise.reject(new Error("no camera in a test")),
  cancel: () => Promise.resolve(),
  checkPermissions: () => Promise.resolve("prompt"),
  requestPermissions: () => Promise.resolve("prompt"),
  openAppSettings: () => Promise.resolve(),
  Format: { QRCode: "QR_CODE" },
}));

async function renderGate(target: string) {
  vi.stubEnv("VITE_TARGET", target);
  vi.resetModules();
  const { PairingGate } = await import("./PairingGate");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <PairingGate>
        <div>the app</div>
      </PairingGate>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  connection.current = { state: "unpaired" };
  dismissed.count = 0;
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("PairingGate", () => {
  it("sends an unpaired phone to the pairing screen, not to the gh CLI screen", async () => {
    await renderGate("mobile");
    expect(await screen.findByText(/pair with your desktop/i)).toBeTruthy();
    // The heart of it: no impossible advice reachable on a phone.
    expect(screen.queryByText(/brew install/i)).toBeNull();
    expect(screen.queryByText(/GitHub CLI/i)).toBeNull();
    expect(screen.queryByText("the app")).toBeNull();
  });

  it("says who revoked the phone rather than silently asking again", async () => {
    // Walkthrough step 6.1 requires the phone to return to the pairing
    // screen "with a message saying the desktop no longer recognises
    // it". Without the name this reads as the app having forgotten,
    // rather than the desktop having decided.
    connection.current = { state: "revoked", desktop: "octocat's laptop", last_poll: null };
    await renderGate("mobile");
    expect(await screen.findByText(/no longer recognises this phone/i)).toBeTruthy();
    expect(screen.getByText(/octocat's laptop/i)).toBeTruthy();
    expect(screen.getByRole("heading", { name: /pair again/i })).toBeTruthy();
  });

  it("lets a paired phone through to the app", async () => {
    connection.current = {
      state: "connected",
      desktop: "octocat's laptop",
      last_poll: null,
      protocol_version: 2,
    };
    await renderGate("mobile");
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.queryByText(/pair with your desktop/i)).toBeNull();
  });

  it("dismisses the splash on the unpaired screen", async () => {
    // The v1.0.0 hang, generalised. The splash is a fixed inset-0
    // z-index-9999 overlay, so ANY branch that renders a real screen
    // without dismissing it hides that screen forever -- and "unpaired"
    // is a real screen, not a loading state.
    await renderGate("mobile");
    await screen.findByText(/pair with your desktop/i);
    await waitFor(() => expect(dismissed.count).toBeGreaterThan(0));
  });

  it("holds the splash while the connection state is still unknown", async () => {
    // The one state that SHOULD keep it up: answering "not paired"
    // before the companion has spoken would flash the pairing screen at
    // someone who is already paired.
    let release: (v: unknown) => void = () => {};
    connection.current = new Promise((r) => {
      release = r;
    });
    await renderGate("mobile");
    expect(dismissed.count).toBe(0);
    release({ state: "unpaired" });
  });

  it("is a pass-through on the desktop build", async () => {
    // Guarded on the build target, not the viewport: a narrow desktop
    // window is still a desktop, has `gh`, and must still get AuthGate.
    await renderGate("desktop");
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.queryByText(/pair with your desktop/i)).toBeNull();
  });
});

it("offers remote setup before pairing with truthful relay privacy copy", async () => {
  await renderGate("mobile");
  await screen.findByText("Set up Tailscale");
  expect(screen.getByText(/Your private network may relay encrypted traffic/)).toBeTruthy();
  expect(screen.queryByText(/Nothing is sent to any server/)).toBeNull();
});
