import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PR_FIXTURES } from "@/fixtures/prs";
import { renderWithQuery as render } from "@/test-utils";
import { READY_BATCH_PROMPT } from "@/lib/readyClaudify";
import type { ReadyRow } from "@/lib/readyMarkdown";
import type { PullRequest } from "@/types/pr";
import { ReadyClaudify } from "./ReadyClaudify";

/// The scan, prefs and viewer the batch Claudify reads (#1579): the same
/// hooks every pull request Claudify reads, controlled here.
const env = vi.hoisted(() => ({
  repos: [] as { identity: string | null; name: string; path: string; worktrees: never[] }[],
  terminal: "",
  viewer: { data: "me" as string | undefined, isError: false, isPending: false },
}));
vi.mock("@/api/hooks", () => ({
  useWorktrees: () => ({ data: env.repos, unreadable: [], isError: false, error: null }),
  useUiPrefs: () => ({ prefs: { terminal_command: env.terminal } }),
  useViewer: () => env.viewer,
}));
const tauri = vi.hoisted(() => ({
  claudifyPrCommand: vi.fn(),
  claudeLaunchPr: vi.fn(),
  claudeLaunchPrPreview: vi.fn(),
  claudeLaunchTerms: vi.fn(),
  actOnPrs: vi.fn(),
}));
vi.mock("@/api/tauri", async (orig) => ({ ...(await orig<object>()), ...tauri }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const pr = (number: number, repo = "acme/widget"): PullRequest => ({
  ...PR_FIXTURES[0],
  number,
  repo,
  title: `Change ${number}`,
  url: `https://github.com/${repo}/pull/${number}`,
});
const ROWS: ReadyRow[] = [
  { pr: pr(1, "acme/gadget") },
  { pr: pr(2), lastPusher: { state: "known", login: "carol" } },
];

const button = () => screen.getByRole("button", { name: /Claudify|Copy prompt/ }) as HTMLButtonElement;
const reason = () => document.querySelector("[data-ready-claudify-reason]")?.textContent ?? "";

beforeEach(() => {
  env.repos = [];
  env.terminal = "";
  env.viewer = { data: "me", isError: false, isPending: false };
  for (const f of Object.values(tauri)) f.mockReset();
  tauri.claudeLaunchTerms.mockResolvedValue({
    models: [],
    permissionModes: ["default", "bypassPermissions"],
    unattended: ["bypassPermissions"],
  });
  tauri.claudeLaunchPrPreview.mockImplementation(
    (_checkout: string, _repo: string, prompt: string) =>
      Promise.resolve({ program: "claude", args: [prompt] }),
  );
  tauri.claudeLaunchPr.mockResolvedValue(undefined);
});

describe("ReadyClaudify (#1579)", () => {
  it("is disabled, with the reason, when nothing is showing", () => {
    render(<ReadyClaudify rows={[]} />);
    expect(button().disabled).toBe(true);
    expect(reason()).toMatch(/No pull requests are showing/);
  });

  it("is disabled until the viewer's login is read, and says why when it cannot be", () => {
    env.viewer = { data: undefined, isError: false, isPending: true };
    const { unmount } = render(<ReadyClaudify rows={ROWS} />);
    expect(button().disabled).toBe(true);
    expect(reason()).toMatch(/Reading your GitHub login/);
    unmount();
    env.viewer = { data: undefined, isError: true, isPending: false };
    render(<ReadyClaudify rows={ROWS} />);
    expect(button().disabled).toBe(true);
    expect(reason()).toMatch(/could not be read/);
  });

  it("opens the shared terms dialog, previews the whole prompt, and launches only on request", async () => {
    // The checkout found is the SECOND row's repository: the first has none.
    env.repos = [{ identity: "acme/widget", name: "widget", path: "/code/widget", worktrees: [] }];
    env.terminal = "wezterm start -- bash -lc {command}";
    render(<ReadyClaudify rows={ROWS} />);
    expect(button().textContent).toContain("Claudify");
    fireEvent.click(button());
    expect(await screen.findByText("Hand 2 ready pull requests to Claude Code")).toBeTruthy();
    expect(screen.getByText(/on the prompt for these 2 pull requests/)).toBeTruthy();

    // The preview is built from the exact prompt, and shows it.
    await waitFor(() => expect(tauri.claudeLaunchPrPreview).toHaveBeenCalled());
    const [checkout, repo, prompt] = tauri.claudeLaunchPrPreview.mock.calls[0] as [string, string, string];
    expect([checkout, repo]).toEqual(["/code/widget", "acme/widget"]);
    expect(prompt.startsWith(READY_BATCH_PROMPT)).toBe(true);
    expect(prompt).toContain("I am @me on GitHub.");
    expect(prompt).toContain("[Change 1]");
    expect(prompt).toContain("last pusher: @carol");
    expect(prompt).toContain("last pusher: not checked");
    expect(await screen.findByText(/Review all of these PRs/)).toBeTruthy();

    // Nothing has run yet: only the preview.
    expect(tauri.claudeLaunchPr).not.toHaveBeenCalled();

    // The permission mode is the user's, and "act without asking" is
    // labelled as it is everywhere else.
    const permissions = screen.getByRole("combobox", { name: /Permissions/ });
    await waitFor(() => expect((permissions as HTMLSelectElement).disabled).toBe(false));
    fireEvent.change(permissions, { target: { value: "bypassPermissions" } });
    expect(await screen.findByText(/This session will act without asking/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Open in terminal" }));
    await waitFor(() => expect(tauri.claudeLaunchPr).toHaveBeenCalledTimes(1));
    expect(tauri.claudeLaunchPr.mock.calls[0]).toEqual([
      "/code/widget",
      "acme/widget",
      prompt,
      { permissionMode: "bypassPermissions" },
    ]);
    // It starts Claude and does nothing to the pull requests itself.
    expect(tauri.actOnPrs).not.toHaveBeenCalled();
  });

  it("copies the prompt alone when no repository has a checkout", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(<ReadyClaudify rows={ROWS} />);
    expect(button().textContent).toContain("Copy prompt");
    expect(reason()).toMatch(/No local checkout/);
    fireEvent.click(button());
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText.mock.calls[0][0].startsWith(READY_BATCH_PROMPT)).toBe(true);
    expect(tauri.claudeLaunchPr).not.toHaveBeenCalled();
  });

  it("copies the command with no terminal configured, as every Claudify does", async () => {
    env.repos = [{ identity: "acme/gadget", name: "gadget", path: "/code/gadget", worktrees: [] }];
    tauri.claudifyPrCommand.mockResolvedValue({ command: "cd x && claude y", claude_installed: true });
    const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(<ReadyClaudify rows={ROWS} />);
    fireEvent.click(button());
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("cd x && claude y"));
    const [checkout, repo, prompt] = tauri.claudifyPrCommand.mock.calls[0] as [string, string, string];
    expect([checkout, repo]).toEqual(["/code/gadget", "acme/gadget"]);
    expect(prompt.startsWith(READY_BATCH_PROMPT)).toBe(true);
    expect(screen.queryByText(/Hand .* to Claude Code/)).toBeNull();
  });
});
