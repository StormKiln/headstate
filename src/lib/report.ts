import type { DiagnosticBundle, PollReport } from "../types/report";
import { NEW_ISSUE_URL } from "./repo";

/// A bug report the user can read, edit and cut before it is posted.
///
/// The banner used to state a problem and offer nothing, and the errors
/// that most need reporting are exactly the ones a user cannot diagnose.
/// Then it offered a link that opened the repository's bug FORM with
/// every field empty (#1575): the report went in `body=`, which an issue
/// form ignores, and the form's own fields -- version, install, OS, `gh`,
/// log -- were never named.
///
/// # The form, filled by field id
///
/// `.github/ISSUE_TEMPLATE/bug_report.yml` is an issue form. GitHub
/// documents that a form field's `id` is "the canonical identifier for
/// the field in URL query parameter prefills", so the link carries
/// `template=bug_report.yml` and one parameter per field id. A dropdown
/// is prefilled with the option's TEXT, which GitHub does not document:
/// when the text matches no option the dropdown is simply left unset,
/// so the install method is ALSO written into "What happened" and
/// nothing is lost either way.
///
/// # Sections, each editable and each optional
///
/// The report is a list of sections, each mapped to one form field. The
/// dialog shows every section as it will be sent, lets the user edit it,
/// and lets them leave any of it out. Nothing is posted by the app.

/// What the caller knows that the environment lookups cannot find.
export interface ReportContext {
  error: string;
  /// The view the user was on, if known (#1148). A boot failure has no
  /// view, and the report says "unknown" rather than guessing.
  view?: string;
  /// Whether the verbose `[diag]` log was being written, when the caller
  /// knows. `undefined` is "not known", never "off" (#1042); the
  /// desktop's own answer in the bundle is used when the caller has none.
  diagnostics?: boolean;
  /// A render crash's component stack, if there was one.
  componentStack?: string;
}

/// What the lookups found. `pending` and `unavailable` are different
/// states: one is "still asking", the other "asked, and could not tell".
export type Gathered =
  | { kind: "pending" }
  | { kind: "unavailable" }
  | { kind: "ready"; bundle: DiagnosticBundle };

/// The environment the report describes.
export interface Environment {
  /// This app's own version, from the webview. `null` if unknown.
  appVersion: string | null;
  gathered: Gathered;
  /// Whether this is the phone companion, whose bundle describes the
  /// paired DESKTOP rather than the phone.
  mobile: boolean;
}

/// The bug form's field ids.
export type FieldId = "what-happened" | "steps" | "version" | "install" | "os" | "gh-version" | "log";

export interface ReportSection {
  id: string;
  title: string;
  field: FieldId;
  text: string;
}

/// The bug form's install options, exactly as `bug_report.yml` spells
/// them. `report.test.ts` asserts this matches the form.
export const INSTALL_OPTIONS = [
  "macOS DMG",
  "Linux AppImage",
  "Linux .deb",
  "Windows installer",
  "Built from source",
] as const;

/// The longest error text worth including.
///
/// Deliberately short. A long error is more likely to be carrying
/// something -- a quoted query naming repositories, a stack of paths --
/// and no diagnosis so far has needed more than a sentence of it.
const MAX_ERROR = 500;

/// The longest component stack worth including. The top frames are the
/// useful ones, so this keeps the START.
const MAX_STACK = 2000;

/// The longest URL the link will carry.
///
/// GitHub answers `414 URI Too Long` past a limit it does not publish.
/// The GitHub CLI refuses to open a prefilled URL of 8,192 bytes or more
/// (`ValidURL` in its `pkg/cmd/pr/shared/params.go`); this stays under
/// that with room for the browser.
export const MAX_URL = 8000;

/// Patterns for the things that must never leave the machine.
///
/// This is a second line of defence, NOT the design. The report is built
/// from named fields that are known to be safe; this exists because some
/// of those fields -- the error string, the log -- are written by code
/// we do not control and can quote anything.
/// Exported for `redaction.mirror.test.ts`, which asserts the Rust copy
/// in `src-tauri/src/redact.rs` still agrees with this table.
export const SCRUB_PATTERNS: [RegExp, string][] = [
  // Every token shape gh can hand out. Checked before paths, since a
  // token can appear inside one.
  [/\b(gh[pousr]|github_pat)_[A-Za-z0-9_]+/g, "[redacted]"],
  // A home directory carries a username; a checkout path can name a
  // private project. Both are leaks the privacy guard exists to stop.
  [/(\/Users\/|\/home\/|C:\\Users\\)[^\s"']*/g, "[path]"],
  // A report goes to a PUBLIC issue tracker, which the diagnostic log
  // does not -- the log names repositories on purpose (its PR action
  // lines are an audit trail) and the user chooses who sees it. A
  // report has no such moment, so it scrubs repo names and the Rust
  // table in `src-tauri/src/redact.rs` deliberately does not.
  // `redaction.mirror.test.ts` asserts that asymmetry stays.
  [/\b[A-Za-z0-9][-\w.]*\/[A-Za-z0-9][-\w.]+\b/g, "[repo]"],
];

/// How many of each kind scrubbing removed. Counts, never values.
export interface ScrubCounts {
  tokens: number;
  paths: number;
  repos: number;
}

export const NO_SCRUBS: ScrubCounts = { tokens: 0, paths: 0, repos: 0 };

/// Scrub `text`, counting what each pattern removed.
export function scrubCounted(text: string): { text: string; removed: ScrubCounts } {
  const counts = [0, 0, 0];
  let out = text;
  SCRUB_PATTERNS.forEach(([pattern, with_], i) => {
    out = out.replace(pattern, () => {
      counts[i] += 1;
      return with_;
    });
  });
  return { text: out, removed: { tokens: counts[0], paths: counts[1], repos: counts[2] } };
}

export function scrub(text: string): string {
  return scrubCounted(text).text;
}

export function addCounts(a: ScrubCounts, b: ScrubCounts): ScrubCounts {
  return { tokens: a.tokens + b.tokens, paths: a.paths + b.paths, repos: a.repos + b.repos };
}

/// "2 file paths and 1 repository name", or `null` for nothing.
export function describeCounts(c: ScrubCounts): string | null {
  const parts = [
    [c.tokens, "token", "tokens"],
    [c.paths, "file path", "file paths"],
    [c.repos, "repository name", "repository names"],
  ]
    .filter(([n]) => (n as number) > 0)
    .map(([n, one, many]) => `${n} ${n === 1 ? one : many}`);
  if (parts.length === 0) return null;
  if (parts.length === 1) return parts[0];
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

const UNKNOWN = "unknown";

/// A duration in words, for "12 min ago".
function ago(secs: number): string {
  if (secs < 90) return `${secs}s ago`;
  if (secs < 90 * 60) return `${Math.round(secs / 60)} min ago`;
  return `${(secs / 3600).toFixed(1)} h ago`;
}

function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

/// Why a desktop fact is missing, for every line that needs one.
function missing(g: Gathered): string {
  return g.kind === "pending"
    ? "unknown (still being gathered when this report was opened)"
    : "unknown (could not be read from the desktop)";
}

function pollLines(p: PollReport): string[] {
  const lines = [`- Background refresh: ${p.operation}`];
  lines.push(
    `- Time limits: ${p.fetchTimeoutSecs}s per request, ${p.tickTimeoutSecs}s per refresh`,
  );
  const last = p.recent[p.recent.length - 1];
  if (last === undefined) {
    lines.push("- Last refresh: none has finished since launch");
  } else if (last.ok) {
    lines.push(`- Last refresh: succeeded in ${seconds(last.fetchMs)}`);
  } else {
    const how =
      last.timedOutAfterSecs !== null
        ? `timed out after ${seconds(last.fetchMs)} (limit ${last.timedOutAfterSecs}s)`
        : `failed after ${seconds(last.fetchMs)}: ${last.error ?? UNKNOWN}`;
    lines.push(`- Last refresh: ${how}, attempt ${last.attempt} in a row`);
  }
  if (last !== undefined) {
    lines.push(`- Review queue on that refresh: ${last.reviewing}`);
  }
  // Qualified by what was kept: "3 of the last 20", never "3 failures".
  lines.push(
    p.recent.length === 0
      ? "- Recent refreshes: none recorded yet"
      : `- Recent refreshes: ${p.failuresInRecent} of the last ${p.recent.length} failed` +
          (p.ticksRecorded > p.recent.length ? ` (${p.ticksRecorded} since launch)` : ""),
  );
  lines.push(
    `- Last successful refresh: ${
      p.lastSuccessSecsAgo !== null
        ? ago(p.lastSuccessSecsAgo)
        : p.ticksRecorded > 0
          ? "none since launch"
          : UNKNOWN + " (no refresh has finished yet)"
    }`,
  );
  lines.push(
    `- Refresh interval: ${
      p.focusedIntervalSecs !== null ? `${p.focusedIntervalSecs}s when focused` : UNKNOWN
    }; last wait ${p.lastWaitSecs !== null ? `${p.lastWaitSecs}s` : UNKNOWN}`,
  );
  return lines;
}

function remaining(n: number | null, unit: string): string {
  return n === null
    ? `${UNKNOWN} (no response has reported it yet)`
    : `${n.toLocaleString("en-US")} ${unit} left this hour`;
}

function block(text: string): string {
  return ["```", text.trim(), "```"].join("\n");
}

/// The report as sections, every free-text part scrubbed.
///
/// Returns the scrub counts beside the sections so the dialog can say
/// what was removed without ever holding the removed values.
export function buildSections(
  ctx: ReportContext,
  env: Environment,
): { sections: ReportSection[]; removed: ScrubCounts } {
  let removed = NO_SCRUBS;
  const clean = (t: string) => {
    const r = scrubCounted(t);
    removed = addCounts(removed, r.removed);
    return r.text;
  };
  const g = env.gathered;
  const b = g.kind === "ready" ? g.bundle : null;

  const error = clean(ctx.error).slice(0, MAX_ERROR);
  const diagnostics = ctx.diagnostics ?? b?.diagnosticsOn;

  const where = [
    `- View: ${ctx.view ?? UNKNOWN}`,
    // Three states, not two (#1042).
    `- Diagnostic logging: ${diagnostics === undefined ? UNKNOWN : diagnostics ? "on" : "off"}`,
  ];
  if (env.mobile) {
    where.push(
      `- Reported from the phone companion ${env.appVersion ?? UNKNOWN}; the environment below is the paired desktop's`,
    );
  }
  if (ctx.componentStack) {
    where.push("", "Component stack:", "", block(clean(ctx.componentStack).slice(0, MAX_STACK)));
  }

  const poll = b ? pollLines(b.poll).map(clean) : [`- Background refresh: ${missing(g)}`];
  const budgets = [
    `- GraphQL: ${b ? remaining(b.graphqlRemaining, "points") : missing(g)}`,
    `- REST: ${b ? remaining(b.restRemaining, "requests") : missing(g)}`,
  ];

  // The desktop's version when the bundle has it: on the phone that is
  // the machine whose poll failed, and on the desktop the two agree.
  const version = b?.appVersion ?? (env.mobile ? null : env.appVersion);
  const install = b
    ? b.install.method !== null
      ? b.install.method
      : `${UNKNOWN} (${b.install.basis})`
    : missing(g);
  const os = b
    ? `${b.osVersion ?? `${b.os} (version ${UNKNOWN})`} ${b.arch}`
    : missing(g);
  const gh = b
    ? b.ghVersion !== null
      ? clean(b.ghVersion)
      : `${UNKNOWN} (${clean(b.ghNote ?? "no reason recorded")})`
    : missing(g);
  const log = b
    ? b.logTail !== null
      ? clean(b.logTail)
      : `No log attached: ${clean(b.logNote ?? UNKNOWN)}.`
    : `No log attached: ${missing(g)}.`;

  const sections: ReportSection[] = [
    {
      id: "error",
      title: "Error",
      field: "what-happened",
      text: `Headstate showed this error:\n\n${block(error)}`,
    },
    { id: "where", title: "Where", field: "what-happened", text: where.join("\n") },
    { id: "poll", title: "Background refresh", field: "what-happened", text: poll.join("\n") },
    { id: "budget", title: "Rate limits", field: "what-happened", text: budgets.join("\n") },
    {
      id: "steps",
      title: "How to get there",
      field: "steps",
      text: "Not recorded by Headstate. What were you doing when it appeared, and does it happen every time?",
    },
    { id: "version", title: "Headstate version", field: "version", text: version ?? missing(g) },
    { id: "install", title: "How it was installed", field: "install", text: install },
    { id: "os", title: "OS and architecture", field: "os", text: clean(os) },
    { id: "gh", title: "gh --version", field: "gh-version", text: gh },
    { id: "log", title: "Diagnostic log", field: "log", text: log },
  ];
  return { sections, removed };
}

/// The issue title: the error's first line, scrubbed and bounded.
export function reportTitle(error: string): string {
  const first = scrub(error).split("\n")[0].trim().slice(0, 100);
  return first ? `Error: ${first}` : "Error";
}

/// The form fields for the sections the user kept, each re-scrubbed.
///
/// Re-scrubbed because the user may have EDITED a section, and anything
/// typed there is as capable of carrying a token as an error string.
/// Scrubbing is idempotent on text it already scrubbed.
export function composeFields(
  sections: ReportSection[],
  excluded: ReadonlySet<string>,
): Partial<Record<FieldId, string>> {
  const kept = sections.filter((s) => !excluded.has(s.id));
  const fields: Partial<Record<FieldId, string>> = {};
  const what: string[] = [];
  for (const s of kept) {
    const text = scrub(s.text).trim();
    if (s.field === "what-happened") {
      what.push(s.id === "error" ? text : `**${s.title}**\n\n${text}`);
    } else if (s.field === "install") {
      // A dropdown takes one of its options exactly. Anything else --
      // "unknown", or a user's own words -- goes into the free text so
      // it still reaches the maintainer.
      if ((INSTALL_OPTIONS as readonly string[]).includes(text)) {
        fields.install = text;
      } else if (text) {
        what.push(`**How it was installed**\n\n${text}`);
      }
    } else if (text) {
      fields[s.field] = text;
    }
  }
  if (what.length > 0) fields["what-happened"] = what.join("\n\n");
  return fields;
}

function encode(title: string, fields: Partial<Record<FieldId, string>>): string {
  const params = [
    ["template", "bug_report.yml"],
    ["title", title],
    ...Object.entries(fields),
  ] as [string, string][];
  return `${NEW_ISSUE_URL}?${params
    .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
    .join("&")}`;
}

/// The prefilled form's URL, and what had to be cut to fit.
///
/// Opens a form rather than submitting: the user is the only one who can
/// confirm nothing sensitive survived scrubbing, and filing publicly on
/// someone's behalf is not something an app should do. It also avoids
/// needing issue-write scope on a token this app only reads with.
///
/// Over [`MAX_URL`], the LOG is trimmed first, from its oldest lines --
/// the newest are the ones about the failure -- then "What happened" from
/// its end. `trimmed` says which, so the dialog can tell the user.
export function issueUrl(
  title: string,
  fields: Partial<Record<FieldId, string>>,
): { url: string; trimmed: string | null } {
  const f = { ...fields };
  let url = encode(title, f);
  if (url.length <= MAX_URL) return { url, trimmed: null };

  const notes: string[] = [];
  if (f.log) {
    const lines = f.log.split("\n");
    const total = lines.length;
    while (lines.length > 1 && encode(title, { ...f, log: lines.join("\n") }).length > MAX_URL) {
      lines.shift();
    }
    f.log = lines.join("\n");
    url = encode(title, f);
    if (url.length > MAX_URL) {
      delete f.log;
      url = encode(title, f);
      notes.push("The log was left out");
    } else if (lines.length < total) {
      notes.push(`The log was shortened to its last ${lines.length} of ${total} lines`);
    }
  }
  if (url.length > MAX_URL && f["what-happened"]) {
    let what = f["what-happened"];
    while (what.length > 0 && encode(title, { ...f, "what-happened": what }).length > MAX_URL) {
      what = what.slice(0, Math.floor(what.length * 0.9));
    }
    f["what-happened"] = what;
    url = encode(title, f);
    notes.push("“What happened” was shortened");
  }
  return {
    url,
    trimmed: notes.length > 0 ? `${notes.join("; ")} to fit GitHub's link length limit.` : null,
  };
}
