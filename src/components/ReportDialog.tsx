import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { ExternalLink } from "./ExternalLink";
import { IS_MOBILE_BUILD } from "@/lib/target";
import {
  buildSections,
  composeFields,
  describeCounts,
  issueUrl,
  reportTitle,
  scrubCounted,
  type Gathered,
  type ReportContext,
} from "../lib/report";
import { lookupBundle, lookupVersion } from "../lib/reportError";

/// The report, previewed and redactable, before anything leaves the app
/// (#1575).
///
/// Every section is shown exactly as it will be sent, is editable, and
/// can be left out. "Open on GitHub" then opens the prefilled bug form in
/// the browser, where the user reviews it once more and submits it
/// themselves. The app never submits.
///
/// # No query hooks, by construction
///
/// This renders inside `ErrorBoundary`'s crash panel, which sits above
/// `QueryClientProvider`. The lookups are plain promises in an effect for
/// that reason -- see `ReportLink`.
///
/// # Mounted per opening
///
/// The parent mounts this only while it is open, so every opening starts
/// from a fresh report and fresh lookups, and nothing a user typed into a
/// previous report can leak into the next one.
export function ReportDialog({
  context,
  onClose,
}: {
  context: ReportContext;
  onClose: () => void;
}) {
  /// `undefined` while asking; `null` once asked and not answered.
  const [version, setVersion] = useState<string | null | undefined>(undefined);
  const [gathered, setGathered] = useState<Gathered>({ kind: "pending" });
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set());
  const [title, setTitle] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void lookupVersion().then((v) => live && setVersion(v));
    void lookupBundle().then(
      (b) => live && setGathered(b ? { kind: "ready", bundle: b } : { kind: "unavailable" }),
    );
    return () => {
      live = false;
    };
  }, []);

  const { sections, removed } = useMemo(
    () =>
      buildSections(context, {
        appVersion: version ?? null,
        gathered,
        mobile: IS_MOBILE_BUILD,
      }),
    [context, version, gathered],
  );
  const shown = sections.map((s) => ({ ...s, text: edits[s.id] ?? s.text }));
  const issueTitle = title ?? reportTitle(context.error);
  const { url, trimmed } = issueUrl(
    scrubCounted(issueTitle).text,
    composeFields(shown, excluded),
  );
  const removedNote = describeCounts(removed);
  const pending = gathered.kind === "pending" || version === undefined;

  const toggle = (id: string) =>
    setExcluded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-2xl text-[#e6edf3]">
        <DialogTitle>Report this problem</DialogTitle>
        <p className="text-sm text-[#8b949e]">
          Nothing is sent from here. Read what the report says, change or leave out
          anything, then open it on GitHub to review and submit it there.
        </p>
        <p aria-live="polite" className="text-xs text-[#8b949e]">
          {pending
            ? "Gathering details…"
            : gathered.kind === "unavailable"
              ? "Some details could not be read, and say “unknown” below."
              : ""}
        </p>
        <p className="text-xs text-[#8b949e]">
          {removedNote
            ? `Removed automatically: ${removedNote}. Anything you type is scrubbed the same way.`
            : "Nothing needed removing automatically. Anything you type is scrubbed before it is sent."}
        </p>

        <label className="flex flex-col gap-1 text-xs font-medium">
          Issue title
          <input
            type="text"
            value={issueTitle}
            onChange={(e) => setTitle(e.target.value)}
            className="w-full rounded border border-[#30363d] bg-[#0d1117] px-2 py-1.5 text-sm text-[#e6edf3]"
          />
        </label>

        {shown.map((s) => {
          const out = excluded.has(s.id);
          const more = describeCounts(scrubCounted(s.text).removed);
          const lines = s.text.split("\n").length;
          return (
            <fieldset key={s.id} className="flex min-w-0 flex-col gap-1">
              {/* The legend FIRST: it names the group, so the checkbox
                  below reads as "Error, Leave this out" to a screen
                  reader rather than as one of ten identical toggles. */}
              <legend className="text-xs font-medium">{s.title}</legend>
              <label className="tap-target flex items-center gap-1.5 self-start text-xs text-[#8b949e]">
                <input type="checkbox" checked={out} onChange={() => toggle(s.id)} />
                Leave this out
              </label>
              <textarea
                aria-label={s.title}
                value={s.text}
                disabled={out}
                onChange={(e) => setEdits((prev) => ({ ...prev, [s.id]: e.target.value }))}
                rows={Math.min(Math.max(lines, 1), 8)}
                className="w-full rounded border border-[#30363d] bg-[#0d1117] p-2 font-mono text-xs text-[#e6edf3] disabled:opacity-40"
              />
              {more && !out ? (
                <p className="text-xs text-[#d29922]">Will also be removed when sent: {more}.</p>
              ) : null}
            </fieldset>
          );
        })}

        {trimmed ? (
          <p role="note" className="text-xs text-[#d29922]">
            {trimmed}
          </p>
        ) : null}

        <div className="flex flex-wrap items-center justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="tap-target rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]"
          >
            Cancel
          </button>
          <ExternalLink
            href={url}
            onClick={onClose}
            className="tap-target rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#2ea043]"
          >
            Open on GitHub
          </ExternalLink>
        </div>
      </DialogContent>
    </Dialog>
  );
}
