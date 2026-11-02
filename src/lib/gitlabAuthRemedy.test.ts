import { describe, expect, it } from "vitest";
import type { GitLabAuthState } from "@/api/tauri";
import { gitlabAuthRemedy } from "./gitlabAuthRemedy";

const failed = (over: Partial<GitLabAuthState>): GitLabAuthState => ({
  host: "gitlab.com", ok: false, issue: "unverified", message: "", configured: false, signedInHosts: null, ...over,
});

describe("gitlabAuthRemedy", () => {
  it("offers the one signed-in host and Settings, never a retry that checks the same wrong host", () => {
    expect(gitlabAuthRemedy(failed({ signedInHosts: ["gitlab.example.com"] }), false))
      .toEqual({ useHost: "gitlab.example.com", settings: true, retry: false });
  });

  it("sends the phone to the desktop's Settings in words, with no button it cannot use", () => {
    expect(gitlabAuthRemedy(failed({ signedInHosts: ["gitlab.example.com"] }), true))
      .toEqual({ useHost: null, settings: false, retry: false });
  });

  it("does not pick between several signed-in hosts", () => {
    expect(gitlabAuthRemedy(failed({ signedInHosts: ["a.example.com", "b.example.com"] }), false))
      .toEqual({ useHost: null, settings: true, retry: false });
  });

  it("keeps the retry when glab is signed in nowhere else: `glab auth login` can fix it", () => {
    expect(gitlabAuthRemedy(failed({ configured: true, signedInHosts: null }), false))
      .toEqual({ useHost: null, settings: false, retry: true });
    // The checked host is not "another" host.
    expect(gitlabAuthRemedy(failed({ configured: true, signedInHosts: ["gitlab.com"] }), false).retry).toBe(true);
  });

  it("points an unsaved default host at Settings alongside the retry", () => {
    expect(gitlabAuthRemedy(failed({ configured: false }), false))
      .toEqual({ useHost: null, settings: true, retry: true });
  });

  it("treats an older desktop that sends no new fields as a saved host", () => {
    expect(gitlabAuthRemedy({ host: "gitlab.com", ok: false, issue: "unverified", message: "" }, false))
      .toEqual({ useHost: null, settings: false, retry: true });
  });

  it("offers only Settings for an invalid host, and a retry when the check never answered", () => {
    expect(gitlabAuthRemedy(failed({ issue: "invalidHost", host: "" }), false)).toEqual({ useHost: null, settings: true, retry: false });
    expect(gitlabAuthRemedy(undefined, false)).toEqual({ useHost: null, settings: false, retry: true });
  });
});
