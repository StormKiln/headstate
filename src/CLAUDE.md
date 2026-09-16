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
