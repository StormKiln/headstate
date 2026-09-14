/// Which list the `j`/`k`/`Enter`/`x` cursor is walking right now.
///
/// # The defect (#953)
///
/// `App.tsx` binds one `keydown` listener and its list-navigation branch
/// closed over `visible` -- `sortPrs(applyFilters(source, filters))`, a
/// `PullRequest[]`. So the cursor could only ever walk the pull request
/// list. On the other nine views `j`/`k` either did nothing (`rows.length
/// === 0`) or silently moved a PR cursor and toggled PR checkboxes
/// BEHIND whatever page was on screen.
///
/// The worst case is measurable: the Claude Code sessions list is ~1,474
/// rows with "Show all" pressed, each row one focusable `<button>`, with
/// no roving tabindex. Reaching the detail pane past it is ~1,474 Tab
/// presses. Settings → Keyboard lists `j`/`k` and says "Move down / up
/// the list", which is true of exactly one list of ten.
///
/// # Scope, and why this is not a general list cursor
///
/// This registry wires ONE more list -- the sessions list -- rather than
/// generalising the cursor to all ten views. The issue offers both and
/// the smaller one is taken deliberately:
///
/// - Ten views means ten different row identities and ten different
///   meanings for "open" and "select". `x` toggles a PR checkbox for a
///   bulk action; there is no bulk action on sessions, worktrees are
///   removed through a safety verdict, and Docker images through a
///   different one. Inventing a selection model per view to satisfy one
///   keybinding is a much larger change than the one the issue's
///   headline asks for.
/// - The keys are BARE LETTERS, and the risk of stealing one from a text
///   field grows with every surface they reach. `shortcuts.ts` states the
///   rule -- "searching for 'jack' moves the cursor four times" -- and the
///   sessions list is the one place where a search box is the primary
///   control, which makes it the right place to prove the guard rather
///   than the tenth.
/// - The measured cost is concentrated. Nine of the ten lists are short
///   enough that Tab reaches the end; one is not, by three orders of
///   magnitude.
///
/// What this does buy is that the tenth view is a `useRowCursor` call
/// rather than another `else if` in `App.tsx`, so the coupling the issue
/// is actually about -- a window listener that knows what a pull request
/// is -- is gone either way.
///
/// # Why a module-level registry rather than context or a store slice
///
/// The listener in `App.tsx` is a `window` listener bound in an effect
/// that must NOT re-bind per keystroke, and #953's "stale-list hazard"
/// is explicit that a registration indirection must not reintroduce the
/// bug the `visible` dependency exists to prevent: *"a registered list
/// has to be read at keypress time, not captured at mount."*
///
/// So the registry is read through a function at the moment a key is
/// pressed. A React context would have to be captured into the effect
/// (the stale-capture bug, restated) or re-bind the listener on every
/// render of the list. A Zustand slice would work and would also make the
/// list part of persisted-ish app state, which it is not: this is one
/// rendering's rows, alive exactly as long as the component that drew
/// them.
///
/// # One cursor, not two
///
/// `filters.ts` scopes filters per view but `cursor` is a single value,
/// and #953 forbids a second: two lists owning two cursors is the drift
/// this repo refuses elsewhere. The registry therefore holds AT MOST ONE
/// active list, and registering replaces whatever was there. Two lists
/// mounted at once would be a bug in the caller, and
/// `rowCursor.test.ts` pins the replacement rather than leaving it
/// implicit.

/// What a registered list can do, in the vocabulary the shortcut handlers
/// already speak.
///
/// `rows` is a FUNCTION rather than an array, which is the whole point:
/// it is called when a key is pressed, so it returns the rows on screen
/// then and not the rows on screen when the component last rendered.
export interface RowCursorTarget {
  /// How many rows the cursor may walk. Read at keypress time.
  rows: () => number;
  /// Open the row at this index -- what `Enter` does.
  open: (index: number) => void;
  /// Check or uncheck the row at this index, for a bulk action.
  ///
  /// Optional: a list with no bulk action leaves it out and `x` does
  /// nothing there, which is better than inventing a selection the view
  /// has no way to act on.
  toggle?: (index: number) => void;
}

let active: RowCursorTarget | null = null;

/// Make this list the one the cursor walks, returning the undo.
///
/// Registering REPLACES any previous target rather than stacking, per the
/// one-cursor rule above. The returned function clears the registration
/// only if it is still the current one, so an unmount that arrives after
/// a different list has registered -- React's order on a view switch is
/// mount-then-unmount for a replaced subtree -- cannot blank the new
/// list's registration. Without that check the sequence "sessions mounts,
/// PRs unmounts" leaves nothing registered and every key dead.
export function registerRowCursor(target: RowCursorTarget): () => void {
  active = target;
  return () => {
    if (active === target) active = null;
  };
}

/// The list the cursor is walking, or null when no view has claimed it.
///
/// Null is a real answer and the caller must handle it: nine views
/// register nothing, and on those the cursor keys must do NOTHING rather
/// than fall back to some other view's list, which is the defect.
export function activeRowCursor(): RowCursorTarget | null {
  return active;
}

/// Where the cursor moves next, clamped to the list.
///
/// Clamped rather than wrapped, and `App.tsx` carries the reason:
/// "wrapping from the bottom back to the top silently moves the eye
/// across the whole screen." Extracted here so the rule is stated once
/// and tested without a DOM, rather than living inside a `keydown`
/// handler where neither is true.
///
/// `null` in means "no cursor yet", and the first press of either key
/// lands on row 0 -- the behaviour the PR list has always had. An empty
/// list stays `null`: there is no row to be on.
export function nextCursor(
  cursor: number | null,
  rows: number,
  direction: "next" | "prev",
): number | null {
  if (rows <= 0) return null;
  if (cursor === null) return 0;
  const moved = direction === "next" ? cursor + 1 : cursor - 1;
  return Math.min(Math.max(moved, 0), rows - 1);
}

/// Clear the registry. Tests only -- module state outlives a render.
export function resetRowCursorForTest(): void {
  active = null;
}
