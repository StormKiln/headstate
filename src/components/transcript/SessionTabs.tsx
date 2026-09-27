/// A selected session's two tabs (#1546): **Details** and **Transcript**.
///
/// The transcript used to be a separate full-window route that replaced
/// the detail (#1479). It is a tab now, so there is one way in: the tab,
/// which `openClaudeTranscript` selects for a deep link.
///
/// # Layout
///
/// The tab body fills the pane's height and adds no scroller of its own
/// to the Transcript panel: the viewer (`MessageScroller`) is the one
/// scroll container there, as it was in the full window, and the
/// composer's slot (`COMPOSER_ENABLED`, #1490) stays under it inside the
/// viewer. The Details panel is ordinary content, so IT scrolls.
///
/// # Keyboard and screen readers (#1489)
///
/// Base UI's tabs give the `tablist`/`tab`/`tabpanel` roles, their
/// `aria-selected`/`aria-controls`/`aria-labelledby` wiring and the
/// roving tabindex. Arrow keys move between the two tabs and activate
/// the one they land on (`activateOnFocus`): with two tabs there is
/// nothing to skip past, and the WAI-ARIA pattern recommends automatic
/// activation when a panel shows without a noticeable delay -- the
/// viewer renders at once and reads behind its own skeleton.
///
/// A hidden panel is unmounted (Base UI's default), so a session on its
/// Details tab holds no transcript pages -- the phone's memory bound
/// (#1476) is the viewer's, and leaving the tab releases it.
///
/// Colours are the transcript palette's, so `palette.test.ts` checks
/// every one of them.
import type { ReactNode } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { ClaudeSessionTab } from "@/store/filters";

/// Overrides for the shadcn trigger's theme tokens, stated for the
/// dark variant too because the app is dark-only (`<html class="dark">`)
/// and the stock classes set colours under `dark:`.
const TRIGGER = [
  "tap-target flex-none px-3 text-xs",
  "text-[#8b949e] dark:text-[#8b949e] hover:text-[#e6edf3] dark:hover:text-[#e6edf3]",
  "data-active:bg-[#21262d] data-active:text-[#e6edf3]",
  "dark:data-active:bg-[#21262d] dark:data-active:text-[#e6edf3] dark:data-active:border-[#30363d]",
  "focus-visible:outline-2 focus-visible:outline-[#58a6ff]",
].join(" ");

export function SessionTabs({
  value,
  onValueChange,
  details,
  transcript,
}: {
  value: ClaudeSessionTab;
  onValueChange: (tab: ClaudeSessionTab) => void;
  details: ReactNode;
  transcript: ReactNode;
}) {
  return (
    <Tabs
      value={value}
      onValueChange={(v) => onValueChange(v === "transcript" ? "transcript" : "details")}
      className="flex min-h-0 min-w-0 flex-1 flex-col gap-3"
      data-testid="session-tabs"
    >
      <TabsList
        aria-label="Session"
        activateOnFocus
        className="shrink-0 border border-[#30363d] bg-[#161b22] group-data-horizontal/tabs:h-auto"
      >
        <TabsTrigger value="details" className={TRIGGER}>
          Details
        </TabsTrigger>
        <TabsTrigger value="transcript" className={TRIGGER}>
          Transcript
        </TabsTrigger>
      </TabsList>
      <TabsContent value="details" className="min-h-0 overflow-y-auto">
        {details}
      </TabsContent>
      {/* `flex` and no `overflow`: the viewer inside is the scroller. */}
      <TabsContent value="transcript" className="flex min-h-0 flex-col">
        {transcript}
      </TabsContent>
    </Tabs>
  );
}
