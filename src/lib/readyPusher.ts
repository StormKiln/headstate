import type { PullRequest, RowPusher } from "@/types/pr";

/// Who pushed a Ready for review row's head, as far as the strip knows
/// (#1576).
///
/// The difference between the last two states is the point:
/// - `viewer` / `other`: the activity log named the pusher of THIS head.
/// - `pending`: NOT CHECKED -- the answer has not arrived, the lookup was
///   declined (REST budget, the per-refresh cap, no head repository), or
///   the viewer's own login has not loaded. "We did not ask" (#1050).
/// - `unknown`: asked, and could not tell -- the log lags or names another
///   commit, the request failed, or the viewer's login could not be read.
///
/// Neither `pending` nor `unknown` is a verdict: nothing is hidden or
/// tagged on either.
export type Pusher =
  | { state: "viewer"; login: string }
  | { state: "other"; login: string }
  | { state: "pending" }
  | { state: "unknown" };

/// What the base's rulesets say about approving the last push.
///
/// `not-required` means no RULESET asks; classic branch protection is
/// invisible to the read, so it is not "nothing asks" -- but the strip
/// never hides on classic protection either, so it decides the row.
/// `unread` covers declined and unreadable alike: a rule that could not be
/// read is not "no rule".
export type LastPushRule = "required" | "not-required" | "unread";

export interface ReadyPusher {
  pusher: Pusher;
  rule: LastPushRule;
}

/// The viewer's login: a string once read, `undefined` while it is
/// loading, `null` when it could not be read.
export type ViewerLogin = string | null | undefined;

const NOT_CHECKED: ReadyPusher = { pusher: { state: "pending" }, rule: "unread" };

/// One row's pusher and rule from the strip's answers.
///
/// An answer about a DIFFERENT head commit than the row now shows is
/// dropped: the pusher of the old head says nothing about the new one.
export function readyPusher(
  pr: Pick<PullRequest, "head_oid">,
  answer: RowPusher | undefined,
  viewer: ViewerLogin,
): ReadyPusher {
  if (!answer || answer.head_oid !== pr.head_oid) return NOT_CHECKED;
  const rule: LastPushRule =
    answer.rules.state !== "read"
      ? "unread"
      : answer.rules.require_last_push_approval
        ? "required"
        : "not-required";
  const p = answer.last_pusher;
  let pusher: Pusher;
  if (p.state === "known") {
    if (viewer === undefined) pusher = { state: "pending" };
    else if (viewer === null) pusher = { state: "unknown" };
    else pusher = { state: p.login === viewer ? "viewer" : "other", login: p.login };
  } else if (p.state === "unknown") {
    pusher = { state: "unknown" };
  } else {
    // `declined` and `not_needed`: nothing was asked.
    pusher = { state: "pending" };
  }
  return { pusher, rule };
}

/// The viewer pushed last AND the rules were read as requiring someone
/// else's approval of the last push: the viewer's approval cannot count.
export function approvalWontCount(r: ReadyPusher): boolean {
  return r.pusher.state === "viewer" && r.rule === "required";
}

/// The strip's choice for rows the viewer pushed last (#1576). Persisted
/// per view like `readySort`.
///
/// - `auto` (the default): hide a row only when the base's rules were
///   READ as requiring someone else's approval of the last push AND the
///   pusher is KNOWN to be the viewer. Nothing else is hidden
///   automatically.
/// - `hide`: hide every row KNOWN to be the viewer's push.
/// - `show`: hide nothing; the tag still marks them.
export type MyPushesMode = "auto" | "hide" | "show";

/// What one row does under a mode. An undecided row is SHOWN, and
/// counted -- never hidden on a guess.
type Fate = "shown" | "hidden" | "not-checked" | "unknown";

function fate({ pusher, rule }: ReadyPusher, mode: MyPushesMode): Fate {
  if (mode === "show") return "shown";
  if (pusher.state === "other") return "shown";
  const undecided: Fate = pusher.state === "unknown" ? "unknown" : "not-checked";
  if (mode === "hide") return pusher.state === "viewer" ? "hidden" : undecided;
  // auto. A base read as NOT requiring it never hides, so the row is
  // decided whoever pushed.
  if (rule === "not-required") return "shown";
  if (pusher.state === "viewer") {
    // Viewer's push under unread rules: the rule is what is undecided.
    if (rule === "required") return "hidden";
    return "unknown";
  }
  return undecided;
}

export interface ReadyPartition<T> {
  shown: T[];
  hidden: number;
  /// Rows the filter might have hidden but has not checked.
  notChecked: number;
  /// Rows the filter might have hidden but could not decide.
  unknown: number;
}

/// Split the strip's rows under a mode. Pure, so every rule above is
/// tested without rendering.
export function partitionReady<T>(
  rows: T[],
  of: (row: T) => ReadyPusher,
  mode: MyPushesMode,
): ReadyPartition<T> {
  const out: ReadyPartition<T> = { shown: [], hidden: 0, notChecked: 0, unknown: 0 };
  for (const row of rows) {
    const f = fate(of(row), mode);
    if (f === "hidden") {
      out.hidden += 1;
      continue;
    }
    if (f === "not-checked") out.notChecked += 1;
    if (f === "unknown") out.unknown += 1;
    out.shown.push(row);
  }
  return out;
}

/// The filter's status line, or null when there is nothing to say.
///
/// Counts, never a bare "some": the reader should know how much of the
/// strip they are not seeing, and how much the filter could not decide.
export function partitionSummary(
  part: Pick<ReadyPartition<unknown>, "hidden" | "notChecked" | "unknown">,
  mode: MyPushesMode,
): string | null {
  if (mode === "show") return null;
  const parts: string[] = [];
  if (part.hidden > 0) {
    parts.push(
      mode === "auto"
        ? `${part.hidden} hidden: you pushed last and your approval can't count`
        : `${part.hidden} hidden: you pushed last`,
    );
  }
  if (part.notChecked > 0) parts.push(`${part.notChecked} not checked yet`);
  if (part.unknown > 0) parts.push(`${part.unknown} could not be decided`);
  return parts.length > 0 ? parts.join(" · ") : null;
}
