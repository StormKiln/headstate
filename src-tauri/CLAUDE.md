# src-tauri

The Rust backend. See the root `CLAUDE.md` for rules that apply everywhere.

## A new command must be classified in BOTH surface tables

`src-tauri/src/remote/surface.rs` **and** `src-mobile/src/surface.rs`, plus a
dispatch arm. Missing either breaks CI.

Classes: `Read | Write | Destructive | Local`. The classification is about what
the command *does*, not how it is rendered.

## Prefer a guard to a comment

`src-tauri/src/invariants.rs` holds source-scanning guards that catch what
ordinary tests miss. Before writing prose asking people to remember something,
ask whether a guard can check it. See the `guard` skill — including the three
ways this repo has already got one wrong.

## Process-wide statics are shared state between tests

`OBSERVED_REMAINING` is the one that bites. `Budget::record` writes it, so any
test driving a response with `rateLimit.remaining` mutates state every other
test's gate reads.

- Sync tests serialise on `budget::observed_test_lock()` and restore with
  `RestoreObserved`.
- **Async tests cannot.** The lock returns a `std::sync::MutexGuard`, and
  clippy's `await_holding_lock` under `-D warnings` refuses to let one be held
  across an `.await`.
- Use `Budget::seeded_for_test`, which seeds the local accumulator only.
  `permits` takes the lower of the local and process figures, so a refusal still
  happens with nothing shared touched.

Getting this wrong burned the v5.20.0 tag (#1048). The symptom is a test that
**passes alone and fails in-suite** — learn to recognise it.

## Platform

- **`PathBuf::join`**, never string concatenation. Windows `canonicalize`
  returns verbatim `\\?\C:\` paths.
- **Normalise `\r\n`** before any `\n`-anchored byte pattern, or it matches
  nothing on a CRLF checkout and passes silently on Windows only.

## Before pushing

`cargo fmt` — `lint-rust` fails on formatting, the cheapest CI failure to have
avoided. Then `cargo test --lib` and report the count.
