import { describe, expect, it } from "vitest";
import type { ClaudeMdAdviceFinding, ClaudeMdAdviceReport } from "@/types/pr";
import { recheckSummary } from "./adviceRecheck";

const REPO = "/home/octocat/hello-world";

const f = (finding: string, over: Partial<ClaudeMdAdviceFinding> = {}): ClaudeMdAdviceFinding => ({
  check: "imports",
  severity: "advice",
  subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
  evidence: [],
  finding,
  brief: "",
  ...over,
});

const report = (findings: ClaudeMdAdviceFinding[]): ClaudeMdAdviceReport => ({
  repo: REPO,
  findings,
  checks: [],
  brief: "",
});

describe("recheckSummary", () => {
  it("counts the findings and says how many fewer", () => {
    const s = recheckSummary(report([f("a"), f("b"), f("c")]), report([f("a")]));
    expect(s.title).toBe("Re-checked: 1 finding");
    expect(s.description).toBe("2 fewer than the report it replaced.");
  });

  it("says how many more", () => {
    const s = recheckSummary(report([]), report([f("a"), f("b")]));
    expect(s.title).toBe("Re-checked: 2 findings");
    expect(s.description).toBe("2 more than the report it replaced.");
  });

  it("says no change only when the findings are the same findings", () => {
    expect(recheckSummary(report([f("a"), f("b")]), report([f("b"), f("a")])).description).toBe(
      "No change from the report it replaced.",
    );
    expect(recheckSummary(report([f("a")]), report([f("z")])).description).toBe(
      "The same number as the report it replaced, but not the same findings.",
    );
  });

  /// A Note is not advice (#1339): it is neither counted nor compared.
  it("ignores observations", () => {
    const note = f("137 sessions", { severity: "note" });
    const s = recheckSummary(report([f("a")]), report([f("a"), note]));
    expect(s.title).toBe("Re-checked: 1 finding");
    expect(s.description).toBe("No change from the report it replaced.");
  });

  it("makes no comparison with nothing to compare against", () => {
    expect(recheckSummary(undefined, report([f("a")])).description).toBeUndefined();
  });
});
