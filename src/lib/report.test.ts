import { describe, expect, it } from "vitest";
import { buildReport, issueUrl } from "./report";

const ctx = {
  version: "3.1.3",
  platform: "macos",
  arch: "aarch64",
  error: "Serde Error: expected value at line 1 column 1",
};

/// The banner stated a problem and offered nothing. These errors are
/// exactly the ones a user cannot diagnose, so the report has to carry
/// the surrounding facts -- both diagnoses in this area needed platform,
/// build kind, and how the app was launched, none of which are in the
/// error string.
///
/// Scrubbing is ALLOW-LIST shaped, not deny-list: the report is built
/// from fields known to be safe rather than by removing dangerous parts
/// of an arbitrary string. A deny-list is one unanticipated error format
/// away from leaking.
describe("buildReport", () => {
  it("carries the facts a maintainer needs", () => {
    const r = buildReport(ctx);
    expect(r).toContain("3.1.3");
    expect(r).toContain("macos");
    expect(r).toContain("aarch64");
    expect(r).toContain("expected value at line 1 column 1");
  });

  // A token in the error string must never survive, in any form.
  it("removes a token that appears in the error", () => {
    const r = buildReport({ ...ctx, error: "bad credentials for ghp_abc123DEF456ghi789jkl" });
    expect(r).not.toContain("ghp_abc123DEF456ghi789jkl");
    expect(r).toContain("[redacted]");
  });

  it("removes every token shape gh can produce", () => {
    for (const t of ["ghp_aaaaaaaaaaaaaaaaaaaa", "gho_bbbbbbbbbbbbbbbbbbbb", "github_pat_ccccccccccccccccccccc"]) {
      expect(buildReport({ ...ctx, error: `failed with ${t}` })).not.toContain(t);
    }
  });

  // A home directory carries a username, and a checkout path can name a
  // private project -- the exact leak the privacy guard was extended for.
  // A single-segment home path, which the repository rule cannot match
  // -- so this isolates the PATH rule. With a deeper path both rules
  // fire and removing either one still passes, which is how the first
  // version of this test was vacuous.
  it("removes a home directory, and with it the username", () => {
    const r = buildReport({ ...ctx, error: "could not read /Users/alice" });
    expect(r).not.toContain("alice");
    expect(r).toContain("[path]");
  });

  it("removes a deeper checkout path naming a project", () => {
    const r = buildReport({ ...ctx, error: "could not find gh in /Users/alice/code/acme/widget" });
    expect(r).not.toContain("alice");
    expect(r).not.toContain("acme/widget");
  });

  // The poll log deliberately records counts only, never repo names. A
  // report must not undo that.
  it("removes anything shaped like a repository", () => {
    const r = buildReport({ ...ctx, error: "query failed for privatecorp/secret-service" });
    expect(r).not.toContain("privatecorp/secret-service");
  });

  // The whole point: an unfamiliar error format must not leak by
  // default. Anything not recognised as safe is dropped, not passed
  // through.
  it("truncates an error long enough to hide something in", () => {
    const r = buildReport({ ...ctx, error: "x".repeat(5000) });
    expect(r.length).toBeLessThan(2000);
  });
});

describe("issueUrl", () => {
  it("targets this repository's issue form", () => {
    expect(issueUrl("body")).toContain("pktstorm/headstate/issues/new");
  });

  it("carries the report as a prefilled body", () => {
    const url = issueUrl("hello world");
    expect(url).toContain(encodeURIComponent("hello world"));
  });

  // The user must see and edit it before it is posted, so this opens a
  // prefilled form rather than submitting anything.
  it("does not submit -- it opens a form", () => {
    expect(issueUrl("x")).toContain("issues/new?");
  });
});

/// The context added in #1148: view, diagnostics and component stack.
describe("buildReport carries what the user cannot describe", () => {
  const base = { version: "1.0", platform: "macos", arch: "arm64", error: "boom" };

  it("includes the component stack under its own heading", () => {
    // The single most valuable thing in a crash report, and until now
    // it existed only in a console nobody has open on a release build.
    const body = buildReport({ ...base, componentStack: "\n    at DockerPage\n    at App" });
    expect(body).toContain("### Where");
    expect(body).toContain("at DockerPage");
  });

  it("omits the stack section entirely when there is none", () => {
    // An empty code block reads as "no stack was captured", which is a
    // claim. Omitting the section says nothing, which is the truth.
    const body = buildReport(base);
    expect(body).not.toContain("### Where");
  });

  it("scrubs the component stack like every other field", () => {
    // A stack carries file paths in a dev build, and the scrub table is
    // the second line of defence precisely for fields written by code
    // this module does not control.
    const body = buildReport({
      ...base,
      componentStack: "at Foo (/Users/octocat/Components.tsx:12)",
    });
    expect(body).not.toContain("/Users/acme");
    expect(body).toContain("[path]");
  });

  it("bounds a very deep stack", () => {
    // A GitHub URL has a practical length limit, past which the form
    // opens truncated or not at all -- worse than a short stack.
    const deep = Array.from({ length: 500 }, (_, i) => `    at Component${i}`).join("\n");
    const body = buildReport({ ...base, componentStack: deep });
    expect(body.length).toBeLessThan(6000);
    // And it keeps the TOP frames, which are the useful ones.
    expect(body).toContain("at Component0");
    expect(body).not.toContain("at Component499");
  });

  it("names the view when it is known", () => {
    expect(buildReport({ ...base, view: "Docker images" })).toContain("On the Docker images view");
  });

  it("says nothing about the view when it is not known", () => {
    // A boot failure has no view to name, and guessing one would be a
    // claim about where the user was.
    expect(buildReport(base)).not.toContain("On the");
  });

  it("distinguishes diagnostics off from diagnostics unknown", () => {
    // THREE states, not two (#1042). "Could not read the setting" is
    // not "it was off", and a maintainer's first reply differs: one is
    // "please turn this on", the other means a log already exists.
    expect(buildReport({ ...base, diagnostics: true })).toContain("Diagnostic logging was on");
    expect(buildReport({ ...base, diagnostics: false })).toContain("Diagnostic logging was off");
    expect(buildReport(base)).not.toContain("Diagnostic logging");
  });
});
