import { describe, expect, it } from "vitest";
import type {
  ClaudeMdAdviceCheck,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceSubject,
} from "@/types/pr";
import { CHECK_LABEL, OBSERVATIONS_KEY, REPOSITORY_ROOT, groupFindings } from "./adviceGrouping";

const REPO = "/home/octocat/hello-world";

function finding(over: Partial<ClaudeMdAdviceFinding> = {}): ClaudeMdAdviceFinding {
  return {
    check: "imports",
    severity: "advice",
    subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
    evidence: [],
    finding: "a finding",
    brief: "## brief\n",
    ...over,
  };
}

function report(over: Partial<ClaudeMdAdviceReport> = {}): ClaudeMdAdviceReport {
  return {
    repo: REPO,
    findings: [],
    checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
    brief: "# advice\n",
    ...over,
  };
}

const claudeMd = (path: string): ClaudeMdAdviceSubject => ({
  kind: "claudeMd",
  path,
  scope: "repo",
  section: null,
});

describe("groupFindings", () => {
  /// The flat arrangement is the wire, untouched: one group, the same
  /// array, in the same order. Not a copy that happens to match -- the
  /// point is that `"none"` cannot reorder anything because it does not
  /// touch the list.
  it("returns the wire list verbatim when not grouping", () => {
    const findings = [finding({ finding: "first" }), finding({ finding: "second" })];
    const groups = groupFindings(report({ findings }), "none");
    expect(groups).toHaveLength(1);
    expect(groups[0].findings).toBe(findings);
    expect(groups[0].unknownChecks).toEqual([]);
  });

  /// THE case #1291 names: a `problem` in a file that sorts last
  /// alphabetically must still come before an `advice` in one that sorts
  /// first. Alphabetical grouping would invert these and bury the
  /// problem, which is grouping silently becoming a re-ranking.
  it("orders groups by their worst finding, not alphabetically", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ severity: "problem", subject: claudeMd(`${REPO}/zzz.md`), finding: "the bad one" }),
          finding({ severity: "advice", subject: claudeMd(`${REPO}/aaa.md`), finding: "the quiet one" }),
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
      }),
      "file",
    );
    expect(groups.map((g) => g.label)).toEqual([`${REPO}/zzz.md`, `${REPO}/aaa.md`]);
  });

  /// The same rule at the other end of the rank: `unknown` -- "could not
  /// decide" -- outranks nothing, so a group holding only an Unknown must
  /// not sort below one holding a `problem`, and must not sort ABOVE it
  /// either.
  it("ranks problem above advice above unknown between groups", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ severity: "unknown", subject: claudeMd(`${REPO}/a.md`) }),
          finding({ severity: "advice", subject: claudeMd(`${REPO}/b.md`) }),
          finding({ severity: "problem", subject: claudeMd(`${REPO}/c.md`) }),
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
      }),
      "file",
    );
    expect(groups.map((g) => g.label)).toEqual([
      `${REPO}/c.md`,
      `${REPO}/b.md`,
      `${REPO}/a.md`,
    ]);
  });

  /// Inside a group the backend's order is kept exactly, EVEN WHEN it is
  /// not the order a re-sort would produce. Fed out of rank order -- as a
  /// stable-within-rank backend legitimately can be across two ranks only
  /// by mistake, but the panel must not be the thing that notices -- the
  /// group must still match the wire.
  it("never re-sorts within a group", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ severity: "unknown", finding: "first on the wire" }),
          finding({ severity: "problem", finding: "second on the wire" }),
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
      }),
      "file",
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].findings.map((f) => f.finding)).toEqual([
      "first on the wire",
      "second on the wire",
    ]);
  });

  /// All three `Subject` kinds get a group, and three different labels.
  /// Ungrouped-because-unhandled is the bug: a `Skill` dropped for not
  /// being a CLAUDE.md loses the skills producer's whole output, and a
  /// `Directory` rendered as a file offers a click that opens nothing.
  it("groups every subject kind, labelled as what it is", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ subject: claudeMd(`${REPO}/CLAUDE.md`) }),
          finding({ subject: { kind: "directory", path: `${REPO}/src` } }),
          finding({
            subject: { kind: "skill", path: `${REPO}/.claude/skills/verify/SKILL.md`, name: "verify" },
          }),
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
      }),
      "file",
    );
    expect(groups).toHaveLength(3);
    const byLabel = Object.fromEntries(groups.map((g) => [g.label, g]));
    // A CLAUDE.md is a file the page can open.
    expect(byLabel[`${REPO}/CLAUDE.md`].file).toBe(`${REPO}/CLAUDE.md`);
    // A directory names a place a file is missing from, and carries no
    // file: there is nothing to open.
    expect(byLabel[`${REPO}/src/`].file).toBeNull();
    // A skill is named by its invocation name as well as its path.
    const skill = byLabel[`${REPO}/.claude/skills/verify/SKILL.md (skill: verify)`];
    expect(skill.file).toBe(`${REPO}/.claude/skills/verify/SKILL.md`);

    // Each label LEADS with its path, and `pathLength` measures exactly
    // that prefix -- the panel shortens it against the repository root
    // and prints the rest verbatim. A directory has a path to shorten
    // and no file to open, which is why this is not `file !== null`.
    for (const g of groups) {
      expect(g.pathLength).toBeGreaterThan(0);
      expect(g.label.slice(0, g.pathLength).startsWith(REPO)).toBe(true);
      expect(g.label.slice(0, g.pathLength).endsWith("/")).toBe(false);
    }
  });

  /// #1366: a directory finding whose subject IS the repository reads as
  /// the repository root. Shortened against the root it was `""` plus the
  /// trailing slash, a bare `/` that reads as the filesystem root. The
  /// label carries no path to shorten, so `pathLength` is 0.
  it("labels a directory finding about the repository itself as the repository root", () => {
    for (const path of [REPO, `${REPO}/`]) {
      const groups = groupFindings(
        report({
          findings: [finding({ check: "gaps", subject: { kind: "directory", path } })],
          checks: [{ check: "gaps", run: { state: "ran", findings: 1 } }],
        }),
        "file",
      );
      expect(groups).toHaveLength(1);
      expect(groups[0].label).toBe(REPOSITORY_ROOT);
      expect(groups[0].label).toBe("repository root");
      expect(groups[0].pathLength).toBe(0);
      expect(groups[0].file).toBeNull();
    }
    // A directory elsewhere keeps its path and its slash.
    const [other] = groupFindings(
      report({ findings: [finding({ subject: { kind: "directory", path: `${REPO}-other` } })] }),
      "file",
    );
    expect(other.label).toBe(`${REPO}-other/`);
  });

  /// Three DIFFERENT subjects that happen to share a path are three
  /// groups, not one: the key carries the kind. A path is not an
  /// identity here -- a directory a file is missing from and a file that
  /// was read are different claims, and merging them would put a
  /// "this does not exist yet" finding under a heading that says it does.
  it("keeps every subject kind at one path in its own group", () => {
    const path = `${REPO}/docs`;
    const groups = groupFindings(
      report({
        findings: [
          finding({ subject: claudeMd(path) }),
          finding({ subject: { kind: "directory", path } }),
          finding({ subject: { kind: "skill", path, name: "docs" } }),
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
      }),
      "file",
    );
    expect(groups).toHaveLength(3);
    expect(new Set(groups.map((g) => g.key)).size).toBe(3);
    // And each still holds its own finding rather than three in one.
    for (const g of groups) expect(g.findings).toHaveLength(1);
  });

  /// By check, a group per check that produced findings, labelled the way
  /// the panel labels a check.
  it("groups by check under the check's label", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ check: "shape", severity: "problem" }),
          finding({ check: "imports", severity: "advice" }),
          finding({ check: "shape", severity: "advice" }),
        ],
        checks: [
          { check: "shape", run: { state: "ran", findings: 2 } },
          { check: "imports", run: { state: "ran", findings: 1 } },
        ],
      }),
      "check",
    );
    expect(groups.map((g) => g.label)).toEqual([CHECK_LABEL.shape, CHECK_LABEL.imports]);
    expect(groups[0].findings).toHaveLength(2);
    // A check label holds no path, so the panel prints it as written.
    for (const g of groups) expect(g.pathLength).toBe(0);
  });

  /// #846, in the view organised by check: a check with zero findings
  /// because it COULD NOT RUN gets a group carrying its reason, while a
  /// check that ran and found nothing gets no group at all. Grouping must
  /// not be how an Unknown becomes an absence.
  it("keeps an unknown check visible, with its reason, and distinct from a clean one", () => {
    const groups = groupFindings(
      report({
        findings: [finding({ check: "imports", severity: "problem" })],
        checks: [
          { check: "imports", run: { state: "ran", findings: 1 } },
          // Ran, found nothing. No group: the clean sentence speaks for it.
          { check: "rot", run: { state: "ran", findings: 0 } },
          // Could not run. A group, with the producer's own words.
          { check: "skills", run: { state: "unknown", reason: "the skills directory could not be listed" } },
        ],
      }),
      "check",
    );
    const labels = groups.map((g) => g.label);
    expect(labels).toContain(CHECK_LABEL.skills);
    expect(labels).not.toContain(CHECK_LABEL.rot);
    const skills = groups.find((g) => g.label === CHECK_LABEL.skills);
    expect(skills?.findings).toEqual([]);
    expect(skills?.unknownChecks.map((c) => (c.run.state === "unknown" ? c.run.reason : ""))).toEqual(
      ["the skills directory could not be listed"],
    );
  });

  /// An unknown check must not sort above a group holding a problem, and
  /// must not be pushed below everything either: it ranks with `unknown`,
  /// which is above nothing.
  it("places an unknown check's group below findings but among the results", () => {
    const groups = groupFindings(
      report({
        findings: [
          finding({ check: "shape", severity: "problem" }),
          finding({ check: "rot", severity: "unknown" }),
        ],
        checks: [
          { check: "shape", run: { state: "ran", findings: 1 } },
          { check: "rot", run: { state: "ran", findings: 1 } },
          { check: "gaps", run: { state: "unknown", reason: "nope" } },
        ],
      }),
      "check",
    );
    expect(groups.map((g) => g.label)).toEqual([
      CHECK_LABEL.shape,
      CHECK_LABEL.rot,
      CHECK_LABEL.gaps,
    ]);
  });

  /// The by-file view must not lose an Unknown check either -- it just
  /// does not own one, because a check that could not run names no file.
  /// The panel renders those separately; the grouping must not invent a
  /// file to hang them on.
  it("attaches no unknown check to a file group", () => {
    const groups = groupFindings(
      report({
        findings: [finding()],
        checks: [{ check: "skills", run: { state: "unknown", reason: "nope" } }],
      }),
      "file",
    );
    for (const g of groups) expect(g.unknownChecks).toEqual([]);
  });

  /// Every check has a label, so a variant added to the wire type cannot
  /// group under `undefined`. The `Record` makes this a compile error
  /// too; this pins it at runtime as well.
  it("labels every check", () => {
    const checks = Object.keys(CHECK_LABEL) as ClaudeMdAdviceCheck[];
    expect(checks.length).toBeGreaterThan(0);
    for (const c of checks) expect(typeof CHECK_LABEL[c]).toBe("string");
  });

  /// A Note is an observation, not advice (#1339). In every arrangement
  /// it leaves the advice groups and lands in one trailing Observations
  /// group, so a count never sits among the things to change -- and a
  /// check whose only output is Notes does not get an advice group.
  it.each(["none", "check", "file"] as const)(
    "moves notes into one trailing Observations group under %s",
    (grouping) => {
      const advice = finding({ check: "transcripts", finding: "advice" });
      const note = finding({ check: "transcripts", severity: "note", finding: "a count" });
      const unknown = finding({ check: "rot", severity: "unknown", finding: "could not" });
      const groups = groupFindings(report({ findings: [advice, unknown, note] }), grouping);
      const last = groups[groups.length - 1];
      expect(last.key).toBe(OBSERVATIONS_KEY);
      expect(last.label).toBe("Observations");
      expect(last.findings).toEqual([note]);
      for (const g of groups.slice(0, -1)) expect(g.findings).not.toContain(note);
      expect(groups.flatMap((g) => g.findings)).toHaveLength(3);
    },
  );

  it("adds no Observations group when there are no notes", () => {
    for (const grouping of ["none", "check", "file"] as const) {
      const groups = groupFindings(report({ findings: [finding()] }), grouping);
      expect(groups.map((g) => g.key)).not.toContain(OBSERVATIONS_KEY);
    }
  });

  /// A report of only Notes has no advice at all, so the flat
  /// arrangement must not render an empty unlabelled advice group above
  /// the observations.
  it("drops the empty flat group when every finding is a note", () => {
    const note = finding({ severity: "note" });
    const groups = groupFindings(report({ findings: [note] }), "none");
    expect(groups.map((g) => g.key)).toEqual([OBSERVATIONS_KEY]);
  });

  /// ...but a check that could not run keeps its by-check group even
  /// when every finding is a note: dropping it would hide an Unknown.
  it("keeps an unknown check's group when every finding is a note", () => {
    const note = finding({ severity: "note" });
    const groups = groupFindings(
      report({
        findings: [note],
        checks: [{ check: "rot", run: { state: "unknown", reason: "no listing" } }],
      }),
      "check",
    );
    expect(groups.map((g) => g.key)).toEqual(["check:rot", OBSERVATIONS_KEY]);
  });
});
