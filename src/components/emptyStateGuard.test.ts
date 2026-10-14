import { describe, expect, it } from "vitest";

/// Every component that renders an empty state also renders a FAILED one.
///
/// # What this enforces
///
/// A component under `src/components` (or `src/App.tsx`) that reads a
/// query and renders "nothing found" copy must also consult `isError` --
/// or `error`, or a query object's `.isError` -- so that a rejected query
/// does not render as an answer.
///
/// # The finding it would have caught (#846, #854)
///
/// `QueryError`'s own doc comment is the canonical statement of the rule,
/// and it is exact: *"A rejected query left `data` at its `[]` default,
/// and the empty-list copy then told the user 'no pull requests match
/// these filters' -- a confident, wrong answer to a question the app
/// could not actually answer. An error has to look like an error."*
///
/// #846 fixed four surfaces BY HAND -- `ArtifactsPage`, `ClaudeMdPage`,
/// `VenvSection`, `WorktreeSidebar`/`ArtifactSidebar` -- and the fifth
/// and sixth were already in the tree when it shipped:
///
/// - **`RepoPickerSidebar`** consumes `useWorktrees`, the very hook #846
///   fixed `WorktreeSidebar` for, and was missed because it is a second
///   consumer of it. Its copy makes it the worst of the set: "No
///   repositories found in the scanned folders" is a DIAGNOSIS pointing
///   at the user's settings, so a failed scan sent someone to fix a
///   configuration that was never wrong. Its own comment states the rule
///   it broke -- *"'We have not looked yet' and 'we looked and there is
///   nothing' are opposite answers"* -- having been given only two of the
///   three arms.
/// - **`CleanupLog`** could not comply however it was written: the gap
///   was in `useCleanupLog`, which destructured `isError` from `useQuery`
///   and returned only `{ entries, isLoading, run }`. #852 had already
///   visited this component to add the `isLoading` arm for this exact
///   bug class and still left the third out.
///
/// # Why a source scan, and why `import.meta.glob`
///
/// The property is about every surface rather than about one, so a render
/// test per component proves the mechanism and says nothing about the
/// seventh component somebody adds -- which is the defect both times.
///
/// `import.meta.glob` rather than the explicit `?raw` imports the four
/// existing source-scanning tests use (`surfaceGuard.test.ts`,
/// `mirroredConstants.test.ts`, `App.lazy.test.tsx`,
/// `worktrees.test.ts`), because a list of imports is exactly the
/// enumeration that let the fifth and sixth surfaces exist. A glob covers
/// a file nobody remembered to add. It was verified to work under this
/// suite's `vmThreads` pool before being relied on -- that pool runs each
/// file in its own V8 context, so it was not safe to assume.
///
/// # What it cannot see, stated rather than glossed
///
/// - **Whether the error arm is REACHABLE.** With `data = []` the empty
///   branch is reached first, so an error arm placed after it never
///   renders in the case it exists for. That ordering is the heart of
///   #846's fix and this guard cannot check it; the per-component tests
///   and `ClaudeMdPage`'s comment carry it.
/// - **Whether the error arm is any GOOD.** `isError` mentioned and
///   ignored passes.
/// - **A failure a component cannot observe.** Several hooks in
///   `src/api/connection.ts` and `phoneNotify.ts` return a plain value
///   rather than a query object, so their callers have nothing to
///   destructure. Those are listed in `NO_QUERY_OF_ITS_OWN` below.
/// - **An empty state this does not recognise as one.** The detector
///   below matches a small set of copy patterns. A component that phrases
///   emptiness some other way is invisible to it -- the usual limit of a
///   text scan, and the reason the self-checks at the bottom assert that
///   the detector still finds the surfaces we know about.
const sources = import.meta.glob("./**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const appSources = import.meta.glob("../App.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/// Production components only.
///
/// `import.meta.glob` has no negative pattern, so test files are filtered
/// here. They are full of empty-state copy as ASSERTIONS -- a test
/// checking that "No images." appears is not a surface rendering it --
/// and judging them by production rules is the false-positive class that
/// gets a guard disabled. `SystemHealthPage.test.tsx` was the first thing
/// this scan reported.
const files: Record<string, string> = Object.fromEntries(
  Object.entries({ ...sources, ...appSources }).filter(
    ([path]) => !path.includes(".test."),
  ),
);

/// Components with no query of their own to fail.
///
/// An explicit, reviewed list with a reason each, which is the form this
/// codebase insists on over a looser pattern -- `check-privacy.sh:120`
/// records ~40 false positives from one unanchored pattern as the reason
/// every pattern in it is anchored, and `surfaceGuard.test.ts`'
/// `DESKTOP_ONLY_WRAPPERS` makes the argument that adding to such a list
/// should be a deliberate act somebody reads.
///
/// Every entry is a component whose "empty" copy is not a query result:
/// it is a prop, local state, or a value a hook cannot report a failure
/// for.
const NO_QUERY_OF_ITS_OWN: Record<string, string> = {
  "./ConnectionBanner.tsx":
    "renders a ConnectionState from `useConnection`, which returns a plain value; " +
    "an unreachable desktop IS its error state and is already shown as one",
  "./PhoneNotifyPanel.tsx":
    "`usePhoneNotify` returns `boolean | null`, with null meaning unknown; " +
    "it has no query object for a caller to destructure",
  "./ui/dialog.tsx": "a primitive with no data of its own",
};

/// Whether a file renders "we looked and there is nothing" copy.
///
/// Two conditions, BOTH required, because either alone was measured noisy
/// on this tree:
///
///  1. Copy that reads as an empty state. Keyed off the small set of
///     phrasings this codebase uses, not off `.length === 0` -- which
///     matches every list component including those whose empty case is
///     simply "render no rows".
///  2. That copy sits in a conditional on the DATA. Without this,
///     `FilterBar`'s "No labels" (a filter-dropdown facet) and
///     `PairingRequestModal`'s "No post-quantum signing key offered" (a
///     description of what a phone sent) both read as empty states. They
///     are not: neither is a claim about a query that came back empty.
function rendersEmptyState(raw: string): boolean {
  const src = code(raw);
  // `[A-Za-z]`, not `[a-z]`: `ClaudeMdPage`'s copy is "No CLAUDE.md files
  // in this repository", and a lowercase-only class missed the one
  // surface whose proper noun follows the "No".
  const copy = [
    />\s*No\s+[A-Za-z]/, // ">No pull requests…", ">No CLAUDE.md files…"
    /"\s*No\s+[A-Za-z]/, // in a string prop
    />\s*Nothing\s/,
    />\s*No\s+\{[\w.]+\}\s+(found|yet|paired|listed)/, // dynamic category labels
    /No\s+\w+\s+(found|yet|paired|listed)/,
  ].some((re) => re.test(src));
  if (!copy) return false;
  // A guard on emptiness itself, rather than on a field's value.
  return /\.length\s*===\s*0/.test(src) || /\.length\s*<\s*1/.test(src) || /!\w+\?\./.test(src);
}

/// A file's CODE, with comments removed.
///
/// Load-bearing, and the reason is the sharpest lesson of writing these
/// guards: this codebase documents its rules at length directly above the
/// code implementing them, so the identifier a guard greps for appears in
/// prose far more often than in an expression. Without this strip,
/// `RepoPickerSidebar` satisfied `consultsFailure` on the strength of a
/// COMMENT saying "`isError` and `refetch` since #854" -- the guard
/// accepted the explanation in place of the behaviour.
///
/// That was found only by reverting the fix and watching the guard NOT
/// fail, which is the whole argument for doing it: a guard nobody has
/// watched fail is a guard that might be checking nothing
/// (`check-symlinks.test.py`).
///
/// A regex cannot lex JavaScript, so a `//` inside a string literal or a
/// regex literal takes the rest of that line with it. The consequence is
/// seeing LESS code, which can only cause a false POSITIVE somebody
/// reads -- never a silent pass.
function code(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, "") // block and JSX-expression comments
    .replace(/^\s*\/\/.*$/gm, "") // whole-line `//` and `///`
    .replace(/\/\/.*$/gm, ""); // trailing `//`
}

/// Whether a file consults a query's failure, in any of the spellings
/// this codebase uses.
///
/// `q.isError` matters as much as destructured `isError`: ten components
/// keep the whole query object (`const seriesQ = useStatsSeries(...)`,
/// then `seriesQ.isError`) and are entirely correct. A guard that
/// demanded destructuring would report ten false positives on working
/// code, which is how this check would have been turned off rather than
/// obeyed.
function consultsFailure(raw: string): boolean {
  const src = code(raw);
  return (
    /\bisError\b/.test(src) ||
    /\.isError\b/.test(src) ||
    /\berror:\s*\w*[Ee]rror\b/.test(src) ||
    /\bloadError\b/.test(src) ||
    // `const { data, isPending, error } = useFoo()` -- `error` on its own
    // inside a destructuring of a query result.
    /\{[^}]*\berror\b[^}]*\}\s*=\s*use[A-Z]/.test(src)
  );
}

/// Whether a file reads a query OF ITS OWN.
///
/// This is the distinction the guard got wrong first, and it is the
/// important one. A `use[A-Z]` heuristic matches every hook in the
/// codebase -- `useFilters` (a Zustand store), `useIsMobile`,
/// `usePairingRequest` (a Tauri event queue, not a fetch), `useMemo` --
/// and on this tree it produced SEVEN findings, every one of them a false
/// positive. A guard that cries wolf is a guard someone disables.
///
/// The false positives fell into one instructive class: a PRESENTATIONAL
/// component that takes its rows as a prop. `PrList`, `RepoTable`,
/// `UpdateWizard` and `FilterBar` all render empty-state copy about data
/// they were handed, and every one of their parents already consults
/// `isError` and renders `QueryError` in a sibling arm -- `App.tsx:756`,
/// `StatsPage.tsx:461`, `PackagesPage.tsx:90`. Asking those components
/// for an error arm would be asking them to break the layering that makes
/// them correct.
///
/// So the test is whether the file imports from the query layer and binds
/// a result in a shape that HAS a failure to report. A prop is not a
/// query, and `useState` is not a query.
function readsAQueryOfItsOwn(raw: string): boolean {
  const src = code(raw);
  // Both import spellings. The api layer is reached as `@/api/hooks` in
  // most files and as `../api/hooks` in others -- `WorktreeSidebar`, one
  // of the four surfaces #846 fixed, uses the relative form, so matching
  // only the alias excused the very file the guard exists for.
  const fromQueryLayer =
    /from "@tanstack\/react-query"/.test(src) ||
    /from "(@\/|\.\.\/|\.\/)api\/hooks"/.test(src);
  if (!fromQueryLayer) return false;
  // And it must actually bind a result: a destructuring with `data`, or a
  // `useQuery` call, or a query object it later reads `.isError` off.
  return (
    /\buseQuery\b/.test(src) ||
    /\{[^}]*\bdata\b[^}]*\}\s*=\s*use[A-Z]/.test(src) ||
    /\{[^}]*\bentries\b[^}]*\}\s*=\s*use[A-Z]/.test(src) ||
    /=\s*use[A-Z]\w*\([^)]*\);[\s\S]{0,800}?\.\s*is(Error|Loading|Pending)\b/.test(src)
  );
}

describe("the guard's own detectors", () => {
  /// Guards the guard, in the one direction that makes it vacuous: a glob
  /// that matched nothing, or detectors that matched nothing, would leave
  /// every assertion below trivially true. That is the failure mode this
  /// whole issue is about, so it is asserted rather than assumed.
  it("sees the component tree", () => {
    expect(Object.keys(files).length).toBeGreaterThan(40);
    expect(files["../App.tsx"], "App.tsx must be in the glob").toBeTruthy();
    expect(files["./RepoPickerSidebar.tsx"]).toBeTruthy();
  });

  it("recognises the empty states of the surfaces #846 and #854 fixed", () => {
    for (const f of [
      "./RepoPickerSidebar.tsx",
      "./CleanupLog.tsx",
      "./ArtifactsPage.tsx",
      "./ClaudeMdPage.tsx",
    ]) {
      expect(rendersEmptyState(files[f]), `${f} must be seen to render an empty state`).toBe(true);
    }
  });

  it("does not see an empty state where there is none", () => {
    expect(rendersEmptyState("const a = 1; return <p>Hello</p>;")).toBe(false);
    // Empty-state copy with no conditional on emptiness: a filter facet
    // or a description of a payload, which is what `FilterBar` and
    // `PairingRequestModal` render.
    expect(rendersEmptyState("<div>No labels</div>")).toBe(false);
    expect(rendersEmptyState('x ? "a" : "No post-quantum key offered"')).toBe(false);
  });

  /// The prop-vs-query distinction, which is the one the first version of
  /// this guard got wrong -- and wrong SEVEN times out of seven.
  it("does not ask a presentational component for an error arm", () => {
    // Takes its rows as a prop; its parent owns the query verdict. This
    // is `PrList`, `RepoTable`, `UpdateWizard` and `FilterBar`.
    expect(readsAQueryOfItsOwn('import { useFilters } from "@/store/filters";')).toBe(false);
    // A Zustand store and a media query are not queries.
    expect(readsAQueryOfItsOwn("const f = useFilters();\nconst m = useIsMobile();")).toBe(false);
    // And a real one is.
    expect(
      readsAQueryOfItsOwn('import { useWorktrees } from "@/api/hooks";\nconst { data } = useWorktrees();'),
    ).toBe(true);
  });

  /// And the four surfaces #846 fixed, plus the two #854 did, must each
  /// still be SEEN as query-reading -- or the check above would excuse
  /// them as presentational and the guard would be vacuous on exactly the
  /// files it exists for.
  it("sees the six fixed surfaces as query-reading", () => {
    for (const f of [
      "./RepoPickerSidebar.tsx",
      "./CleanupLog.tsx",
      "./ArtifactsPage.tsx",
      "./ClaudeMdPage.tsx",
      "./VenvSection.tsx",
      "./WorktreeSidebar.tsx",
    ]) {
      expect(readsAQueryOfItsOwn(files[f]), `${f} must be seen to read a query`).toBe(true);
    }
  });

  it("accepts both the destructured and the whole-object spellings", () => {
    expect(consultsFailure("const { data, isError } = useFoo();")).toBe(true);
    expect(consultsFailure("const q = useFoo();\nif (q.isError) return null;")).toBe(true);
    expect(consultsFailure("const { data, isPending, error } = useFoo();")).toBe(true);
    expect(consultsFailure("const { data } = useFoo();")).toBe(false);
  });

  /// And every exemption must still name a file that exists and still
  /// renders an empty state, so the list cannot quietly outlive what it
  /// excuses. A stale entry is how an allowlist stops describing the
  /// code.
  it("has no stale exemptions", () => {
    for (const [file, why] of Object.entries(NO_QUERY_OF_ITS_OWN)) {
      expect(files[file], `NO_QUERY_OF_ITS_OWN names ${file} (${why}), which does not exist`)
        .toBeTruthy();
    }
  });
});

describe("every surface that can render 'nothing found'", () => {
  it("also consults whether the query FAILED", () => {
    const offenders: string[] = [];
    let checked = 0;
    for (const [file, src] of Object.entries(files)) {
      if (file in NO_QUERY_OF_ITS_OWN) continue;
      if (!readsAQueryOfItsOwn(src) || !rendersEmptyState(src)) continue;
      checked += 1;
      if (!consultsFailure(src)) offenders.push(file);
    }
    // The scan found something to judge, so a renamed directory or a
    // changed idiom fails loudly rather than passing over an empty set.
    expect(checked, "the scan is broken, not the components").toBeGreaterThan(8);
    expect(
      offenders,
      `these components render an empty state and never consult whether the query failed:\n  ${offenders.join(
        "\n  ",
      )}\n\nA rejected query leaves \`data\` at its \`[]\` default, so the empty-state copy ` +
        `answers a question the app could not answer -- and where that copy is a DIAGNOSIS ` +
        `("No repositories found in the scanned folders") it sends the user to fix something ` +
        `that is not broken. Destructure \`isError\` and \`refetch\` and render the failure ` +
        `arm BEFORE the empty one, as #846's four surfaces do; \`QueryError\` is the shared ` +
        `component. If the value cannot fail, add the file to NO_QUERY_OF_ITS_OWN with a ` +
        `reason (#854).`,
    ).toEqual([]);
  });
});

it("recognises a dynamic category label in a qualified empty state", () => {
 expect(rendersEmptyState('if (rows.length === 0) return <p>No {label} found in the completed portion.</p>')).toBe(true);
});
