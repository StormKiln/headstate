import type { ClaudeRestartEntry, ClaudeRestartList } from "@/types/pr";

/// The restart list, rendered as the text a user saves before a reboot
/// (#1071).
///
/// # Why this is a pure function in `lib/` and not JSX
///
/// It is the thing the feature actually produces. A component that built
/// the string inline could only be tested through the DOM, and the four
/// properties that matter here -- a `Gone` directory never emits a `cd`,
/// a metacharacter-bearing id stays quoted, a shortfall is stated, an
/// `Unknown` session is present -- are properties of the TEXT. Testing
/// them through a render would test the rendering instead.
///
/// # The shape, and why every part of it is load-bearing
///
/// ```text
/// # Claude Code sessions to restart
/// # Generated 2026-09-15T09:41:02.000Z — 3 sessions (2 running, 1 could not be checked)
///
/// cd '/Users/x/repo' && claude --resume 'abc-123'
///
/// # The directory this session ran in is gone (/tmp/old), so this will
/// # resume in whatever directory you run it from.
/// claude --resume 'def-456'
/// ```
///
/// - **A `#` header with the time and the count.** The file outlives the
///   machine state it describes; a list with no timestamp is one the user
///   cannot tell from a stale one they saved last month.
/// - **The caveat ABOVE its line, as a `#` comment.** Not beside it and
///   not in a footnote: a bare `claude --resume` resumes in whatever
///   directory the shell happens to be in, and the user is pasting these
///   blind. Above means they read it before the line they are about to
///   run.
/// - **Shell comments, so the whole file is pasteable.** Every
///   non-command line starts with `#`, which means a user who selects the
///   lot and pastes it gets the commands and not syntax errors.
///
/// The commands themselves are NOT built here. They arrive built, from
/// `claude::sessions::resume_command`, which quotes both halves and
/// handles four cwd states -- see `ClaudeRestartEntry`.

/// The header's first line. Also what the test matches on, so a change
/// here is a change both sides see.
const TITLE = "# Claude Code sessions to restart";

/// Render one entry: its caveat as `#` comments, then its command.
///
/// The caveat is wrapped at nothing -- it is emitted verbatim on however
/// many lines it already occupies -- but every one of those lines gets
/// its own `#`. A multi-line caveat with only the first line commented
/// would put prose into the shell.
function renderEntry(entry: ClaudeRestartEntry): string[] {
  const lines: string[] = [];
  if (entry.resume.caveat !== null) {
    for (const line of entry.resume.caveat.split("\n")) {
      lines.push(`# ${line}`);
    }
  }
  lines.push(entry.resume.command);
  return lines;
}

/// The one-line summary of what the list covers, and what it does not.
///
/// Follows the house rule: only-low -> qualify, possibly-wrong ->
/// suppress. Both shortfalls here are only-low, so the count becomes
/// "at least N" rather than disappearing -- a short list the user is TOLD
/// is short is worth far more before a reboot than no list at all.
function summary(list: ClaudeRestartList): string[] {
  const total = list.running.length + list.uncertain.length;
  const short = 
    list.registry_failure !== null ||
    list.registry_unreadable.length > 0 ||
    list.registry_unnamed.length > 0;
  const noun = total === 1 ? "session" : "sessions";
  const count = short ? `at least ${total} ${noun}` : `${total} ${noun}`;
  const split =
    list.uncertain.length > 0
      ? ` (${list.running.length} running, ${list.uncertain.length} could not be checked)`
      : "";

  const lines = [`# Generated ${new Date().toISOString()} — ${count}${split}`];

  if (list.registry_failure !== null) {
    // The strongest qualifier, and it says what it means for the list
    // rather than only naming the error. With no registry, NOTHING could
    // be positively established as running -- so an empty list here is
    // not "nothing was running", and a user who read it that way would
    // reboot over live work.
    lines.push(
      `# INCOMPLETE: the live session registry could not be read (${list.registry_failure}),`,
      "# so no session could be confirmed running. Sessions below are the ones we could",
      "# still reason about; there may be others this list does not name.",
    );
  }
  if (list.registry_unreadable.length > 0) {
    const n = list.registry_unreadable.length;
    lines.push(
      `# INCOMPLETE: ${n} live session ${n === 1 ? "record" : "records"} could not be read, and each`,
      "# hides a session that may be running. This list is a floor, not a count.",
      `# ${list.registry_unreadable[0]}`,
    );
  }
  if (list.registry_unnamed.length > 0) {
    // #1315. Running, and not in this file: nothing names the session, so
    // there is no resume line to write. Each process is named so the
    // user can find its terminal before the reboot.
    const n = list.registry_unnamed.length;
    lines.push(
      `# INCOMPLETE: ${n} running Claude Code ${n === 1 ? "session is" : "sessions are"} not matched to a saved`,
      "# session, so no resume line is written for them. This list is a floor, not a count.",
      ...list.registry_unnamed.map((line) => `# ${line}`),
    );
  }
  return lines;
}

/// Build the text of the restart export.
///
/// Always returns something, including for an empty list: a file saying
/// "no session was running" is an ANSWER the user can act on, and
/// returning an empty string would leave them unable to tell it from a
/// copy that failed. The distinction between a settled empty answer and
/// an unreadable registry is carried by `summary` above.
export function restartExportText(list: ClaudeRestartList): string {
  const lines = [TITLE, ...summary(list)];

  if (list.running.length > 0) {
    lines.push("", "# Running now");
    for (const entry of list.running) {
      lines.push("", ...renderEntry(entry));
    }
  }

  if (list.uncertain.length > 0) {
    // A SEPARATE heading, never merged into the block above. "This is
    // running" and "we could not tell whether this is running" are
    // different claims, and resuming something already alive starts a
    // second copy of it -- so the boundary has to be visible to someone
    // pasting line by line.
    lines.push(
      "",
      "# Might be running — we could not tell",
      "# Included deliberately: a session left out of this list is work you lose on",
      "# restart. Resuming one that had already finished costs you a window.",
    );
    for (const entry of list.uncertain) {
      lines.push("", `# Why we could not tell: ${entry.why}`, ...renderEntry(entry));
    }
  }

  if (list.running.length === 0 && list.uncertain.length === 0) {
    const short = 
    list.registry_failure !== null ||
    list.registry_unreadable.length > 0 ||
    list.registry_unnamed.length > 0;
    lines.push(
      "",
      ...(short
        ? // NOT a zero. The qualifier above already said why, and this
          // line must not contradict it by sounding settled -- "nothing
          // is running" is exactly what makes a reboot look safe.
          [
            "# Nothing could be confirmed running — see the note above. That is not the",
            "# same as nothing running.",
          ]
        : // A measured zero, and it is allowed to say so plainly: the
          // registry WAS read, and nothing in it was alive.
          ["# No Claude Code session is running, so there is nothing to restart."]),
    );
  }

  // A trailing newline, so the last command runs when the file is piped
  // to a shell rather than pasted. A pasted final line without one waits
  // at the prompt for Enter, which is merely odd; a piped one is silently
  // dropped by some shells, which is not.
  return `${lines.join("\n")}\n`;
}
