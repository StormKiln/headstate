/// `aria-current` for the selected row in a single-select list.
///
/// `"true"` rather than `"page"`: these rows SCOPE the page you are already
/// on -- they pick which subject the detail pane shows -- rather than
/// navigating to a different one, and `aria-current="page"` would claim each
/// row is its own page. That is the `RepoSidebar` / `StatsSidebar` case, not
/// the `SystemHealthSidebar` / `ClaudeCodeSidebar` one.
///
/// `undefined` rather than `"false"` on the inactive rows -- the attribute's
/// absence is how "not current" is spelled, and `aria-current="false"` is
/// announced by some readers.
///
/// A helper rather than an inline ternary at each call site, so the rows in
/// one list cannot drift and another cannot be added without it. Exported
/// and declared ONCE, because four sidebars each carried a private copy of
/// this same line and the two newest lists -- the Claude Code session list
/// and the CLAUDE.md file list -- could not inherit it and reached for
/// `aria-pressed` instead (#977). "Pressed" describes a toggle the user just
/// operated and invites pressing it again to un-press; these are
/// single-select rows where a second click is a no-op, so it announced a
/// control that does not exist and gave no way to tell where in the list the
/// user was. `DockerSidebar` records the same defect fixed under #852.
///
/// This is only for single-select LIST ROWS. Genuine toggles keep
/// `aria-pressed` -- the chips, the filter bar, `SystemHealthPage`'s
/// controls -- which `SystemHealthPage` states as the inverse case.
export function current(active: boolean): "true" | undefined {
  return active ? ("true" as const) : undefined;
}
