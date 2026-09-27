# Transcript send readiness (7.9 groundwork for 7.10)

7.10 is meant to let a person send a message to a running Claude Code session,
from the desktop or from the phone. 7.9 does not send anything. It makes sure
that 7.10 only has to **add** a send, not redesign the transcript viewer to make
room for one (#1491). #1490 lists the constraints the 7.9 design must satisfy,
and this note records how each one is met.

**Send command class: to be decided in 7.10 planning.** The owner decided on
2026-09-26 to leave the channel open: iTerm2's API, tmux, Terminal.app, and how
delivery is confirmed. That choice decides the class, so it is not made here.

## What 7.9 ships

| piece | where | state in 7.9 |
|---|---|---|
| pending-message model and reconciliation | `src/components/transcript/pending.ts` | complete and tested; nothing creates a pending message |
| host state for pending messages | `src/components/transcript/usePendingMessages.ts` | held by both hosts, and always empty |
| pending rendering, desktop | `TerminalPendingMessage` in `TerminalMessage.tsx` | complete and tested |
| pending rendering, phone | `PhonePendingMessage` in `phone/PhoneMessage.tsx` | complete and tested |
| composer | `src/components/transcript/Composer.tsx` | built, passed to the viewer's slot by both hosts, and hidden behind `COMPOSER_ENABLED` (`composerFlag.ts`); it has no `onSend`, so it could not send even if shown |
| phone keyboard avoidance | `src/components/transcript/keyboardInset.ts` | built and tested in jsdom; **not yet measured on a device** |
| send scroll behaviour | `src/components/transcript/sendScroll.ts` plus `TranscriptViewer` | built and tested; runs only when a composer sends |

Nothing is registered on the remote surface. No command, event, or wire type is
added.

## The #1490 constraints

1. **Stable message identity.** Delivered by #1475, not by this issue. A
   record's `uuid` is its id (`IdSource` in
   `src-tauri/src/claude/transcript_model.rs`, mirrored as
   `TranscriptIdSource` in `src/types/transcript.ts`). A uuid-less record gets
   an anchored id, and the variant says which promise the id makes. The tests
   that pin this are `ids_are_stable_across_a_reread`,
   `a_duplicate_uuid_keeps_the_first`,
   `permission_mode_change_gets_an_anchored_id` and
   `a_page_starting_on_uuid_less_records_anchors_them_as_a_whole_read_does`.
   On the follow side, `compaction mid-follow keeps the reader on the same
   message id` in `src/lib/transcriptFollow.test.ts` covers a compaction.
   Reconciliation in 7.9 does not rely on that id, because a sent message has
   no id until it is recorded. Rules 1 to 4 below match it by content and time.
   Once a record has matched, its id is what the viewer uses.
2. **Local pending messages are representable.** *Met by this issue.*
   `PendingMessage` holds `clientId`, `text`, `createdAt`, `state`, `after` and
   `reason`. `state` is one of `pending`, `delivered`, `failed` or
   `unconfirmed`. The viewer draws pending messages after the newest record,
   using `renderPending`, and only while its window reaches the tail. Both
   renderers draw every state visibly differently from a recorded prompt:
   - the desktop draws a dashed, dimmed band with no time and no copy buttons;
   - the phone draws a dimmed, dashed, right-aligned bubble.

   In both, the state is shown in a `role="status"` line.
3. **The layout has a composer slot.** *Met by this issue.* The viewer is a
   column: the scroller takes `flex-1` and the slot takes `shrink-0` beneath
   it. Showing the composer therefore shrinks the scroller rather than pushing
   the conversation, and the scroller's follow re-pins the live edge when its
   viewport shrinks. On the phone, `useKeyboardInset` reads
   `visualViewport` (`innerHeight - height - offsetTop`) and pads the composer
   bar by that amount. The padding is `max(0.5rem, safe area, keyboard)`, so
   opening the keyboard shrinks the scroller in the same way.
   `sendGroundwork.test.tsx` checks the following:
   - showing or hiding the slot keeps the live edge pinned and remounts no row;
   - a reader who has scrolled up is not moved;
   - opening or closing the keyboard keeps the live edge pinned.

   jsdom has no layout, so these tests declare the height each change takes
   and check how the viewer responds.
4. **Scroll behaviour for sends.** *Met by this issue.* `sendScroll` anchors the
   new pending row (`scrollAnchor`) and calls `scrollToEnd`, which re-engages
   follow-output. If the reader's window stops short of the tail, it re-windows
   to the tail first, as "jump to latest" does. A reader who had scrolled up is
   brought back. When the transcript's own record replaces the pending row, it
   takes the same position, and the test asserts that nothing jumps.

   Both hosts pass `pending`, `renderPending` and `composer` to the viewer.
   `sendWiring.test.ts` checks this, because while the flag is off, a host
   that dropped them would render exactly the same. A host that filters its
   rows (#1484) has two further duties:
   - it reconciles against every message it holds, not only the shown ones,
     or a hidden record would leave its pending message on screen;
   - it keeps the viewer mounted when a filter hides every row, because the
     composer slot is inside the viewer.
5. **Live follow latency.** Delivered by #1476 and #1477. The follow
   (`src/lib/transcriptFollow.ts`) reads every `FAST_MS` (750 ms) while a
   running session's file has grown in the last minute. When the file is
   quiet, it backs off from 5 s to 15 s. `claude::activity` stats every
   running session's transcript once per `TICK` (1 s). It emits the
   content-free `claude-session-activity` event on a change, and that event
   is in both `EVENT_NAMES` lists. A follower with that session open then
   reads at once (`nudge`, wired in `src/api/hooks.ts`). A reply therefore
   reaches the phone about a second after it is written, even after a quiet
   spell.
6. **No 7.9 choice locks out a channel.**
   - Liveness is `Liveness::Running { pid, status }`
     (`src-tauri/src/claude/liveness.rs`), derived from the registry that
     Claude Code writes, including terminal-launched sessions (#1315). A tty
     or terminal handle would be one more field on `Running`, found from the
     same pid.
   - The header (#1485) has a `reply` slot that nothing fills
     (`TranscriptHeader.tsx`). `makes no send-capability claim in 7.9` checks
     that the header says nothing about replying or sending.
   - The read-only rule in `claude/mod.rs` covers writes to `~/.claude`. A
     send through a terminal channel writes nothing there, so it does not
     have to be unwound.
   - The composer takes an `onSend` callback and knows nothing about how a
     message is delivered.
7. **The remote surface has room for a new class.** *Met by this issue.* The
   surface is unchanged: no `Class` variant is added and no command is
   classified. 7.10 can add a new `Class` variant or register the send as
   `Destructive` with step-up, and nothing in 7.9 prevents either. A new
   variant has to go into both `src-tauri/src/remote/surface.rs` and
   `src-mobile/src/surface.rs`. The idempotency key is described in the next
   section. Step-up signs the canonical command and arguments
   (`stepup.rs`), so a `clientId` in the arguments is covered by the
   signature. The invariant
   `every_transcript_command_is_masked_at_the_remote_boundary` flags any
   remote command whose name contains "transcript". A send that returns no
   transcript text needs a `NO_TEXT` row there, or a name without that
   word.
8. **Privacy holds for sends too.** `PendingMessage.text` is exactly what was
   typed, and it is never masked. Masking (#1488) applies only to text that
   leaves the desktop for display. The pending renderers draw the text as it
   was typed. The reconciliation rule reads masking markers only in the
   **record's** text, and never writes any. On the desktop,
   `privacy::admit` (called once, in `listener.rs`) passes the arguments of
   any command that is not in `TRANSCRIPT_TEXT` through untouched. For a
   command that is in the table, it removes only the `reveal` flag.
   `Plan::finish` masks only the command's **answer**.
   `a_command_without_transcript_text_passes_through_untouched` pins this
   with a secret-shaped argument.

## The reconciliation rule

`reconcilePending(pending, messages)` pairs each pending message with at most
one record, and each record with at most one pending message. A record answers
a pending message only if all four of these hold:

1. **The record is the user speaking.** It is a `user_prompt` that is not
   `is_meta`, or a `queued_prompt`. A message sent while Claude is busy is
   recorded as queued. A sidechain record never matches.
2. **The record comes after `after`.** `after` is the newest message the host
   held when the send began. This check applies only while `after` is still
   held. Because the transcript is append-only, the check rules out an older
   prompt with identical text.
3. **The record's timestamp falls within the send window.** The window runs
   from `createdAt - 30 s` to `createdAt + 10 min`. The 30 s allows for the
   phone's clock differing from the desktop's. The 10 min covers a queued
   message, which is written when Claude takes it up. A record with no
   timestamp, or one that cannot be parsed, never matches. A wrong match
   would hide a message that was never delivered.
4. **The record's text is the sent text.** Text is compared after two steps:
   - normalise `\r\n` and trim the outer whitespace;
   - join the record's text blocks with `\n`.

   Two exceptions apply. A span the desktop masked before sending it to a
   phone matches any non-empty run of text. A clipped block matches as a
   prefix.

Pending messages are paired oldest first, each with the earliest record that
qualifies. Sending "yes" twice therefore pairs each send with its own record.

A matched pending message is dropped from the host's state, whatever its state
was, because the transcript records what happened. Nothing in 7.9 expires a
pending message. "Not seen yet" is only true while the transcript is still
being read, so when to give up is a decision for 7.10's send path.

## The idempotency key

- `clientId` is a UUID made on the sending device when the pending message is
  created. It is sent **with** the command as its idempotency key.
- The desktop must remember the `clientId`s it has delivered, at least for the
  match window, and answer a repeated one with the first outcome instead of
  delivering again. A retry therefore resends the same `clientId`, never a new
  one.
- Step-up is a separate mechanism, if the send ends up `Destructive`. A
  step-up signature carries its own nonce, so a retry needs a fresh signature.
  It still carries the same `clientId`, and the nonce stops replays while the
  key stops double delivery.
- **TimedOut is not failed (#1466).** A timeout means the connection was made
  and no answer came within `CALL_TIMEOUT`. The command may have run. It sets
  the state to `unconfirmed`, never `failed`. `unconfirmed` tells the reader to
  check the session before sending again. The status copy is in
  `pendingStatus`. The state is `failed` only when the desktop answered and
  said it did **not** deliver.
- **The webview cannot tell a timeout apart yet.** The companion keeps
  `ClientError::TimedOut` separate from `Unreachable` (`src-mobile/src/client.rs`).
  `companion.rs` then hands the webview a plain error string, like any other
  failure. The send path must carry the timeout as structured data, so that
  the webview can set `unconfirmed` without reading the wording.
- **A retry never sends twice.** Resending an `unconfirmed` message reuses its
  `clientId`. If the first attempt was delivered, the desktop's key check
  refuses the second. If the transcript already holds the message, the pending
  message has already reconciled and there is nothing left to retry.

## What 7.10 must do before turning the flag on

- Decide the channel and the send command's class, and register the command
  in both surface files, with a `src/api/tauri.ts` wrapper and a transport test
  row.
- Pass `onSend` to both composers: `add(text)`, then the command with
  `clientId`, then `settle(...)` from its outcome.
- Decide when an unanswered pending message becomes `unconfirmed`, and say it
  only while the transcript is actually being read.
- Return a timed-out send to the webview as structured data rather than an
  error string (see "The webview cannot tell a timeout apart yet").
- Measure keyboard avoidance on an iPhone. Check whether the app shell's own
  bottom safe-area padding doubles the composer's.
