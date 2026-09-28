import { describe, expect, it } from "vitest";
import type { ClaudeMdAdviceFinding, ClaudeMdAdviceResult } from "@/types/pr";
import { groupFindings } from "./adviceGrouping";
import { groupMarkdown, reportMarkdown } from "./adviceMarkdown";

const REPO = "/home/octocat/hello-world";
const NOW = new Date("2026-01-01T03:00:00Z");

const finding = (over: Partial<ClaudeMdAdviceFinding> = {}): ClaudeMdAdviceFinding => ({
  check: "imports",
  severity: "problem",
  subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
  evidence: [
    { at: { kind: "file", path: `${REPO}/CLAUDE.md`, line: 4 }, measured: "`@./x.md`: file not found" },
  ],
  finding: "`@./x.md` in the file does not resolve",
  brief: "## THE BRIEF\n",
  ...over,
});

/// Two advice groups, one Note, one Unknown finding, and one check that
/// could not run -- the report #1399's test names.
const result = (): ClaudeMdAdviceResult => ({
  report: {
    repo: REPO,
    findings: [
      finding(),
      finding({
        check: "rot",
        severity: "advice",
        subject: { kind: "directory", path: `${REPO}/docs` },
        evidence: [],
        finding: "a | b is stale",
      }),
      finding({
        check: "rot",
        severity: "unknown",
        subject: { kind: "directory", path: REPO },
        evidence: [{ at: { kind: "session", sessionId: "abc", record: 7 }, measured: "not decided" }],
        finding: "could not tell whether `x|y` is used",
      }),
      finding({
        check: "shape",
        severity: "note",
        subject: { kind: "skill", path: `${REPO}/.claude/skills/s/SKILL.md`, name: "s" },
        evidence: [],
        finding: "12 rules found",
      }),
    ],
    checks: [
      { check: "imports", run: { state: "ran", findings: 1 } },
      { check: "rot", run: { state: "ran", findings: 2 } },
      { check: "shape", run: { state: "ran", findings: 1 } },
      { check: "skills", run: { state: "unknown", reason: "the skills directory could not be listed" } },
    ],
    brief: "# ALL BRIEFS\n",
  },
  freshness: { state: "cached", stale: false },
  computedAt: "2026-01-01T00:00:00Z",
  build: "7.6.0",
});

describe("reportMarkdown", () => {
  it("names the repository and says when and by which build it was checked", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md.startsWith(`## CLAUDE.md advice for \`${REPO}\`\n`)).toBe(true);
    expect(md).toContain("From the last check");
    expect(md).toContain("2026-01-01T00:00:00Z");
    expect(md).toContain("by Headstate 7.6.0");
  });

  it("includes every group with its counts, the Note, the Unknown and the check that could not run", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md).toContain("### imports (1 problem)");
    expect(md).toContain("### rot (1 advice, 1 could not decide)");
    expect(md).toContain("### Observations (1 observation)");
    expect(md).toContain("### skills (could not check)");
    expect(md).toContain("Could not check: the skills directory could not be listed");
    // Qualified, because one check could not run (absent is not zero).
    expect(md).toContain("1 of 4 checks could not run");
    expect(md).toContain("| observation | 12 rules found | `.claude/skills/s/SKILL.md` |");
    expect(md).toContain("| could not decide |");
  });

  it("escapes pipes in cells and keeps code spans intact", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md).toContain("| advice | a \\| b is stale | `docs/` |");
    expect(md).toContain("| could not decide | could not tell whether `x\\|y` is used | repository root |");
    // Every table row splits into exactly its three columns.
    for (const row of md.split("\n").filter((l) => l.startsWith("| "))) {
      expect(row.split(/(?<!\\)\|/).length).toBe(5);
    }
  });

  it("shortens paths as the panel does, and calls the repository the repository root", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md).toContain("| problem | `@./x.md` in the file does not resolve | `CLAUDE.md` |");
    expect(md).toContain("repository root");
    // The absolute path appears only in the header.
    expect(md.split(REPO).length).toBe(2);
  });

  it("lists each finding's evidence as a nested list under its sentence", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md).toContain(
      "- `@./x.md` in the file does not resolve (`CLAUDE.md`)\n  - `CLAUDE.md:4` — `@./x.md`: file not found",
    );
    expect(md).toContain("  - `session abc record 7` — not decided");
  });

  it("leaves the briefs out", () => {
    const md = reportMarkdown(result(), REPO, "check", false, NOW);
    expect(md).not.toContain("BRIEF");
  });

  it("lists checks that could not run on their own when grouped by file", () => {
    const md = reportMarkdown(result(), REPO, "file", false, NOW);
    expect(md).toContain("### Could not check");
    expect(md).toContain("- skills: the skills directory could not be listed");
    expect(md).toContain("### CLAUDE.md (1 problem)");
    expect(md).toContain("### docs/ (1 advice)");
  });

  it("heads the flat list as findings", () => {
    const md = reportMarkdown(result(), REPO, "none", false, NOW);
    expect(md).toContain("### Findings, worst first (1 problem, 1 advice, 1 could not decide)");
  });

  it("says nothing was found only when every check ran", () => {
    const clean: ClaudeMdAdviceResult = {
      ...result(),
      report: { ...result().report, findings: [], checks: [{ check: "imports", run: { state: "ran", findings: 0 } }] },
    };
    expect(reportMarkdown(clean, REPO, "check", false, NOW)).toContain("1 check ran; nothing found.");
  });
});

describe("groupMarkdown", () => {
  it("renders only its own group", () => {
    const [imports] = groupFindings(result().report, "check");
    const md = groupMarkdown(imports, REPO);
    expect(md.startsWith("### imports (1 problem)\n")).toBe(true);
    expect(md).toContain("| Severity | Finding | Where |\n| --- | --- | --- |");
    expect(md).not.toContain("stale");
    expect(md).not.toContain("##  ");
  });

  it("uses a longer fence for a path that holds a backtick", () => {
    const [g] = groupFindings(
      {
        ...result().report,
        findings: [finding({ subject: { kind: "directory", path: `${REPO}/a\`b` } })],
      },
      "check",
    );
    expect(groupMarkdown(g, REPO)).toContain("| ``a`b/`` |");
  });
});
