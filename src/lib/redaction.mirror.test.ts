import { describe, expect, it } from "vitest";
// `?raw` rather than `node:fs`, for the reason mirroredConstants.test.ts:3
// gives: the project carries no `@types/node`, so a filesystem read would
// fail `yarn tsc -b`.
import redactRs from "../../src-tauri/src/redact.rs?raw";
import { SCRUB_PATTERNS } from "./report";

/// The redaction patterns exist twice -- once in TypeScript for the
/// issue-report body, once in Rust for the diagnostic log -- and this
/// asserts they still agree.
///
/// #1122 is why both copies exist. The TS table was written first and
/// guarded the report; the log was written unredacted for its whole
/// life while Settings and the help topic both claimed otherwise. The
/// fix put the same patterns on the Rust side, which creates exactly the
/// drift hazard #850 documents: two copies whose comments claim
/// agreement and neither of which reads the other.
///
/// The direction that matters, per mirroredConstants.test.ts:45, is that
/// changing EITHER side alone fails.
describe("redaction patterns are mirrored in Rust", () => {
  /// The Rust regex literals, in source order.
  ///
  /// Extracted from `Regex::new(r"...")` rather than from a table of
  /// expected spellings, so this reads the value Rust actually compiles.
  /// Both raw-string forms: `r"..."` and `r#"..."#`. The path pattern
  /// needs the hashed form because it contains a literal `"` in its
  /// character class, and an extractor that read only the bare form
  /// would silently find one pattern instead of two -- which is what
  /// the count assertion below exists to catch.
  const rustPatterns = [
    ...redactRs.matchAll(/Regex::new\(r#"(.+?)"#\)/g),
    ...redactRs.matchAll(/Regex::new\(r"([^"]+)"\)/g),
  ].map((m) => m[1]);

  it("finds the Rust patterns at all", () => {
    // A guard on the extractor itself. If `redact.rs` is restructured so
    // these stop matching, this test must fail loudly rather than
    // silently compare two empty lists and pass.
    expect(
      rustPatterns.length,
      "redact.rs must define its patterns as Regex::new(r\"...\") literals",
    ).toBeGreaterThan(0);
  });

  it("mirrors the token pattern", () => {
    const ts = SCRUB_PATTERNS[0][0].source;
    expect(rustPatterns).toContain(ts);
  });

  it("carries one Rust pattern per shared TypeScript pattern", () => {
    // The tables are deliberately NOT the same length: TS scrubs
    // repository names out of the issue report, and Rust does not,
    // because the log's audit trail names them on purpose (see
    // redact.rs's module comment). That asymmetry is the thing most
    // likely to be "fixed" by mistake, so it is asserted rather than
    // left to a comment.
    expect(rustPatterns.length).toBe(2);
    expect(SCRUB_PATTERNS.length).toBe(3);
  });

  it("keeps repository names out of the Rust table", () => {
    // If someone adds the repo pattern to redact.rs, the audit trail
    // README advertises silently stops naming repositories. Fail here
    // and make them read why.
    const repoPattern = SCRUB_PATTERNS[2][0].source;
    expect(
      rustPatterns,
      "redact.rs must NOT scrub repository names -- the log's audit trail names them deliberately",
    ).not.toContain(repoPattern);
  });
});
