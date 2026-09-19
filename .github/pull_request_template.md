<!--
This checklist MIRRORS the gate rather than adding to it. The required
checks are the only thing that blocks a merge; these are the things CI
cannot see, and the ones this repository has shipped as defects before.

Delete any line that does not apply.
-->

## What changed, and why it mattered

<!-- The mechanism, not the symptom. An issue body that says only what
the user saw makes the next reader redo the diagnosis. -->

## Verification

- [ ] `make lint` — **counts reported, not adjectives**
- [ ] `make test` — Rust and frontend counts against the baseline
- [ ] `make test-mobile` if `src/` or `src-mobile/` changed
      <!-- The iOS companion renders the same `src/` frontend, so a
           frontend change is also a mobile change. -->

## Rules this repository has shipped as defects

- [ ] **Absent is not zero.** A reading that could not be taken is not a
      reading of zero (#846).
- [ ] **Partial is not nothing.** A bounded operation that runs out of
      budget keeps what it already retrieved, qualified (#1044).
- [ ] **Pending and Unknown are different states.** "Not checked yet"
      renders as a skeleton; "checked, could not decide" renders as a
      failure (#1042).
- [ ] **Never report "we did not ask" as "they did not answer"** (#1050).
- [ ] **Qualify, or suppress.** Only-low → qualify ("at least N");
      possibly-wrong → suppress.
- [ ] `PathBuf::join`, never `format!("{}/…")` — Windows `canonicalize`
      returns verbatim `\\?\C:\` paths, and this has cost six
      Windows-only failures.

## If this adds a Tauri command

<!-- Five wiring points. Missing any one fails a guard, usually in CI. -->

- [ ] `commands.rs`, `lib.rs` (registration)
- [ ] `remote/surface.rs` — row in the correct class **block** and a
      dispatch arm
- [ ] `src-mobile/src/surface.rs` — the same row, same position
- [ ] `src/api/tauri.ts` wrapper, and a row in `transport.test.ts`
