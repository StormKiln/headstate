# src

The React 19 + TypeScript frontend. **Rendered by both the desktop app and the
iOS companion** (`src-mobile/`), so a change here reaches the mobile app. Run
`make test-mobile` as well as `yarn vitest run`.

See the root `CLAUDE.md` for rules that apply everywhere.

## `useIsMobile()` vs `IS_MOBILE_BUILD`

Two different questions, routinely confused:

- **`useIsMobile()`** (`src/lib/useIsMobile.ts`) — a *layout* question. Is the
  viewport narrow? True in a resized desktop window.
- **`IS_MOBILE_BUILD`** (`src/lib/target.ts`) — a *capability* question. Is this
  the iOS build? Decides whether a command exists at all.

`src/lib/target.ts` argues the distinction at length. Read it before reaching
for either; the file exists because getting it wrong is easy.

## Accessibility

**Use `current()` from `src/lib/ariaCurrent.ts` for `aria-current`.** A bare
boolean serialises `false` to the string `"false"`, which a screen reader
announces as *current*. That shipped in #1037, was fixed in #1039, and was
nearly reintroduced in #1043.

```tsx
aria-current={current(id === view)}   // "true" | undefined
```

`aria-current` is for navigation; `aria-pressed` is for toggles.

## A component and its host can land in different PRs

#1038 built a table and #1039 built the page meant to host it. Both PRs were
green; nothing rendered the table. Before calling a component done:

```bash
grep -rn 'ComponentName' src --include='*.tsx' | grep -v 'ComponentName.tsx\|\.test\.'
```

Zero hits outside its own file and test means it is dead code.

## Distinguish the empty states

A skeleton means "not measured yet". An error panel means "measured, and it
failed". A zero means "measured, and it was zero". Three different states — do
not let one render as another. This is the root file's absent-is-not-zero rule
as it shows up in the UI.

When a load fails, say **why** in terms the reader can act on, and do not offer
a retry that cannot succeed (#1050).

## A warning states a fact, not an excuse

A warning says **what is true** and **what the reader can do about it**. It does
not explain the implementation, name an internal, or defend the app's conduct.

> These are floors, not totals: the transcript is 76.7 MB and only its first
> 8 MB were read.

That is the whole message. What followed it — *"Reading it whole would hang this
pane"* — was the defect #1088 is about: it justified a decision the reader did
not make, and carried a performance claim the code's own measurements did not
support (`usage.rs` records that transcript rolled up in **0.024 s**).

- **Cut the `so that…` / `which is…` tail.** "Files are read up to a fixed
  limit" is a fact. "…so that a large one cannot be pulled over the connection
  whole" is a design note wearing a warning's clothes.
- **Never name an internal.** "the poll driving the rest of this view" is not
  something the reader can see, check, or act on.
- **Keep a constraint that changes what the reader concludes.** "the system does
  not attribute GPU work per process without elevated privileges" STAYS — it
  stops them thinking the panel broke. "…so Headstate reports the totals it can
  read" goes; that half is the app talking about itself.
- **The rationale belongs in the module docs**, where it is developer-facing and
  correct. #1088 was one sentence that leaked out of `usage.rs` onto a screen.

Measured claims decay. If a warning states a figure, it must come from the
response — `measuredFigures.test.ts` enforces this — and never from a constant
that was true the day it was typed.
