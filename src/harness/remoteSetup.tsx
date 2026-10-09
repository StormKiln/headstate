// Synthetic, reusable screenshot fixture. No provider or tailnet access.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { RemoteConnectionPanel } from "../components/RemoteConnectionPanel";
import "../index.css";

const phone = new URLSearchParams(location.search).get("mode") === "phone";
mockIPC((command) => {
  if (command === "get_connection_addresses" || command === "get_remote_connection_addresses") {
    return ["100.64.0.7", "studio.example.ts.net"];
  }
  if (command === "add_connection_address" || command === "check_desktop_connection") return;
  throw new Error("Unsupported synthetic preview command");
});
createRoot(document.getElementById("root")!).render(
  <main className="mx-auto max-w-xl p-5 text-[#e6edf3]">
    <h1 className="text-lg font-semibold">{phone ? "Desktop" : "Phone"}</h1>
    <p className="mt-2 text-xs text-[#8b949e]">Synthetic preview · Studio desktop</p>
    <RemoteConnectionPanel mode={phone ? "phone" : "desktop"} />
  </main>,
);
