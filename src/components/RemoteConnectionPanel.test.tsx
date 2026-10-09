import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("@/api/transport", () => api);
import { RemoteConnectionPanel } from "./RemoteConnectionPanel";

beforeEach(() => api.call.mockReset());
it("keeps the offline guide available without any automatic connection queries", () => {
  render(<RemoteConnectionPanel mode="phone" />);
  expect(screen.getByText("Remote connection")).toBeTruthy();
  fireEvent.click(screen.getByText("Set up Tailscale"));
  expect(screen.getByText(/same tailnet/)).toBeTruthy();
  expect(screen.getByText(/non-commercial/)).toBeTruthy();
  expect(api.call).not.toHaveBeenCalled();
});
it("checks once, blocks repeated checks while pending, and preserves cached content", async () => {
  const client = new QueryClient();
  client.setQueryData(["cached", "desktop"], { title: "Existing review" });
  let finish!: () => void;
  api.call.mockReturnValue(new Promise<void>((resolve) => { finish = resolve; }));
  render(<QueryClientProvider client={client}><RemoteConnectionPanel mode="phone" /></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Check connection" }));
  fireEvent.click(screen.getByRole("button", { name: "Checking…" }));
  expect(api.call).toHaveBeenCalledExactlyOnceWith("check_desktop_connection");
  await act(async () => finish());
  expect(screen.getByRole("status").textContent).toContain("Desktop connection verified");
  expect(client.getQueryData(["cached", "desktop"])).toEqual({ title: "Existing review" });
});
it("verifies an added address without clearing pairing or cache, and keeps private failure details out of the UI", async () => {
  const client = new QueryClient();
  client.setQueryData(["cached"], ["review"]);
  api.call.mockRejectedValueOnce(new Error("certificate mismatch at secret-work.ts.net token=private"));
  render(<QueryClientProvider client={client}><RemoteConnectionPanel mode="phone" /></QueryClientProvider>);
  fireEvent.change(screen.getByLabelText("Desktop address"), { target: { value: "  studio.example.ts.net  " } });
  fireEvent.click(screen.getByRole("button", { name: "Verify and add address" }));
  await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
  expect(api.call).toHaveBeenCalledExactlyOnceWith("add_connection_address", { address: "studio.example.ts.net" });
  expect(screen.getByRole("alert").textContent).toContain("not added");
  expect(document.body.textContent).not.toContain("secret-work");
  expect(client.getQueryData(["cached"])).toEqual(["review"]);
});
it("shows stored addresses locally only when asked and adds an authenticated candidate", async () => {
  api.call.mockResolvedValueOnce(["100.90.80.70"]).mockResolvedValueOnce(undefined);
  render(<RemoteConnectionPanel mode="phone" />);
  fireEvent.click(screen.getByRole("button", { name: "Show saved addresses" }));
  await screen.findByText("100.90.80.70");
  fireEvent.change(screen.getByLabelText("Desktop address"), { target: { value: "studio.example.ts.net" } });
  fireEvent.click(screen.getByRole("button", { name: "Verify and add address" }));
  await screen.findByText(/Address verified and saved/);
  expect(api.call.mock.calls.map(([name]) => name)).toEqual(["get_connection_addresses", "add_connection_address"]);
});
it("reads desktop interface addresses on demand without issuing a pairing token or enabling connections", async () => {
  api.call.mockResolvedValue(["100.90.80.70"]);
  render(<RemoteConnectionPanel mode="desktop" />);
  expect(api.call).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Show desktop addresses" }));
  await screen.findByText("100.90.80.70");
  expect(api.call).toHaveBeenCalledExactlyOnceWith("get_remote_connection_addresses");
  expect(screen.getByRole("button", { name: "Copy 100.90.80.70" })).toBeTruthy();
});
it.each([
  ["The endpoint did not match your paired desktop's identity.", "Check that the address belongs to the original paired desktop"],
  ["The desktop did not authorize this phone.", "Check this phone’s access in the desktop’s paired-device list"],
  ["The secure connection failed. Check that this phone is still paired.", "Check that this phone is still listed on the desktop"],
  ["The connection check timed out. Check your private network and try again.", "The connection check timed out"],
  ["Enter a bare IP address or full DNS name, without a port or URL.", "Enter an IP address or a full DNS name"],
  ["The desktop returned an incompatible connection response.", "Update Headstate on both devices"],
  ["The desktop uses an incompatible Headstate protocol. Update both apps.", "Update Headstate on both devices"],
])("turns the exact safe failure %s into actionable advice", async (error, advice) => {
  api.call.mockRejectedValueOnce(error);
  render(<RemoteConnectionPanel mode="phone" />);
  fireEvent.change(screen.getByLabelText("Desktop address"), { target: { value: "studio.example.ts.net" } });
  fireEvent.click(screen.getByRole("button", { name: "Verify and add address" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Address not added");
  expect(alert.textContent).toContain(advice);
  if (error.includes("identity")) expect(alert.textContent).not.toContain("VPN");
});
it("requires an exact safe error match, never echoing appended endpoint details", async () => {
  api.call.mockRejectedValueOnce(new Error("The endpoint did not match your paired desktop's identity. secret-work.ts.net"));
  render(<RemoteConnectionPanel mode="phone" />);
  fireEvent.click(screen.getByRole("button", { name: "Check connection" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("could not be verified");
  expect(alert.textContent).not.toContain("secret-work");
  expect(alert.textContent).not.toContain("identity");
});
