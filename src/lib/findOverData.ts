/// Match highlighting computed from the DATA, not the DOM (#1200).
///
/// # Why this is not `Cmd-F`
///
/// The session list draws [`RENDER_CAP`] rows and says so. The match
/// COUNT beside the search box is already complete -- `useMatchedSessions`
/// filters the whole array -- but a reader could not see WHERE in a row
/// the match landed, and native find-in-page cannot help: it only sees
/// painted text, so on a capped list it silently misses the rows past the
/// cap. A user searches, the browser says "not found", and the row is
/// right there in the data.
///
/// Computing the segments from the same strings the filter matched on
/// keeps the highlight and the count answering the same question. It also
/// means this keeps working unchanged when the list is virtualized
/// (#1200's second half): a row that is not painted is still counted, and
/// is highlighted the moment it scrolls in.
///
/// # The consistency guard
///
/// A highlight is computed per FIELD and never spans two of them.
/// Concatenated row text can otherwise produce a visual match that the
/// counter never counted -- searching `"mainsrc"` against a row whose
/// branch is `main` and whose cwd starts `src/` would highlight across
/// the boundary while the count says zero. A count that disagrees with
/// the highlights is worse than either alone, so the seam is not
/// searchable.

/// One run of text, and whether the query matched it.
export interface Segment {
  text: string;
  hit: boolean;
}

/// Split `value` into alternating non-matching and matching runs.
///
/// Matching is case-insensitive and literal -- the query is a string a
/// user typed, never a pattern. A regex built from it would make `.` and
/// `*` mean something the user did not ask for, and `(` throw.
///
/// An empty or whitespace-only query returns the whole value as one
/// non-matching segment rather than `[]`, so a caller can render the
/// result unconditionally and never has to special-case "no search".
export function segments(value: string, query: string): Segment[] {
  const q = query.trim().toLowerCase();
  if (q === "" || value === "") return value === "" ? [] : [{ text: value, hit: false }];

  const hay = value.toLowerCase();
  const out: Segment[] = [];
  let at = 0;
  for (;;) {
    const found = hay.indexOf(q, at);
    if (found === -1) break;
    if (found > at) out.push({ text: value.slice(at, found), hit: false });
    // Sliced from `value`, NOT from `hay`: the lowercased copy is for
    // FINDING only. Rendering it would quietly downcase the user's own
    // branch names and paths.
    out.push({ text: value.slice(found, found + q.length), hit: true });
    at = found + q.length;
  }
  if (at < value.length) out.push({ text: value.slice(at), hit: false });
  return out;
}

/// Whether `value` contains the query at all.
///
/// Shares `segments`' trimming and case rules ON PURPOSE. Two functions
/// deciding "does this match" with two copies of the rule is how a
/// highlight and a count come to disagree, which is the failure the
/// module header describes.
export function matches(value: string | null | undefined, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "" || !value) return false;
  return value.toLowerCase().includes(q);
}
