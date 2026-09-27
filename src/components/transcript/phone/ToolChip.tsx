import { useState } from "react";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "../../ui/sheet";
import { DiffView } from "../DiffView";
import { changeFromArgs } from "../diff";
import { InSheet } from "../inSheet";
import { palette } from "../palette";
import { ToolCall } from "../ToolCall";
import type { ToolCallBlock } from "../types";
import { toolChip, type ChipTone } from "./chip";
import { usePhone } from "./context";
import { scaleStyle } from "./textScale";
import { callDuration } from "./timing";

/// Above this many diff lines, an inline diff is capped in height with a
/// "Show full diff" button. A phone screen shows about thirty lines of
/// transcript; one edit should not take half of it by default.
export const INLINE_DIFF_LINES = 12;

const TONE: Record<ChipTone, string> = {
  ok: palette.ok,
  error: palette.error,
  warn: palette.warn,
  muted: palette.muted,
};

/// One tool call on the phone (#1481): a compact chip that opens the
/// whole call -- output, diff, arguments -- in a bottom sheet.
///
/// The sheet draws `ToolCall` with `variant="compact"` (#1483), inside
/// `InSheet`, so its folds open in place rather than stacking a second
/// sheet. An edit's diff is ALSO shown under the chip by default (the
/// epic's decided default), capped in height beyond
/// `INLINE_DIFF_LINES`, with "Show full diff" lifting the cap.
export function ToolChip({ call, callAt }: { call: ToolCallBlock; callAt: string | null }) {
  const phone = usePhone();
  const [open, setOpen] = useState(false);
  const chip = toolChip(call, phone.liveness, phone.tasks);
  const name = chip.status ? `${chip.text}, ${chip.status}` : chip.text;

  return (
    <div className="min-w-0">
      <Sheet open={open} onOpenChange={setOpen}>
        <button
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-label={name}
          onClick={() => setOpen(true)}
          data-tone={chip.tone}
          // Wraps rather than truncates: at a large text size the whole
          // label still reads, on as many lines as it needs.
          className="inline-flex min-h-9 max-w-full items-baseline gap-x-1.5 rounded-2xl border px-3 py-1.5 text-left text-[13px] leading-snug break-words focus-visible:outline focus-visible:outline-2"
          style={{
            borderColor: palette.border,
            background: palette.surface,
            color: palette.text,
          }}
        >
          <span aria-hidden style={{ color: TONE[chip.tone] }}>
            ⏺
          </span>
          <span className="min-w-0 break-words">
            {chip.text}
            {chip.status ? <span style={{ color: TONE[chip.tone] }}> · {chip.status}</span> : null}
          </span>
        </button>
        <SheetContent side="bottom" className="max-h-[85dvh] overflow-y-auto">
          <SheetHeader>
            <SheetTitle className="pr-8 break-words">{chip.text}</SheetTitle>
          </SheetHeader>
          <div className="px-4 pb-4" style={scaleStyle(phone.scale)}>
            <InSheet.Provider value={true}>
              <ToolCall
                call={call}
                variant="compact"
                liveness={phone.liveness}
                durationMs={callDuration(call, callAt)}
                onLoadFullText={phone.onLoadFullText}
                onOpenSubagent={phone.onOpenSubagent}
                tasks={phone.tasks}
              />
            </InSheet.Provider>
          </div>
        </SheetContent>
      </Sheet>
      <InlineDiff call={call} />
    </div>
  );
}

/// The diff an edit made, under its chip. Nothing for any other tool, or
/// for an edit the tool refused -- the chip says "failed", and the
/// attempted change is in the sheet, labelled as not applied.
function InlineDiff({ call }: { call: ToolCallBlock }) {
  const [full, setFull] = useState(false);
  const args = call.args;
  if (args.tool !== "edit" && args.tool !== "multi_edit") return null;
  if (call.result?.is_error === true) return null;
  const change = call.result?.change ?? changeFromArgs(args);
  if (!change) return null;
  const lines = change.hunks.reduce((n, h) => n + h.lines.length, 0);
  const capped = lines > INLINE_DIFF_LINES && !full;
  return (
    <div className="mt-1">
      <div
        data-testid="inline-diff"
        data-capped={capped ? "true" : undefined}
        className={capped ? "relative max-h-[14em] overflow-hidden" : undefined}
      >
        <DiffView change={change} variant="compact" />
        {capped ? (
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 bottom-0 h-8"
            style={{
              background: `linear-gradient(transparent, ${palette.ground})`,
            }}
          />
        ) : null}
      </div>
      {lines > INLINE_DIFF_LINES ? (
        <button
          type="button"
          aria-expanded={full}
          onClick={() => setFull((f) => !f)}
          className="mt-0.5 min-h-8 text-[12px] underline-offset-2 hover:underline focus-visible:outline focus-visible:outline-2"
          style={{ color: palette.link }}
        >
          {full ? "Show less of the diff" : "Show full diff"}
        </button>
      ) : null}
    </div>
  );
}
