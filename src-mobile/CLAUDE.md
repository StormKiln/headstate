# src-mobile

The iOS/Android companion. This crate is the **shell and the companion
surface** — it renders the same `src/` frontend the desktop app does, so most
UI work happens in `src/`, not here.

See the root `CLAUDE.md` for rules that apply everywhere.

## The surface table is mirrored and easy to miss

`src-mobile/src/surface.rs` mirrors `src-tauri/src/remote/surface.rs`. A new
command must be classified in **both**.

The mobile copy is checked by `make test-mobile`, which `yarn vitest run` and
`cargo test` do **not** run. A drift here passes every local gate and fails CI.

## Capability, not layout

Use `IS_MOBILE_BUILD` (`src/lib/target.ts`) to decide whether a command exists;
use `useIsMobile()` only for viewport layout. See `src/CLAUDE.md`.

## Releasing

`docs/mobile-release-process.md` is the process — follow it rather than
improvising. The build high-water mark is guarded by
`scripts/check-mobile-build-mark.py` and must not lag what shipped.

Pairing walkthrough: `docs/mobile-pairing-walkthrough.md`.

## Gates

```bash
make lint-mobile
make test-mobile
```

Both, whenever `src-mobile/` **or the shared `src/` frontend** changed.
