import { describe, expect, it } from "vitest";
import type { WireClaudeSessionList } from "@/types/pr";
import { hydrateClaudeSessions } from "./hooks";

/// The transport boundary for the Claude Code session list (#985).
///
/// # What is being defended
///
/// `claude_sessions` returned every row with every field on every
/// ten-second poll: 1,474 rows, 990 bytes each, 1.35 MB, and the phone
/// paid it over the pairing transport. The fix splits the row by FIELD --
/// the list keeps what it searches, filters, counts and draws; the rest
/// is fetched for the one selected session -- and interns the one field
/// the list does render that was almost entirely repetition, the liveness
/// reason, which was the same 150-character sentence on 1,474 of 1,474
/// real rows.
///
/// These tests are about the interning half, because that is the half
/// that could silently lose something. A dropped `why` would leave "Not
/// running" with no grounds; a mis-resolved one would put a different
/// session's grounds under it, which is worse. So the assertions are that
/// the sentence survives byte-for-byte, that distinct reasons stay
/// distinct, and that a table which cannot answer produces `unknown`
/// rather than a fabricated or empty verdict.
const DEAD =
  "no process was ever recorded for this session, and it is not in the live session " +
  "registry -- which lists every running session -- so it is not running";
const UNKNOWN = "could not read the live session registry: Permission denied";

const wire = (over: Partial<WireClaudeSessionList> = {}): WireClaudeSessionList => ({
  sessions: [
    {
      session_id: "s1",
      name: "about s1",
      cwd: "/code/widget",
      git_branch: "feat/spoon",
      last_activity_at: "2026-09-13T09:00:00Z",
      liveness: { state: "dead", why: 0 },
      cwd_state: { state: "exists" },
    },
  ],
  reasons: [DEAD],
  registry_failure: null,
  registry_unreadable: [],
  ...over,
});

describe("hydrateClaudeSessions", () => {
  it("resolves an interned reason back to the exact sentence", () => {
    const got = hydrateClaudeSessions(wire());
    expect(got.sessions[0].liveness).toEqual({ state: "dead", why: DEAD });
  });

  /// The saving is only legitimate if it is a transport encoding. The
  /// list renders this string as every row's `title`, so a shortened or
  /// summarised version would be a silent loss of the grounds behind a
  /// verdict -- the defect class epic #941 is about.
  it("carries one reason once and gives every row the same sentence", () => {
    const got = hydrateClaudeSessions(
      wire({
        sessions: [
          {
            session_id: "s1",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "dead", why: 0 },
            cwd_state: { state: "not-recorded" },
          },
          {
            session_id: "s2",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "dead", why: 0 },
            cwd_state: { state: "not-recorded" },
          },
        ],
        reasons: [DEAD],
      }),
    );
    expect(got.sessions.map((s) => s.liveness)).toEqual([
      { state: "dead", why: DEAD },
      { state: "dead", why: DEAD },
    ]);
  });

  /// The failure interning could introduce: two sessions whose verdicts
  /// differ must not end up sharing one. Putting one session's grounds
  /// under another's verdict is worse than having no grounds, because it
  /// is confidently wrong.
  it("keeps two different reasons apart", () => {
    const got = hydrateClaudeSessions(
      wire({
        sessions: [
          {
            session_id: "dead",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "dead", why: 0 },
            cwd_state: { state: "not-recorded" },
          },
          {
            session_id: "unknown",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "unknown", why: 1 },
            cwd_state: { state: "not-recorded" },
          },
        ],
        reasons: [DEAD, UNKNOWN],
      }),
    );
    expect(got.sessions[0].liveness).toEqual({ state: "dead", why: DEAD });
    expect(got.sessions[1].liveness).toEqual({ state: "unknown", why: UNKNOWN });
  });

  /// A running row has no reason to intern, and the pid and status ride
  /// on the row itself. Asserted so a future refactor cannot start
  /// routing `running` through the table and inventing an entry for it.
  it("passes a running liveness through untouched", () => {
    const got = hydrateClaudeSessions(
      wire({
        sessions: [
          {
            session_id: "s1",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "running", pid: 14779, status: "busy" },
            cwd_state: { state: "exists" },
          },
        ],
        reasons: [],
      }),
    );
    expect(got.sessions[0].liveness).toEqual({
      state: "running",
      pid: 14779,
      status: "busy",
    });
  });

  /// An index with no entry should be impossible -- the backend builds
  /// the table and the indices in one pass. This is about what happens if
  /// it ever is not.
  ///
  /// `unknown`, never `dead`. `?? ""` would put an empty tooltip under a
  /// confident "Not running", and defaulting to the dead sentence would
  /// invent grounds the app does not have. `unknown` is the state that
  /// means "the check did not complete", and #841 is why it must not be
  /// rendered as a shade of dead: "not running" is what offers Resume,
  /// and resuming a live session starts a second copy of it.
  it("turns an unresolvable reason into unknown rather than a fabricated verdict", () => {
    const got = hydrateClaudeSessions(
      wire({
        sessions: [
          {
            session_id: "s1",
            name: null,
            cwd: null,
            git_branch: null,
            last_activity_at: null,
            liveness: { state: "dead", why: 7 },
            cwd_state: { state: "not-recorded" },
          },
        ],
        reasons: [DEAD],
      }),
    );
    const liveness = got.sessions[0].liveness;
    expect(liveness.state).toBe("unknown");
    // And it SAYS what went wrong, rather than being a blank shrug.
    expect(liveness.state === "unknown" && liveness.why).toMatch(/did not arrive/);
    expect(liveness.state === "unknown" && liveness.why).toMatch(/index 7 of 1/);
  });

  /// Absent is not zero (#846). The two registry fields are the reason
  /// the response is an envelope rather than a bare array, and the split
  /// must not have quietly dropped either.
  it("carries the registry failure and the unreadable list through", () => {
    const got = hydrateClaudeSessions(
      wire({ registry_failure: "Permission denied", registry_unreadable: ["4242.json"] }),
    );
    expect(got.registry_failure).toBe("Permission denied");
    expect(got.registry_unreadable).toEqual(["4242.json"]);
  });

  /// The honest total, which is #985's stated trap.
  ///
  /// A server-side `LIMIT` would have made the stated total a separate
  /// claim that could drift from the rows beside it. The count line reads
  /// `all.length`, so the invariant is simply that hydration returns as
  /// many rows as arrived -- every row, in order. 200 and 201 bracket
  /// `RENDER_CAP`, the only cap in the feature.
  it.each([200, 201])("returns every one of %i rows, in order", (n) => {
    const sessions = Array.from({ length: n }, (_, i) => ({
      session_id: `s${i}`,
      name: null,
      cwd: null,
      git_branch: null,
      last_activity_at: null,
      liveness: { state: "dead" as const, why: 0 },
      cwd_state: { state: "not-recorded" as const },
    }));
    const got = hydrateClaudeSessions(wire({ sessions, reasons: [DEAD] }));
    expect(got.sessions).toHaveLength(n);
    expect(got.sessions.map((s) => s.session_id)).toEqual(sessions.map((s) => s.session_id));
  });

  /// The four fields search covers survive the split.
  ///
  /// #985's other stated trap: search is this list's primary navigation
  /// and filters the whole corpus on these four. Any of them missing from
  /// the list tier would narrow search to whatever still carried it -- a
  /// worse feature that still looks like it works.
  it("keeps all four searchable fields on the row", () => {
    const got = hydrateClaudeSessions(wire());
    const row = got.sessions[0];
    expect(row.session_id).toBe("s1");
    expect(row.name).toBe("about s1");
    expect(row.cwd).toBe("/code/widget");
    expect(row.git_branch).toBe("feat/spoon");
  });
});
