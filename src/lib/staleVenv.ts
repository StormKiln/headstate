/// The one place the frontend decides how long idle counts as stale.
///
/// # Why this file exists (#957)
///
/// Staleness was decided in THREE places that could disagree:
///
/// | where | value |
/// |---|---|
/// | `src-tauri/src/caches/mod.rs` `STALE_SECS` | 90 days, fixed |
/// | `VenvSection.tsx` `STALE_SECS` | 90 days, fixed |
/// | `UiPrefs::stale_venv_days` → `poll::stale_venv_days` | configurable |
///
/// The third is the one the DELETE re-verifies against
/// (`RemovalPolicy::stale_days`, enforced by `remove_venvs`), and it had
/// no UI control at all -- so it was always 0, which resolves to 90, so
/// all three agreed and nothing was visibly wrong.
///
/// `VenvSection`'s own comment argued the duplication was safe because
/// the constant "is only used to LABEL rows here… and this never gates a
/// removal". That half is wrong: `displayState` returns `"stale"` from
/// it and `isRemovable` returns `true` for `"stale"`, so the frontend
/// constant decides whether the CHECKBOX is enabled. The backend decides
/// the delete. Two numbers, two decisions, one action -- which is fine
/// only while they are equal, and #957 is the issue that they need not
/// be the moment anyone exposes the setting.
///
/// Exposing it is what this change does, so both failures it would have
/// caused are closed here instead of created:
///
/// 1. above 90, a row reads Stale and its checkbox is enabled, and the
///    delete is then REFUSED by `RemovalPolicy` -- a confirmed action
///    that silently does nothing;
/// 2. below 90, rows the backend would delete are never offered.
///
/// Both are the frontend using a different threshold from the backend,
/// so the frontend stops having one of its own and resolves the stored
/// preference through the same rules Rust does.

/// The threshold when nothing is stored, in days.
///
/// Ninety, and `caches/mod.rs` carries the argument: "a project worked on
/// seasonally is normal, and the cost of a wrong call here is a
/// re-resolve, but the cost of nagging about a live project is that the
/// whole view stops being trusted."
///
/// Asserted against the Rust `STALE_SECS` in `mirroredConstants.test.ts`,
/// which is where this pair belonged from the start.
export const DEFAULT_STALE_DAYS = 90;

/// The floor and ceiling `poll::stale_venv_days` clamps to.
///
/// The floor is the load-bearing one and its comment says why: "this
/// number gates a delete once the opt-in above is on -- so the floor is
/// what stops a typo in Settings from making live work selectable."
export const MIN_STALE_DAYS = 30;
export const MAX_STALE_DAYS = 3650;

/// Days idle before a virtualenv counts as stale, honouring the setting.
///
/// The TypeScript half of `poll::stale_venv_days`, and deliberately the
/// same three rules in the same order, because the two run against the
/// same stored integer and a divergence is the defect this file exists
/// to prevent:
///
/// - **0 means "use the default", never "everything is stale."** That
///   reading is not a nicety -- `UiPrefs::stale_venv_days` defaults to 0
///   and every existing install therefore stores one, so treating 0 as a
///   threshold would reclassify every venv on every machine as stale the
///   moment this shipped. The Rust comment states it as a safety rule:
///   "a stored 0 from a bad write must not reclassify the whole cache."
/// - **Clamped, not trusted.** A stored value can come from anywhere the
///   preference row can be written, including a future control or a hand
///   edit, and the floor is what keeps live work unselectable.
/// - **Non-integral and negative input resolves to the default** rather
///   than clamping up to the floor. Rust cannot reach this branch at all
///   -- `stale_venv_days` is a `u32` -- so there is no behaviour to
///   mirror, and treating a value that could not have come from this
///   app's own control as "not set" is the conservative reading: it
///   restores 90 rather than inventing 30.
export function staleVenvDays(stored: number | undefined): number {
  if (stored === undefined || !Number.isInteger(stored) || stored <= 0) {
    return DEFAULT_STALE_DAYS;
  }
  return Math.min(Math.max(stored, MIN_STALE_DAYS), MAX_STALE_DAYS);
}

/// The same threshold in seconds, which is the unit every idle time on
/// the Artifacts page is measured in.
export function staleVenvSecs(stored: number | undefined): number {
  return staleVenvDays(stored) * 24 * 60 * 60;
}

/// The choices Settings offers.
///
/// A fixed set rather than a free-text number, which is the shape #957
/// asks for and the one `INTERVALS` already uses for the poll interval.
/// A number field would need the clamp re-implemented on the way in, and
/// a field that silently rewrites 7 to 30 is a worse control than one
/// that never offered 7: the user is left believing a threshold the app
/// does not have.
///
/// Every entry is inside `[MIN_STALE_DAYS, MAX_STALE_DAYS]`, asserted in
/// `staleVenv.test.ts` -- an option the clamp would rewrite is exactly
/// the lie a select is supposed to make impossible.
export const STALE_DAY_CHOICES = [30, 90, 180, 365] as const;

export function staleDaysLabel(days: number): string {
  if (days === 365) return "1 year";
  return `${days} days`;
}
