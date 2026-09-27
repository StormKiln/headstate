/// #1480: a message or a turn as markdown for the clipboard.

import { describe, expect, it } from "vitest";
import type { TranscriptMessage } from "../../types/transcript";
import { call, output } from "./fixtures";
import { dividerLabel, fenced, messageMarkdown, turnMarkdown } from "./transcriptCopy";

function msg(
  id: string,
  turn: string | null,
  kind: TranscriptMessage["kind"],
  blocks: TranscriptMessage["blocks"] = [],
): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: turn,
    kind,
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks,
  };
}

const text = (t: string, index = 0) => ({ kind: "text" as const, index, text: t, clip: null });

describe("fenced", () => {
  it("fences with more backticks than the text holds", () => {
    expect(fenced("a ``` b")).toBe("````\na ``` b\n````");
    expect(fenced("plain")).toBe("```\nplain\n```");
  });
});

describe("messageMarkdown", () => {
  it("quotes the prompt under who said it", () => {
    const md = messageMarkdown(msg("u1", "u1", { kind: "user_prompt", origin: null }, [text("line one\n\nline two")]));
    expect(md).toBe("**You**\n\n> line one\n>\n> line two");
  });

  it("says a call with no result has none, and fences a result's output", () => {
    const bash = { tool: "bash", command: "make test", description: null, truncated: false } as const;
    const md = messageMarkdown(
      msg("a1", "u1", { kind: "assistant" }, [
        text("Running it."),
        call("Bash", bash, output({ text: "ok", clip: { shown_chars: 2, total_chars: 900 } }), "t1"),
        { ...call("Bash", bash, null, "t2"), index: 2 },
      ]),
    );
    expect(md).toContain("Running it.");
    expect(md).toContain("**⏺ Bash(`make test`)**\n\n```\nok\n```");
    // The clip is said, as on screen.
    expect(md).toContain("showing the first 2 of 900 characters");
    expect(md).toContain("_No result in what was read._");
  });

  it("names a thinking block that was not recorded rather than copying nothing", () => {
    const md = messageMarkdown(
      msg("a1", "u1", { kind: "assistant" }, [
        { kind: "thinking", index: 0, text: "", clip: null, recorded: false },
      ]),
    );
    expect(md).toBe("> _Thinking (not recorded)_");
  });

  it("writes a record that is not the conversation as its labelled divider", () => {
    const md = messageMarkdown(
      msg("c1", "u1", {
        kind: "compaction_boundary",
        trigger: "auto",
        pre_tokens: 150_000,
        post_tokens: 20_000,
      }),
    );
    expect(md).toBe("_— Conversation compacted (auto): 150,000 → 20,000 tokens —_");
  });
});

describe("dividerLabel", () => {
  it("says an unrecognised record's type rather than nothing", () => {
    expect(dividerLabel(msg("x", null, { kind: "unrecognised", record_type: "brand-new" }))).toBe(
      "Unrecognised record: brand-new",
    );
  });

  it("does not print a slash twice", () => {
    expect(dividerLabel(msg("s", "s", { kind: "slash_command", name: "/review" }))).toBe("/review");
    expect(dividerLabel(msg("s", "s", { kind: "slash_command", name: "review" }))).toBe("/review");
  });
});

describe("turnMarkdown", () => {
  it("copies every message of the turn, in order, and nothing from the next", () => {
    const all = [
      msg("u1", "u1", { kind: "user_prompt", origin: null }, [text("first ask")]),
      msg("a1", "u1", { kind: "assistant" }, [text("first answer")]),
      msg("u2", "u2", { kind: "user_prompt", origin: null }, [text("second ask")]),
    ];
    const md = turnMarkdown(all, "u1");
    expect(md).toBe("**You**\n\n> first ask\n\nfirst answer\n");
    expect(md).not.toContain("second ask");
  });

  it("says when the turn began above what was read", () => {
    const md = turnMarkdown([msg("a0", null, { kind: "assistant" }, [text("tail end")])], null);
    expect(md.startsWith("_This turn began before the part of the transcript that was read._")).toBe(true);
  });
});
