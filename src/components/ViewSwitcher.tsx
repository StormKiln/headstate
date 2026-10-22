import { ChevronDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { MOBILE_HIDDEN_VIEWS, type View, useFilters } from "../store/filters";
import { useUiPrefs } from "../api/hooks";
import { IS_MOBILE_BUILD } from "../lib/target";
import { current } from "../lib/ariaCurrent";
import { useIsMobile } from "../lib/useIsMobile";
import { ALWAYS_OFFERED, GROUPS, VIEWS } from "./viewMenu";

export function ViewSwitcher({ counts }: { counts?: Partial<Record<View, number>> }) {
  const { view: storedView, setView } = useFilters();
  const view =
    IS_MOBILE_BUILD && MOBILE_HIDDEN_VIEWS.has(storedView) ? "my-prs" : storedView;
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const currentView = VIEWS.find((v) => v.id === view) ?? VIEWS[0];
  // A LAYOUT question, so the hook rather than `IS_MOBILE_BUILD` (#1020).
  const isMobile = useIsMobile();
  const { prefs } = useUiPrefs();
  // Two views are never hidden, whatever is stored:
  //
  // - "my-prs" is the default view and the app's whole premise. Hiding
  //   it would leave someone with no way back to what they installed
  //   this for.
  // - The CURRENT view, even when hidden, or the app would show a page
  //   its own switcher says does not exist -- with no way off it.
  const hidden = new Set(prefs?.hidden_views ?? []);
  // The build-time set is checked FIRST and overrides both escape
  // hatches above. A view the companion does not ship is not hidden by
  // preference -- it does not exist in this bundle, so "but it is the
  // current view" cannot make it offerable: the phone has no page to
  // show behind the entry. `App.tsx` is what keeps `view` off such a
  // value in the first place, so the two cannot disagree about what is
  // on screen.
  // A CAPABILITY check, in the same position and for the same reason as
  // the build-time set above: it overrides both escape hatches (#916).
  //
  // The distinction from `hidden_views` is the whole point. That list means
  // "I do not want to see this", so honouring it loosely is correct -- a
  // user sitting on a view they hid keeps it rather than being thrown off
  // mid-task. A switched-off integration is not that: there is no page
  // behind the entry, so "but it is the current view" must not make it
  // offerable, exactly as for a view the companion does not ship.
  //
  // Routing this through `hidden_views` instead would also clobber a real
  // preference -- someone who hid the Claude Code view for their own
  // reasons would lose that the first time the capability toggled.
  const capabilityOff = (id: View) =>
    id === "claude-code" && !prefs?.claude_integrations_enabled;
  const offered = VIEWS.filter(({ id }) =>
    (IS_MOBILE_BUILD && MOBILE_HIDDEN_VIEWS.has(id)) || capabilityOff(id)
      ? false
      : ALWAYS_OFFERED.has(id) || id === view || !hidden.has(id),
  );

  // Group AFTER filtering, and drop a group with no surviving members
  // (#1018).
  //
  // The order matters and is the whole of the issue. The natural refactor
  // -- iterate the groups, collect each one's views, filter inside the
  // loop -- renders the heading before it knows whether anything is left
  // under it, and an "AI" heading with nothing beneath it is a menu
  // telling the user about a feature they cannot reach. Filtering first
  // and keeping only non-empty groups makes that state unrepresentable
  // rather than merely unlikely.
  //
  // This is reachable without any capability. `hidden_views` can empty
  // Repositories, Builds and System outright -- none of their members is
  // in `ALWAYS_OFFERED`. Only Pull requests is protected, and only
  // because `my-prs` is. The Claude capability is the second route to
  // the same state, not the only one.
  //
  // The saving property is that the CURRENT view is always offered, so
  // the group the user is standing in can never be the empty one. That
  // is what makes "render nothing" safe: dropping a group can never drop
  // the way back to where you are.
  //
  // A view whose `group` is not in `GROUPS` cannot exist -- `Group` is
  // derived from `GROUPS` -- so this loses nothing the type permits.
  const visibleGroups = GROUPS.map((g) => ({
    ...g,
    members: offered.filter((v) => v.group === g.id),
  })).filter(({ members }) => members.length > 0);

  // Dismiss on Escape and on a click elsewhere. Without both, the menu
  // stays open behind whatever the user does next.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onClick);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onClick);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative mb-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-haspopup="menu"
        className="flex w-full items-center gap-2 rounded px-3 py-2 text-sm font-semibold text-[#e6edf3] hover:bg-[#161b22]"
      >
        <currentView.Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
        <span className="truncate">{currentView.label}</span>
        <ChevronDown
          className={`ml-auto h-3.5 w-3.5 shrink-0 transition-transform ${
            open ? "rotate-180" : ""
          }`}
          aria-hidden="true"
        />
      </button>

      {open ? (
        <div
          role="menu"
          className="absolute left-0 right-0 top-full z-20 mt-1 rounded border border-[#30363d] bg-[#161b22] p-1 shadow-lg"
        >
          {visibleGroups.map(({ id: groupId, label: groupLabel, members }) => {
            const headingId = `view-group-${groupId}`;
            return (
              <div
                key={groupId}
                // `role="group"` with `aria-labelledby`, NOT a bare `<h3>`
                // (#1022). `role="menu"`'s content model admits `menuitem`,
                // `menuitemradio`, `menuitemcheckbox`, `group` and
                // `separator` -- a heading element is not among them, so an
                // `<h3>` dropped straight into the menu is a child a screen
                // reader is entitled to ignore, taking the label with it.
                // Wrapping the members in a `group` and pointing it at the
                // heading keeps the label attached to the items it names.
                role="group"
                aria-labelledby={headingId}
              >
                {/* Painted on every layout (#1403). #1020 hid these on the
                    narrow layout, reasoning that "the phone's sheet has
                    288px" of vertical space for five headings to crowd --
                    but 288px is the sheet's WIDTH (`w-72` in `App.tsx`); a
                    left sheet spans the full viewport height. Five headings
                    cost ~100px of it, and they are what makes a 13-entry
                    menu scannable, most of all on a phone.

                    Slightly tighter on the narrow layout -- less space above
                    each heading -- which is the one part of #1020's instinct
                    worth keeping. `useIsMobile()` rather than
                    `IS_MOBILE_BUILD`, per `lib/target.ts`: this is layout,
                    and a desktop window dragged narrow gets the same. */}
                <h3
                  id={headingId}
                  className={`px-2 pb-0.5 ${isMobile ? "pt-1" : "pt-1.5"} text-xs font-semibold uppercase tracking-wide text-[#8b949e]`}
                >
                  {groupLabel}
                </h3>
                {members.map(({ id, label, Icon }) => (
                  <button
                    key={id}
                    type="button"
                    role="menuitem"
                    // `current(...)` from `@/lib/ariaCurrent` (#977, adopted
                    // here in #1022). This was `aria-current={id === view}`,
                    // which React serialises to the literal string "false"
                    // on every non-current item -- announced by some
                    // readers, where the attribute's ABSENCE is how "not
                    // current" is spelled. `ViewSwitcher` was the one
                    // navigation list that never adopted the shared helper,
                    // because it already had the attribute and so did not
                    // look broken.
                    aria-current={current(id === view)}
                    onClick={() => {
                      setView(id);
                      setOpen(false);
                    }}
                    className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-sm ${
                      id === view ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#21262d]"
                    }`}
                  >
                    <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">{label}</span>
                    {counts?.[id] ? (
                      <span className="ml-auto text-xs tabular-nums">{counts[id]}</span>
                    ) : null}
                  </button>
                ))}
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
