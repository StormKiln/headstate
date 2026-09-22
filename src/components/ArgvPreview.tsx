/// The one place an argv is rendered for a human to read before it runs.
///
/// # Why this is shared rather than written twice
///
/// It was written twice. `LaunchTermsPicker` (#1214) and
/// `ClaudifyAction` (#1292) each grew their own `ArgvWord` with the same
/// intent and different class lists, and only the second one handled
/// overflow -- so the launch dialog reached from a worktree page put a
/// `cd '<path>' && claude '<whole assessment prompt>'` on ONE unwrapped
/// line inside a `max-w-lg` dialog and pushed it off the right edge
/// (#1303). Two renderings of the same thing is how one of them gets
/// fixed. There is now one.
///
/// # The two properties this component exists to hold together
///
/// **Every word is readable.** `whitespace-pre-wrap` keeps the newlines
/// a brief actually contains, `break-words` breaks a path or a token too
/// long for the line, and `overflow-auto` under `max-h-56` bounds a word
/// that is pages long. Nothing scrolls sideways: horizontal scrolling is
/// what made the text unreadable, vertical is the fix.
///
/// **Every boundary is visible.** Each argv word is its own bordered
/// box, and that is not decoration -- it is the answer to "could a `;` in
/// this path become a second command". A space-joined line answers that
/// question by hiding it. So wrapping happens strictly INSIDE a box:
/// `block` gives each word its own row, and the boxes stack in a column
/// with a gap between them, so a word that wraps onto four lines is still
/// unmistakably four lines of ONE box rather than four words.
///
/// That ordering matters. A fix that made the text readable by joining or
/// unboxing the words would be a worse bug than the overflow, because the
/// boxes are the entire safety claim the preview makes.
function ArgvWord({ word }: { word: string }) {
  return (
    <code className="block max-h-56 overflow-auto overflow-x-hidden whitespace-pre-wrap break-words rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 font-mono text-[11px] text-[#e6edf3]">
      {word === "" ? <span className="text-[#8b949e]">(empty)</span> : word}
    </code>
  );
}

/// The program and its arguments, one box each.
///
/// `as` exists because the two callers sit in different element trees.
/// `LaunchTermsPicker` renders into a block `<div>`; `ClaudifyAction`
/// returns an inline `<span>` chain, where a nested `<div>` is invalid
/// HTML that React passes straight through to the DOM. The default is
/// `div`, so only the inline caller has to say anything.
export function ArgvPreview({
  program,
  args,
  as: As = "div",
}: {
  program: string;
  args: string[];
  as?: "div" | "span";
}) {
  return (
    <As className="mt-1 flex flex-col items-stretch gap-1">
      <ArgvWord word={program} />
      {args.map((a, i) => (
        // The index is the key on purpose: these are positional argv
        // slots, two of which can legitimately be the same string, and
        // the position IS the identity.
        <ArgvWord key={i} word={a} />
      ))}
    </As>
  );
}
