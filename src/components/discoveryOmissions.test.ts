import { describe, expect, it } from "vitest";
import { groupOmissions, projectOf } from "./discoveryOmissions";

const DEPTH = "Discovery depth limit reached; add this folder as a scan root";
const UNPROVED = "Candidate skipped: build-output ownership could not be proved";

describe("projectOf", () => {
  it("is the first folder below the scan folder", () => {
    expect(projectOf("/fixture/code/alpha/a/b/c", ["/fixture/code"])).toBe("/fixture/code/alpha");
  });
  it("ignores a trailing separator on the scan folder", () => {
    expect(projectOf("/fixture/code/alpha/a", ["/fixture/code/"])).toBe("/fixture/code/alpha");
  });
  it("does not treat a sibling with a shared prefix as inside", () => {
    expect(projectOf("/fixture/code-old/alpha/a", ["/fixture/code"])).toBe("/fixture/code-old/alpha/a");
  });
  it("prefers the deepest scan folder that holds the path", () => {
    expect(projectOf("/fixture/code/alpha/beta/x", ["/fixture/code", "/fixture/code/alpha"])).toBe(
      "/fixture/code/alpha/beta",
    );
  });
  it("handles Windows separators", () => {
    expect(projectOf("C:\\fixture\\code\\alpha\\x", ["C:\\fixture\\code"])).toBe("C:\\fixture\\code\\alpha");
  });
  it("falls back to the path itself when no scan folder holds it", () => {
    expect(projectOf("/elsewhere/x", ["/fixture/code"])).toBe("/elsewhere/x");
  });
});

describe("groupOmissions", () => {
  it("groups by reason, then project, largest first, and loses nothing", () => {
    const groups = groupOmissions(
      [
        { root: "/fixture/code/alpha/a", why: DEPTH },
        { root: "/fixture/code/beta/a", why: DEPTH },
        { root: "/fixture/code/beta/b", why: DEPTH },
        { root: "/fixture/code/alpha/bin", why: UNPROVED },
      ],
      ["/fixture/code"],
    );
    expect(groups.map((g) => [g.why, g.count])).toEqual([
      [DEPTH, 3],
      [UNPROVED, 1],
    ]);
    expect(groups[0].projects).toEqual([
      { project: "/fixture/code/beta", paths: ["/fixture/code/beta/a", "/fixture/code/beta/b"] },
      { project: "/fixture/code/alpha", paths: ["/fixture/code/alpha/a"] },
    ]);
  });
});
