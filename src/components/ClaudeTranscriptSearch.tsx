/// Content search over the transcript corpus (#1203, epic #1121).
///
/// The app has always held 1,482 transcripts and 0.83 GB of text and
/// indexed only metadata, so "find the session where I was debugging the
/// FSEvents thing" was a question it could not answer about data it
/// already had. This is the surface that answers it.
///
/// # This component's whole job is the empty state
///
/// Everything else here is a list. The part that is the ticket is what
/// this renders when nothing matched, because there are TWO reasons for
/// that and they must not produce the same sentence:
///
/// | backend verdict | what this renders |
/// |---|---|
/// | `matches` | the hits |
/// | `none` | "No matches" — licensed only because the WHOLE corpus was searched |
/// | `none_yet` | "No matches in the 340 of 1,482 sessions indexed so far" |
///
/// The third row is the feature. 6.0 removed that conflation from four
/// other surfaces (#846, #1042, #1044, #1152), and a search box is the
/// place a user is least likely to question an empty result: an empty
/// list reads as a settled fact about their history, not as a statement
/// about our bookkeeping.
///
/// The backend makes this hard to get wrong -- `verdict` is a tagged
/// union, so there is no possibly-empty array to render carelessly --
/// but the WORDING lives here, and `ClaudeTranscriptSearch.test.tsx`
/// asserts on the distinct strings rather than on which branch ran.
///
/// # Staleness is a fact about the index, not about the session
///
/// When coverage is short, the line says the index has not reached those
/// sessions yet. It deliberately does not say the sessions are empty or
/// missing: that would be a claim about the user's data, and it would be
/// false. This is a claim about ours, and it is true.
import { useState } from "react";

import { useClaudeIndexCoverage, useClaudeTranscriptSearch } from "../api/hooks";
import { errorMessage } from "./QueryError";

/// How much of the corpus is searchable, in words.
///
/// Rendered above the box as well as inside an empty result, because the
/// honest moment to say "this index is still building" is BEFORE someone
/// types, not after they have read an empty list and drawn a conclusion.
function CoverageLine({
  indexed,
  total,
  unreadable,
  truncated,
  lastIndexedAt,
}: {
  indexed: number;
  total: number;
  unreadable: string[];
  truncated: number;
  lastIndexedAt: string | null;
}) {
  // No pass has run at all. Not "0 of 0 searchable", which would read as
  // a complete index of an empty corpus -- the most reassuring possible
  // way to report that nothing has happened yet.
  if (total === 0) {
    return (
      <p className="text-[11px] text-[#8b949e]">
        {lastIndexedAt === null
          ? "The transcript index has not run yet, so nothing is searchable so far."
          : "The size of the transcript corpus is not known yet, so how much is searchable cannot be stated."}
      </p>
    );
  }

  const complete = indexed >= total && unreadable.length === 0;
  return (
    <p className="text-[11px] text-[#8b949e]">
      {complete ? (
        <>All {total.toLocaleString()} sessions are searchable.</>
      ) : (
        <>
          {indexed.toLocaleString()} of {total.toLocaleString()} sessions indexed so far. A
          search covers those; the rest have not been indexed yet.
        </>
      )}
      {unreadable.length > 0 ? (
        <>
          {" "}
          {unreadable.length.toLocaleString()} transcript
          {unreadable.length === 1 ? "" : "s"} could not be read and will not be searched.
        </>
      ) : null}
      {truncated > 0 ? (
        <>
          {" "}
          Only the first 8 MB of {truncated.toLocaleString()} transcript
          {truncated === 1 ? " was" : "s were"} indexed.
        </>
      ) : null}
    </p>
  );
}

export function ClaudeTranscriptSearch({ enabled = true }: { enabled?: boolean }) {
  // Hooks first and unconditionally: this component has an early return
  // below for the disabled case, and a hook under it would be a
  // different hook order on the render after the feature is switched on.
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const coverage = useClaudeIndexCoverage(enabled);
  const search = useClaudeTranscriptSearch(query, enabled);

  if (!enabled) return null;

  const cov = search.data?.coverage ?? coverage.data;

  return (
    <section className="flex flex-col gap-2">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          setQuery(draft);
        }}
        className="flex gap-2"
      >
        <input
          type="search"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="Search transcript content"
          aria-label="Search transcript content"
          className="flex-1 rounded-md border border-[#30363d] bg-[#0d1117] px-2 py-1 text-xs text-[#c9d1d9]"
        />
        <button
          type="submit"
          className="tap-target rounded-md border border-[#30363d] px-3 py-1 text-xs text-[#c9d1d9]"
        >
          Search
        </button>
      </form>

      {/* Coverage BEFORE anything is typed. A search box that says
          nothing about its own readiness invites the first empty result
          to be read as settled. */}
      {cov !== undefined ? (
        <CoverageLine
          indexed={cov.indexed}
          total={cov.total}
          unreadable={cov.unreadable}
          truncated={cov.truncated}
          lastIndexedAt={cov.last_indexed_at}
        />
      ) : coverage.isError ? (
        <p role="status" className="text-[11px] text-[#d29922]">
          How much of the corpus is searchable could not be read (
          {errorMessage(coverage.error)}).
        </p>
      ) : null}

      {/* A failed search is an ERROR, never an empty list. #846's exact
          shape: a rejected read rendered as "nothing found". */}
      {search.isError ? (
        <p
          role="status"
          className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2 text-xs text-[#d29922]"
        >
          The transcript search could not run ({errorMessage(search.error)}). Nothing was
          searched.
        </p>
      ) : null}

      {search.isPending && query.trim() !== "" ? (
        <p className="text-[11px] text-[#8b949e]">Searching…</p>
      ) : null}

      {search.data !== undefined ? (
        <SearchResult answer={search.data} />
      ) : null}
    </section>
  );
}

/// The three verdicts, each with its own sentence.
function SearchResult({
  answer,
}: {
  answer: NonNullable<ReturnType<typeof useClaudeTranscriptSearch>["data"]>;
}) {
  const { verdict } = answer;

  // Nothing was asked, so there is nothing to report about matches. The
  // coverage line above the box already says how much is searchable,
  // which is the honest thing to show before anyone types.
  if (verdict.kind === "not_asked") return null;

  if (verdict.kind === "matches") {
    return (
      <ul className="flex flex-col gap-1">
        {verdict.hits.map((h) => (
          <li
            key={h.session_id}
            className="rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#c9d1d9]"
          >
            <span className="font-mono text-[11px] text-[#8b949e]">{h.session_id}</span>
            <p className="mt-1">{h.snippet}</p>
            {/* A hit is a hit either way, but a truncated session's
                CONTENT is only partly indexed, so the row says so rather
                than implying the whole transcript was searched. */}
            {h.truncated ? (
              <p className="mt-1 text-[11px] text-[#8b949e]">
                Only the first 8 MB of this transcript was indexed.
              </p>
            ) : null}
          </li>
        ))}
      </ul>
    );
  }

  // ---- THE BRANCH THIS FEATURE IS ABOUT ----
  //
  // `none_yet` must never render the `none` sentence. The two strings
  // are deliberately not a template with a conditional clause: they are
  // separate sentences making separate claims, and a shared template is
  // how the qualifier gets dropped in a later edit.
  if (verdict.kind === "none_yet") {
    return (
      <p
        role="status"
        className="rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#c9d1d9]"
      >
        No matches in the {verdict.indexed.toLocaleString()} of{" "}
        {verdict.total.toLocaleString()} sessions indexed so far. The remaining{" "}
        {(verdict.total - verdict.indexed).toLocaleString()} have not been indexed yet and
        were not searched.
      </p>
    );
  }

  return (
    <p
      role="status"
      className="rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#c9d1d9]"
    >
      No matches. All {answer.coverage.total.toLocaleString()} sessions were searched.
    </p>
  );
}
