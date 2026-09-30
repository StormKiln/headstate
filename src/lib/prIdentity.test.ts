import { describe, expect, it } from "vitest";
import { prIdentity, prKey } from "./prIdentity";
import type { PrIdentity } from "../types/identity";

describe("PR identity", () => {
  it("keeps legacy GitHub payloads equivalent to explicit GitHub.com identity", () => {
    const legacy = JSON.parse('{"repo":"group/project","number":7}') as PrIdentity;
    expect(prKey(legacy)).toBe(prKey({ ...legacy, source: { provider: "github", host: "github.com" } }));
    expect(prIdentity(legacy)).toEqual(legacy);
  });

  it("distinguishes provider, host, full project path and project-local IID", () => {
    const base: PrIdentity = { source: { provider: "gitlab", host: "gitlab.com" }, repo: "group/project", number: 7 };
    const identities: PrIdentity[] = [
      base,
      { ...base, source: { provider: "github", host: "gitlab.com" } },
      { ...base, source: { provider: "gitlab", host: "gitlab.example" } },
      { ...base, repo: "group/subgroup/project" },
      { ...base, number: 8 },
    ];
    expect(new Set(identities.map(prKey)).size).toBe(identities.length);
    expect(prIdentity(base)).toEqual(base);
  });

  it("does not confuse field boundaries with delimiters inside names", () => {
    expect(prKey({ source: { provider: "gitlab", host: "a/b" }, repo: "c", number: 1 }))
      .not.toBe(prKey({ source: { provider: "gitlab", host: "a" }, repo: "b/c", number: 1 }));
  });
});
