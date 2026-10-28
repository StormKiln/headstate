import { describe, expect, it } from "vitest";
import { settingsSections } from "./settingsSections";

describe("settingsSections", () => {
  it("offers Jira and GitLab on the desktop only", () => {
    const desktop = settingsSections(false).map((s) => s.id);
    const phone = settingsSections(true).map((s) => s.id);
    expect(desktop).toContain("jira");
    expect(desktop).toContain("gitlab");
    expect(phone).not.toContain("jira");
    expect(phone).not.toContain("gitlab");
    expect(phone).toContain("claude");
  });
});
