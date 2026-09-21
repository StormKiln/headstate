import type { ClaudePrLink } from "@/types/pr";

/// One repository's pull requests from a single session (#1280).
///
/// A named group rather than a `Record<string, ClaudePrLink[]>`, because
/// the ORDER of the groups is part of the answer and an object's key
/// iteration order is not something a reader should have to reason
/// about. V8 iterates string keys in insertion order, which would make
/// the group order depend on the order the backend happened to send the
/// links in -- stable enough to pass a test and wrong the moment a
/// session opens its second repo's PR first.
export interface PrGroup {
  repo: string;
  prs: ClaudePrLink[];
}

/// A session's pull requests, grouped by repository (#1280).
///
/// # Why grouped at all
///
/// A long-running session touches several repositories and the flat list
/// arrived in whatever order the link table yielded, so finding one PR in
/// it was a scan. Grouping answers the question the list is actually
/// asked -- "what did this session produce, and where" -- in one read.
///
/// # Ascending within a group, on purpose
///
/// Not newest-first, which is this app's default everywhere else. Within
/// ONE session the numbers were issued in roughly the order the work
/// happened, so ascending reads as the sequence of that session's work.
/// Descending would present the same list back-to-front against the
/// transcript beside it.
///
/// # The group order is alphabetical, and it is chosen rather than left
///
/// Repository name, case-insensitively, with the raw name as the
/// tiebreak so two repos differing only in case still have one settled
/// order. The alternative -- ordering groups by the smallest PR number,
/// or by first appearance -- would make the list re-order itself as new
/// PRs land, which is the thing that made the flat list hard to read.
/// Alphabetical is stable under any addition.
///
/// `localeCompare` is deliberately NOT used: it is locale-dependent, so
/// the same session would group differently for two users, and the
/// comparison here is over `owner/repo` slugs that are ASCII by GitHub's
/// own rules.
export function groupPrsByRepo(prs: readonly ClaudePrLink[]): PrGroup[] {
  const byRepo = new Map<string, ClaudePrLink[]>();
  for (const pr of prs) {
    const bucket = byRepo.get(pr.repo);
    if (bucket) bucket.push(pr);
    else byRepo.set(pr.repo, [pr]);
  }

  const groups: PrGroup[] = [];
  for (const [repo, bucket] of byRepo) {
    groups.push({
      repo,
      // Sorted here rather than by sorting the whole input once and
      // partitioning it: the numbers are only comparable WITHIN a
      // repository, since `acme/api#7` and `acme/ui#7` are two different
      // pull requests that happen to share an integer.
      prs: [...bucket].sort((a, b) => a.number - b.number),
    });
  }

  groups.sort((a, b) => {
    const la = a.repo.toLowerCase();
    const lb = b.repo.toLowerCase();
    if (la !== lb) return la < lb ? -1 : 1;
    // Two repos whose names differ only in case. Rare to the point of
    // being hypothetical, but a comparator that returns 0 for distinct
    // keys leaves their order to the sort's stability over a Map
    // iteration order -- which is the insertion order this function
    // exists to stop depending on.
    return a.repo < b.repo ? -1 : a.repo > b.repo ? 1 : 0;
  });
  return groups;
}

/// A search query that names a pull request (#1280).
///
/// `repo` is null when the query gave a number but no repository --
/// `#1234` or `1234`. The lookup is keyed on `(repo, number)`, so the
/// repository has to be resolved before it can run; `reposForNumber`
/// below is what does that, and it returns a LIST because two
/// repositories can both hold a `#1234`.
export interface PrQuery {
  repo: string | null;
  number: number;
}

/// GitHub's own rule for an `owner/repo` slug: letters, digits, `.`, `-`
/// and `_`, in exactly two segments.
///
/// ANCHORED, and both ends matter. Without the anchors a query like
/// `notarization/step two#3` would parse, and the deeper path a user
/// pastes -- `owner/widgets/Foo.tsx#L12` -- would read as a repository
/// called `widgets/Foo.tsx`. With them, the whole query has to be a
/// reference and nothing else.
///
/// A two-segment relative path -- `owner/thing#5` -- does still parse,
/// and that is accepted rather than defended against. It IS the shape
/// of a repository reference, there is no way to tell it from one
/// without a list of the user's repositories, and the cost of being
/// wrong is one lookup that returns empty -- while the text filter goes
/// on matching the same string against every prompt and path as it
/// always did.
const SLUG = /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/;

/// A GitHub pull request URL, which is the third shape a user pastes.
const PR_URL = /^https?:\/\/(?:www\.)?github\.com\/([^/\s]+\/[^/\s]+)\/pull\/(\d+)(?:[/?#].*)?$/i;

/// Read a search query as a pull request reference, or `null` (#1280).
///
/// # Which shapes trigger the lookup, and why not every number
///
/// Four, and no more:
///
/// | query | reads as |
/// |---|---|
/// | `#1234` | PR 1234 in some repository |
/// | `1234` | the same -- a bare number, which is how people say a PR aloud |
/// | `owner/repo#1234` | PR 1234 in exactly that repository |
/// | `https://github.com/owner/repo/pull/1234` | the same, pasted from the browser |
///
/// Ordinary prose does NOT reach the backend. That is the whole point of
/// parsing rather than sending the raw box: "notarization" and "fix the
/// spinner" are the overwhelming majority of what is typed here, and a
/// lookup per keystroke of those would be a command round trip for every
/// character of a query that could never match a PR.
///
/// The bare-number arm is the one judgement call. It is included because
/// `1234` is what people actually type -- the `#` is a GitHub rendering
/// convention, not how anyone refers to a PR in speech -- and because the
/// cost of a false positive is bounded and visible: the lookup runs,
/// finds no link, and the list still shows every text match for `1234`.
/// Plain-text search is never replaced, only added to.
///
/// A number with any other text attached is prose. `v1234`, `fix 1234`
/// and `1234x` all return `null`: the query has to be a reference and
/// nothing else, so a sentence that happens to contain digits is not a
/// PR lookup.
///
/// Zero and negative numbers are rejected -- GitHub numbers PRs from 1 --
/// as is anything longer than seven digits, which cannot be a real
/// number and is more likely an id someone pasted.
export function parsePrQuery(raw: string): PrQuery | null {
  const q = raw.trim();
  if (q === "") return null;

  const url = PR_URL.exec(q);
  if (url) {
    const repo = url[1].replace(/\.git$/i, "");
    return SLUG.test(repo) ? withNumber(repo, url[2]) : null;
  }

  const hash = q.indexOf("#");
  if (hash > 0) {
    const repo = q.slice(0, hash);
    return SLUG.test(repo) ? withNumber(repo, q.slice(hash + 1)) : null;
  }

  // `#1234` and `1234`. `hash === 0` is the first form; `hash === -1` is
  // the second, and anything else was handled above.
  return withNumber(null, hash === 0 ? q.slice(1) : q);
}

/// The shared tail of every arm above: a numeric string, validated once.
function withNumber(repo: string | null, digits: string): PrQuery | null {
  if (!/^\d{1,7}$/.test(digits)) return null;
  const number = Number(digits);
  // `0` is not a pull request. Stated rather than left to the backend,
  // which would answer an empty list and make "there is no PR 0" read as
  // "no session produced PR 0" -- the collapse this feature is about.
  return number > 0 ? { repo, number } : null;
}

/// Which repository a bare `#1234` means (#1280).
///
/// `claude_sessions_for_pr` is keyed on `(repo, number)` -- the index
/// `schema.rs` builds -- so a number alone cannot be looked up. Rather
/// than add a scanning command, the repository is resolved from the
/// pull request list this app already holds in cache: `usePullRequests`
/// fetches it once with `staleTime: Infinity`, so this costs nothing on
/// the wire and no extra round trip per keystroke.
///
/// # What this deliberately cannot do
///
/// The cached list holds the pull requests Headstate TRACKS. A number
/// belonging to a repository outside it -- or to a PR closed long enough
/// ago to have left the list -- resolves to nothing here, and the caller
/// must then say it could not tell which repository was meant rather
/// than reporting an empty lookup. Those are different facts: "we asked
/// and no session produced it" against "we never got to ask".
///
/// Returns every repository carrying that number, because two repos can
/// both have a `#1234` and picking one would be a guess.
export function reposForNumber(
  prs: readonly { repo: string; number: number }[] | undefined,
  number: number,
): string[] {
  if (!prs) return [];
  const out = new Set<string>();
  for (const pr of prs) if (pr.number === number) out.add(pr.repo);
  return [...out].sort();
}
