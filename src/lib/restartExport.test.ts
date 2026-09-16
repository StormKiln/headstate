import { describe, expect, it } from "vitest";
import { restartExportText } from "./restartExport";
import type {
  ClaudeRestartEntry,
  ClaudeRestartList,
  ClaudeUncertainEntry,
} from "@/types/pr";

/// An anchored entry, as `resume_command` builds one for a cwd that
/// exists: the `cd` is present and there is nothing to warn about.
function anchored(sessionId: string, cwd: string): ClaudeRestartEntry {
  return {
    session_id: sessionId,
    name: null,
    cwd,
    resume: {
      command: `cd '${cwd}' && claude --resume '${sessionId}'`,
      caveat: null,
      anchored: true,
    },
  };
}

/// An entry whose directory is GONE, as `resume_command` builds one: the
/// bare command, and the caveat that has to travel with it.
function gone(sessionId: string, cwd: string): ClaudeRestartEntry {
  return {
    session_id: sessionId,
    name: null,
    cwd,
    resume: {
      command: `claude --resume '${sessionId}'`,
      caveat: `The directory this session ran in is gone (${cwd}), so this will resume in whatever directory you run it from.`,
      anchored: false,
    },
  };
}

function list(over: Partial<ClaudeRestartList> = {}): ClaudeRestartList {
  return {
    running: [],
    uncertain: [],
    registry_failure: null,
    registry_unreadable: [],
    ...over,
  };
}

/// Every line that is NOT a `#` comment. These are the lines a shell
/// would execute, which is the only set worth asserting over when the
/// question is "what does pasting this actually run".
function commandLines(text: string): string[] {
  return text
    .split("\n")
    .filter((l) => l.trim() !== "" && !l.startsWith("#"));
}

describe("restartExportText", () => {
  /// A `Gone` directory exports the bare command WITH its caveat, and the
  /// caveat is ABOVE the line.
  ///
  /// Above, not beside: the user pastes this file blind, and a bare
  /// `claude --resume` resumes in whatever directory the shell happens to
  /// be in. A warning they read after running the line is not a warning.
  it("exports a gone directory as a bare command with its caveat above it", () => {
    const text = restartExportText(list({ running: [gone("s1", "/tmp/deleted")] }));
    const lines = text.split("\n");
    const at = lines.indexOf("claude --resume 's1'");

    expect(at).toBeGreaterThan(-1);
    expect(commandLines(text)).toEqual(["claude --resume 's1'"]);
    // No `cd` anywhere in what would be executed.
    for (const line of commandLines(text)) {
      expect(line).not.toContain("cd ");
    }
    // The caveat is the line immediately above, and it is commented so
    // the whole file stays pasteable.
    expect(lines[at - 1]).toMatch(/^# .*is gone \(\/tmp\/deleted\)/);
  });

  /// A multi-line caveat gets a `#` on EVERY line.
  ///
  /// One `#` on the first line only would put prose straight into the
  /// shell, which is the failure the comment prefix exists to prevent --
  /// and it would be invisible until someone pasted a file whose caveat
  /// happened to wrap.
  it("comments every line of a multi-line caveat", () => {
    const entry = gone("s1", "/tmp/x");
    entry.resume.caveat = "first line\nsecond line";
    const text = restartExportText(list({ running: [entry] }));

    expect(text).toContain("# first line\n# second line\n");
    expect(commandLines(text)).toEqual(["claude --resume 's1'"]);
  });

  /// A session id carrying shell metacharacters round-trips quoted.
  ///
  /// The id is `path.file_stem()` of an arbitrary `*.jsonl` with no
  /// format check, and this file is the surface where an unquoted one
  /// does the most damage: the user pastes the whole thing without
  /// reading each line.
  it("keeps a metacharacter-bearing id quoted", () => {
    const nasty = "$(whoami)`id`;rm -rf ~";
    const text = restartExportText(list({ running: [anchored(nasty, "/tmp/x")] }));
    const commands = commandLines(text);

    expect(commands).toHaveLength(1);
    expect(commands[0]).toContain(`claude --resume '${nasty}'`);
    // The id appears ONLY inside quotes. Checked as the absence of an
    // unquoted occurrence, because a line carrying both would satisfy a
    // "contains the quoted form" assertion and still substitute.
    const outside = commands[0].replace(`'${nasty}'`, "");
    expect(outside).not.toContain("$(");
    expect(outside).not.toContain("`");
    expect(outside).not.toContain(";");
  });

  /// A non-null `registry_failure` makes the export say the list may be
  /// short.
  ///
  /// The count is qualified as "at least", and the reason is named. An
  /// unqualified count under a failed registry read is the confident
  /// number that might be wrong, and here being wrong means the user
  /// reboots over live work.
  it("says the list may be short when the registry could not be read", () => {
    const text = restartExportText(
      list({
        running: [anchored("s1", "/tmp/x")],
        registry_failure: "could not read ~/.claude/sessions: permission denied",
      }),
    );

    expect(text).toContain("INCOMPLETE");
    expect(text).toContain("permission denied");
    expect(text).toContain("at least 1 session");
    expect(text).not.toContain("— 1 session");
  });

  /// An unreadable record alone qualifies the list too.
  ///
  /// Separate from the test above because the two shortfalls arrive
  /// independently: a builder that checked only `registry_failure` would
  /// print an unqualified count on a machine where one record could not
  /// be parsed, and each such record hides a session that may be running.
  it("says the list is a floor when a record could not be read", () => {
    const text = restartExportText(
      list({
        running: [anchored("s1", "/tmp/x")],
        registry_unreadable: ["4821.json: invalid JSON at line 1"],
      }),
    );

    expect(text).toContain("at least 1 session");
    expect(text).toContain("floor, not a count");
    expect(text).toContain("4821.json: invalid JSON at line 1");
  });

  /// An `Unknown`-liveness session is INCLUDED, under its own heading.
  ///
  /// #984 in export form: treating `Unknown` as a shade of `Dead`
  /// misclassified 183 of 1,491 sessions, and here that mistake is work
  /// the user rebooted away. It appears, its command is pasteable, and
  /// its grounds are stated -- but it is under a separate heading, so
  /// nobody mistakes "could not tell" for "is running".
  it("includes an uncertain session, in its own section, with its grounds", () => {
    const uncertain: ClaudeUncertainEntry = {
      ...anchored("s-unknown", "/tmp/x"),
      why: "the live session registry could not be read",
    };
    const text = restartExportText(
      list({ running: [anchored("s-running", "/tmp/y")], uncertain: [uncertain] }),
    );

    expect(commandLines(text)).toEqual([
      "cd '/tmp/y' && claude --resume 's-running'",
      "cd '/tmp/x' && claude --resume 's-unknown'",
    ]);
    expect(text).toContain("# Might be running — we could not tell");
    expect(text).toContain("# Why we could not tell: the live session registry could not be read");
    // The boundary is visible and the uncertain line is on the far side
    // of it, so a reader pasting downwards cannot cross without seeing.
    const heading = text.indexOf("# Might be running");
    expect(text.indexOf("claude --resume 's-running'")).toBeLessThan(heading);
    expect(text.indexOf("claude --resume 's-unknown'")).toBeGreaterThan(heading);
    // And the split is stated in the header, so the count is not read as
    // three confirmed-running sessions.
    expect(text).toContain("2 sessions (1 running, 1 could not be checked)");
  });

  /// An empty list with a readable registry is a settled answer, and says
  /// so plainly.
  it("states a measured zero as an answer", () => {
    const text = restartExportText(list());

    expect(commandLines(text)).toEqual([]);
    expect(text).toContain("No Claude Code session is running");
    expect(text).not.toContain("INCOMPLETE");
  });

  /// An empty list under a failed registry read is NOT a zero, and must
  /// not read as one.
  ///
  /// This is the pair to the test above and the one that matters: "no
  /// session is running" is exactly the sentence that makes a reboot look
  /// safe, so the unreadable case must never produce it.
  it("does not state an unreadable registry as a zero", () => {
    const text = restartExportText(list({ registry_failure: "permission denied" }));

    expect(text).not.toContain("No Claude Code session is running");
    expect(text).toContain("That is not the");
    expect(text).toContain("same as nothing running.");
  });

  /// Every non-command line is a shell comment, so the whole file can be
  /// selected and pasted.
  ///
  /// The property the `#` prefixes exist for. Asserted over a list
  /// carrying every shape at once, because a prefix missed on one branch
  /// would otherwise only show up on the machine that hit that branch.
  it("emits only commands and comments, so the whole file is pasteable", () => {
    const text = restartExportText(
      list({
        running: [anchored("s1", "/tmp/x"), gone("s2", "/tmp/deleted")],
        uncertain: [{ ...gone("s3", "/tmp/other"), why: "could not probe the process table" }],
        registry_failure: "permission denied",
        registry_unreadable: ["4821.json: bad"],
      }),
    );

    for (const line of text.split("\n")) {
      if (line === "") continue;
      const isCommand = line.startsWith("cd '") || line.startsWith("claude --resume ");
      expect(line.startsWith("#") || isCommand).toBe(true);
    }
    expect(text.endsWith("\n")).toBe(true);
  });

  /// The header carries a timestamp.
  ///
  /// The file outlives the machine state it describes. Without a
  /// generated-at line a user cannot tell today's list from one they
  /// saved a month ago, and pasting the stale one resumes the wrong set.
  it("stamps the header with when it was generated", () => {
    const text = restartExportText(list({ running: [anchored("s1", "/tmp/x")] }));

    expect(text.startsWith("# Claude Code sessions to restart\n")).toBe(true);
    expect(text).toMatch(/^# Generated \d{4}-\d{2}-\d{2}T[\d:.]+Z — /m);
  });
});
