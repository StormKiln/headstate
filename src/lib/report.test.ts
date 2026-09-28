import { describe, expect, it } from "vitest";
// `?raw`, not `node:fs`: the project carries no `@types/node`.
import bugForm from "../../.github/ISSUE_TEMPLATE/bug_report.yml?raw";
import type { DiagnosticBundle } from "../types/report";
import {
  INSTALL_OPTIONS,
  MAX_URL,
  buildSections,
  composeFields,
  describeCounts,
  issueUrl,
  reportTitle,
  scrubCounted,
  type Environment,
  type ReportContext,
  type ReportSection,
} from "./report";

const bundle = (over: Partial<DiagnosticBundle> = {}): DiagnosticBundle => ({
  appVersion: "7.9.2",
  os: "macos",
  arch: "aarch64",
  osVersion: "macOS 15.6",
  install: { method: "macOS DMG", basis: "a release build running from an application bundle" },
  ghVersion: "gh version 2.80.0 (2026-01-01)",
  ghNote: null,
  poll: {
    operation: "the open pull request search (GraphQL)",
    fetchTimeoutSecs: 30,
    tickTimeoutSecs: 45,
    focusedIntervalSecs: 60,
    lastWaitSecs: 300,
    ticksRecorded: 25,
    recent: [
      {
        atUnixMs: 1,
        ok: true,
        fetchMs: 2100,
        error: null,
        timedOutAfterSecs: null,
        attempt: 0,
        reviewing: "ok",
      },
      {
        atUnixMs: 2,
        ok: false,
        fetchMs: 30004,
        error: "GitHub request timed out after 30s",
        timedOutAfterSecs: 30,
        attempt: 3,
        reviewing: "skipped",
      },
    ],
    failuresInRecent: 1,
    lastSuccessSecsAgo: 720,
  },
  graphqlRemaining: 4512,
  restRemaining: 4999,
  diagnosticsOn: true,
  logTail: "[diag] poll tick start\n[diag] poll tick fetch done 30004ms err: timed out",
  logNote: null,
  ...over,
});

const ready = (b = bundle()): Environment => ({
  appVersion: "7.9.2",
  gathered: { kind: "ready", bundle: b },
  mobile: false,
});

const ctx: ReportContext = { error: "GitHub request timed out after 30s", view: "My PRs" };

const sectionsOf = (c: ReportContext = ctx, env: Environment = ready()) =>
  buildSections(c, env).sections;

const text = (sections: ReportSection[], id: string) =>
  sections.find((s) => s.id === id)?.text ?? "";

/// The query string, decoded, as a map.
const params = (url: string) => new URL(url).searchParams;

describe("issueUrl fills the bug FORM", () => {
  /// #1575's first cause: the link sent `body=`, which an issue form
  /// ignores, so every field of the form opened empty.
  it("names the template and fills fields by their form id", () => {
    const { url } = issueUrl(reportTitle(ctx.error), composeFields(sectionsOf(), new Set()));
    const p = params(url);
    expect(p.get("template")).toBe("bug_report.yml");
    expect(p.get("body")).toBeNull();
    expect(p.get("what-happened")).toContain("timed out after 30s");
    expect(p.get("version")).toBe("7.9.2");
    expect(p.get("install")).toBe("macOS DMG");
    expect(p.get("os")).toBe("macOS 15.6 aarch64");
    expect(p.get("gh-version")).toBe("gh version 2.80.0 (2026-01-01)");
    expect(p.get("log")).toContain("poll tick start");
    expect(p.get("steps")).toBeTruthy();
    expect(p.get("title")).toBe("Error: GitHub request timed out after 30s");
  });

  /// Every id the link uses must exist in the form, or it fills nothing.
  it("uses only ids the form declares", () => {
    const { url } = issueUrl("t", composeFields(sectionsOf(), new Set()));
    for (const key of params(url).keys()) {
      if (key === "template" || key === "title") continue;
      expect(bugForm).toContain(`id: ${key}`);
    }
  });

  /// A dropdown selects only on an exact option.
  it("offers install answers that are exactly the form's options", () => {
    for (const option of INSTALL_OPTIONS) {
      expect(bugForm).toContain(`        - ${option}\n`);
    }
    const declared = bugForm.split("id: install")[1].split("validations")[0];
    expect(declared.match(/^ {8}- /gm)?.length).toBe(INSTALL_OPTIONS.length);
  });

  /// #1575's second cause: the pre-transfer owner.
  it("points at the canonical repository owner", () => {
    const { url } = issueUrl("t", {});
    expect(url.startsWith("https://github.com/StormKiln/headstate/issues/new?")).toBe(true);
  });

  /// An install the app could not identify is not forced into the
  /// dropdown; it is written out where the maintainer will read it.
  it("keeps an unrecognised install out of the dropdown but in the report", () => {
    const b = bundle({ install: { method: null, basis: "nothing identified the package" } });
    const p = params(issueUrl("t", composeFields(sectionsOf(ctx, ready(b)), new Set())).url);
    expect(p.get("install")).toBeNull();
    expect(p.get("what-happened")).toContain("unknown (nothing identified the package)");
  });
});

describe("the report carries the detail a timeout needs", () => {
  it("names the operation, limits, elapsed time, attempt, history and interval", () => {
    const poll = text(sectionsOf(), "poll");
    expect(poll).toContain("the open pull request search (GraphQL)");
    expect(poll).toContain("30s per request, 45s per refresh");
    expect(poll).toContain("timed out after 30.0s (limit 30s), attempt 3 in a row");
    expect(poll).toContain("1 of the last 2 failed (25 since launch)");
    expect(poll).toContain("Last successful refresh: 12 min ago");
    expect(poll).toContain("60s when focused; last wait 300s");
  });

  it("states the budgets remaining", () => {
    const budget = text(sectionsOf(), "budget");
    expect(budget).toContain("GraphQL: 4,512 points left this hour");
    expect(budget).toContain("REST: 4,999 requests left this hour");
  });

  /// The desktop's own answer fills in when the caller could not say.
  it("takes diagnostics from the desktop when the caller does not know", () => {
    expect(text(sectionsOf({ error: "x" }), "where")).toContain("Diagnostic logging: on");
  });
});

describe("unknowns are stated, never blank and never zero", () => {
  /// Absent is not zero: every figure that could not be read says so.
  it("says unknown for everything the desktop could not supply", () => {
    const env: Environment = { appVersion: null, gathered: { kind: "unavailable" }, mobile: false };
    const s = sectionsOf({ error: "boom" }, env);
    for (const id of ["version", "install", "os", "gh", "log", "poll", "budget"]) {
      expect(text(s, id), id).toContain("unknown");
    }
    expect(text(s, "where")).toContain("View: unknown");
    expect(text(s, "where")).toContain("Diagnostic logging: unknown");
    // And no section is empty.
    for (const sec of s) expect(sec.text.trim(), sec.id).not.toBe("");
  });

  /// Pending and unknown are different states.
  it("distinguishes still-gathering from could-not-read", () => {
    const pending = sectionsOf(ctx, { appVersion: null, gathered: { kind: "pending" }, mobile: false });
    const failed = sectionsOf(ctx, { appVersion: null, gathered: { kind: "unavailable" }, mobile: false });
    expect(text(pending, "os")).toContain("still being gathered");
    expect(text(failed, "os")).toContain("could not be read");
  });

  it("says unknown for null figures inside a bundle", () => {
    const b = bundle({
      graphqlRemaining: null,
      restRemaining: null,
      ghVersion: null,
      ghNote: "gh was not found",
      osVersion: null,
      logTail: null,
      logNote: "diagnostic logging is off",
      diagnosticsOn: false,
      poll: { ...bundle().poll, focusedIntervalSecs: null, lastWaitSecs: null, recent: [], ticksRecorded: 0, failuresInRecent: 0, lastSuccessSecsAgo: null },
    });
    const s = sectionsOf({ error: "x" }, ready(b));
    expect(text(s, "budget")).toContain("GraphQL: unknown");
    expect(text(s, "budget")).not.toMatch(/\b0 points/);
    expect(text(s, "gh")).toBe("unknown (gh was not found)");
    expect(text(s, "os")).toContain("version unknown");
    expect(text(s, "log")).toContain("diagnostic logging is off");
    expect(text(s, "poll")).toContain("Last successful refresh: unknown (no refresh has finished yet)");
    expect(text(s, "poll")).toContain("Refresh interval: unknown; last wait unknown");
  });
});

describe("scrubbing", () => {
  it("removes tokens, paths and repository names, and counts them", () => {
    const { sections, removed } = buildSections(
      {
        error: "bad credentials ghp_abc123DEF456ghi789jkl for acme/widget at /Users/alice",
      },
      ready(),
    );
    const err = text(sections, "error");
    expect(err).not.toContain("ghp_abc123DEF456ghi789jkl");
    expect(err).not.toContain("acme/widget");
    expect(err).not.toContain("alice");
    expect(removed).toEqual({ tokens: 1, paths: 1, repos: 1 });
    expect(describeCounts(removed)).toBe("1 token, 1 file path and 1 repository name");
  });

  it("scrubs the log tail and the component stack like every other field", () => {
    const b = bundle({ logTail: "query for acme/widget failed" });
    const s = sectionsOf({ error: "x", componentStack: "at Foo (/Users/alice/Components.tsx:12)" }, ready(b));
    expect(text(s, "log")).not.toContain("acme/widget");
    expect(text(s, "where")).not.toContain("alice");
    expect(text(s, "where")).toContain("[path]");
  });

  /// The user's own edits go through the same scrub on the way out.
  it("still applies to text the user edited", () => {
    const s = sectionsOf().map((sec) =>
      sec.id === "steps" ? { ...sec, text: "opened acme/widget with ghp_zzzzzzzzzzzzzzzzzzzz in /home/bob" } : sec,
    );
    const steps = params(issueUrl("t", composeFields(s, new Set())).url).get("steps") ?? "";
    expect(steps).not.toContain("acme/widget");
    expect(steps).not.toContain("ghp_zzzzzzzzzzzzzzzzzzzz");
    expect(steps).not.toContain("bob");
    expect(steps).toContain("[repo]");
  });

  it("scrubs the title", () => {
    expect(reportTitle("failed for acme/widget")).toBe("Error: failed for [repo]");
    expect(scrubCounted("[path] [repo] [redacted]").removed).toEqual({ tokens: 0, paths: 0, repos: 0 });
  });

  it("bounds the error and keeps the top of a deep stack", () => {
    const deep = Array.from({ length: 500 }, (_, i) => `    at Component${i}`).join("\n");
    const s = sectionsOf({ error: "x".repeat(5000), componentStack: deep });
    expect(text(s, "error").length).toBeLessThan(700);
    expect(text(s, "where")).toContain("at Component0");
    expect(text(s, "where")).not.toContain("at Component499");
  });
});

describe("redaction by the user", () => {
  /// A section the user leaves out does not reach the URL at all.
  it("removes left-out sections from the URL", () => {
    const s = sectionsOf();
    const p = params(issueUrl("t", composeFields(s, new Set(["log", "budget", "gh"]))).url);
    expect(p.get("log")).toBeNull();
    expect(p.get("gh-version")).toBeNull();
    expect(p.get("what-happened")).not.toContain("GraphQL:");
    // What was kept is still there.
    expect(p.get("what-happened")).toContain("timed out after 30s");
    expect(p.get("version")).toBe("7.9.2");
  });

  it("drops the whole field when every section behind it is left out", () => {
    const all = new Set(["error", "where", "poll", "budget", "install"]);
    expect(params(issueUrl("t", composeFields(sectionsOf(), all)).url).get("what-happened")).toBeNull();
  });
});

describe("the URL fits GitHub's limit", () => {
  it("trims the log from its oldest lines and says so", () => {
    const log = Array.from({ length: 400 }, (_, i) => `[diag] line ${i} ${"y".repeat(40)}`).join("\n");
    const { url, trimmed } = issueUrl("t", composeFields(sectionsOf(ctx, ready(bundle({ logTail: log }))), new Set()));
    expect(url.length).toBeLessThanOrEqual(MAX_URL);
    expect(trimmed).toMatch(/log was shortened to its last \d+ of 400 lines/);
    const kept = params(url).get("log") ?? "";
    expect(kept).toContain("line 399");
    expect(kept).not.toContain("line 0 ");
  });

  it("leaves a short report untouched", () => {
    expect(issueUrl("t", composeFields(sectionsOf(), new Set())).trimmed).toBeNull();
  });
});

describe("on the phone", () => {
  it("says the environment is the paired desktop's", () => {
    const where = text(sectionsOf(ctx, { ...ready(), appVersion: "0.9.0", mobile: true }), "where");
    expect(where).toContain("phone companion 0.9.0");
    expect(where).toContain("paired desktop's");
  });
});
