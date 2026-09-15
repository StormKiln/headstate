import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { stubViewport } from "@/test-utils";

/// The companion renders the browser, and it renders no control it
/// cannot use (#1035).
///
/// # Why this test exists at all
///
/// `src/api/surfaceGuard.test.ts` is explicit about its own scope: it
/// reads `Class::Local` rows out of `surface.rs` by regex and checks that
/// no typed wrapper reaches one unguarded. Both browser commands are
/// `Class::Read`, so that test passes without asserting ANYTHING about
/// them -- it checks classification, not rendering. Its own instructions
/// say the rest: "do not just add it here. First make sure whatever calls
/// it is behind `IS_MOBILE_BUILD` … so the phone never renders a control
/// that can only reject."
///
/// #603, #604 and #606 are three instances of that failure, independently,
/// in the same release. So the property is pinned here instead: every
/// control this page renders on a phone is one the phone can actually
/// use. If a `Class::Local` control is ever added -- a reveal-in-Finder
/// on a file row is the obvious candidate, and `claude_reveal_path` is
/// already `Local` for exactly the stated reason -- this is the test that
/// must be extended to prove it is hidden.
///
/// # The viewport is stubbed, and that is not incidental
///
/// A test that mocks `IS_MOBILE_BUILD` and leaves `matchMedia` alone runs
/// at DESKTOP width while claiming to be a phone: `useIsMobile()` answers
/// "render the phone layout" and is true for the mobile build OR a narrow
/// viewport, and jsdom's default window is neither. So the two are set
/// together, which is the only configuration that is actually a phone.

vi.mock("@/lib/target", () => ({
  IS_MOBILE_BUILD: true,
  IS_DESKTOP_BUILD: false,
}));

const state = vi.hoisted(() => ({
  repoFile: undefined as string | undefined,
}));

vi.mock("../api/hooks", () => ({
  useWorktrees: () => ({
    data: [{ path: "/code/app", name: "app" }],
    isError: false,
  }),
  useRepoTree: () => ({
    data: {
      path: "",
      entries: [
        { name: "src", path: "src", dir: true, symlink: false },
        { name: "README.md", path: "README.md", dir: false, symlink: false },
      ],
    },
    isLoading: false,
    isError: false,
    error: undefined,
    refetch: vi.fn(),
  }),
  useRepoFile: () => ({
    data: {
      path: "README.md",
      size: 12,
      content: "# hello\n",
      truncated: false,
      binary: false,
    },
    isLoading: false,
    isError: false,
    error: undefined,
    refetch: vi.fn(),
  }),
}));

vi.mock("@/store/filters", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useActiveFilters: () => ({ repo: "/code/app" }),
  useFilters: () => ({
    repoPath: "",
    setRepoPath: vi.fn(),
    repoFile: state.repoFile,
    setRepoFile: vi.fn(),
  }),
}));

const { RepositoriesPage } = await import("./RepositoriesPage");

beforeEach(() => {
  // A real phone width, paired with the build flag above.
  stubViewport(390);
  state.repoFile = undefined;
});

afterEach(() => {
  stubViewport(null);
});

describe("the repository browser on the companion", () => {
  /// The view is NOT in `MOBILE_HIDDEN_VIEWS`, and this is what that
  /// decision means in practice. The set is "a statement about what the
  /// COMPANION cannot do, not about screen width", and the companion can
  /// do this: both commands are `Class::Read`, and all three limits --
  /// the containment guard, the re-derived root and the 256 KB window --
  /// live inside them, so the phone inherits every one.
  it("lists a repository's files", () => {
    render(<RepositoriesPage />);
    expect(screen.getByText("src")).toBeTruthy();
    expect(screen.getByText("README.md")).toBeTruthy();
  });

  it("reads a file", () => {
    state.repoFile = "README.md";
    render(<RepositoriesPage />);
    expect(screen.getByText(/# hello/)).toBeTruthy();
  });

  /// The property this file exists for, asserted over the WHOLE rendered
  /// tree rather than over a list of control names: a phone must render
  /// no control that can only reject.
  ///
  /// `reveal` / `Finder` / `open in` are the shapes a `Class::Local`
  /// control takes here -- `claude_reveal_path` and `reveal_log` are both
  /// `Local` because "the phone has no Finder to reveal into". Asserted
  /// as absence over the accessible names of every button, so a control
  /// added later without thinking about the phone fails here rather than
  /// shipping.
  it("renders no desktop-only control on either panel", () => {
    const check = () => {
      for (const b of screen.queryAllByRole("button")) {
        const label = (b.textContent ?? "").toLowerCase();
        expect(
          /reveal|finder|show in folder|open in/.test(label),
          `"${b.textContent}" looks like a Class::Local control, which the ` +
            `phone can only ever be refused for. Put it behind IS_MOBILE_BUILD ` +
            `and give it its own render test (#1035).`,
        ).toBe(false);
      }
    };
    const listing = render(<RepositoriesPage />);
    check();
    listing.unmount();
    state.repoFile = "README.md";
    render(<RepositoriesPage />);
    check();
  });
});
