import { Blocks, Bot, Gauge, ListTree } from "lucide-react";
import type { ClaudePage } from "../store/filters";

/// Apart from `ClaudeCodeSidebar.tsx` so that file exports only
/// components (#1965), which is what lets dev hot-reload swap it in place.

/// Every Claude Code page, in sidebar order.
///
/// Exported so the sidebar and any other renderer of the same list cannot
/// drift into offering different pages -- the failure `HEALTH_PAGES` exists
/// to prevent, and #675 fixed for `VIEWS`.
///
/// `"overview"` is first because it is the landing page
/// (#939). #921 put `"sessions"` first and argued for it: "the view is
/// opened to get a crashed session back, and the overview answers a second
/// question rather than the first." Using the view disproved both halves.
/// The overview is not a chart standing in the way of that answer -- it
/// carries the resurrection tiles and the resumable list, which is the
/// thing a user acts on, so it answers the FIRST question. And the
/// sessions list is no longer behind it: since #939 the list renders in
/// this column, under the `Sessions` row, a glance away whichever page is
/// selected. "Sessions first" was buying immediate access to something
/// that is no longer hidden, at the price of hiding the tiles.
///
/// The store's `claudePage` default and its `setView` reset both say
/// `"overview"` to match. Three places, one decision: were they to
/// disagree, where you land would depend on how you arrived.
///
/// This is the ONLY list of the pages -- `ClaudePage` in the store is a
/// plain union rather than an `as const` array, precisely so there is no
/// second declaration of the same two names for this one to drift from.
export const CLAUDE_PAGES: {
  id: ClaudePage;
  label: string;
  Icon: typeof Bot;
}[] = [
  { id: "overview", label: "Overview", Icon: Gauge },
  { id: "sessions", label: "Sessions", Icon: ListTree },
  // Last: it answers a question asked occasionally ("am I getting value
  // out of what I installed?") rather than the one the view is opened
  // for. Its scan is also the most expensive thing behind any of these
  // pages, and putting it above Sessions would make arriving here cost a
  // corpus read nobody asked for.
  { id: "plugins", label: "Plugins", Icon: Blocks },
];
