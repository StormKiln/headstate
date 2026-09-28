import { describe, expect, it } from "vitest";
// `?raw`, not `node:fs`: the project carries no `@types/node`.
import releaseNotesRs from "../../src-tauri/src/release_notes.rs?raw";
import cargoToml from "../../src-tauri/Cargo.toml?raw";
import { LATEST_RELEASE_URL, NEW_ISSUE_URL, REPO_SLUG, REPO_URL } from "./repo";

/// The repository moved owners and four links kept the old one (#1575).
describe("the repository's address", () => {
  it("is the canonical owner", () => {
    expect(REPO_SLUG).toBe("StormKiln/headstate");
    expect(NEW_ISSUE_URL).toBe("https://github.com/StormKiln/headstate/issues/new");
    expect(LATEST_RELEASE_URL).toBe("https://github.com/StormKiln/headstate/releases/latest");
  });

  /// The Rust side names the repository for the release-notes menu, and
  /// `release_notes.rs` asserts the update check agrees with it; this
  /// ties the frontend to the same slug, so either side changing alone
  /// fails.
  it("agrees with the Rust constant and the crate manifest", () => {
    expect(releaseNotesRs).toContain(`pub const REPO: &str = "${REPO_SLUG}";`);
    expect(cargoToml).toContain(`repository = "${REPO_URL}"`);
  });
});
