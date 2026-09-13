/// The banner for a worktree scan that could not read everything (#951).
///
/// # The third answer
///
/// A scan has three outcomes, not two. "We have not looked yet", "we
/// looked and there is nothing", and "we looked and could not tell" are
/// three different claims with three different remedies, and this
/// component is the third one. Before #951 the walk had nowhere to put it:
/// `scan_dirs_fast` returned a bare `Vec<Repo>`, so a repository whose
/// worktree listing failed was simply absent and read as "not a
/// repository".
///
/// `RepoPickerSidebar`'s empty copy is why that mattered most --
/// "No repositories found in the scanned folders" is a DIAGNOSIS pointing
/// at the user's settings, so a failed scan sent someone to fix a
/// configuration that was never wrong. `emptyStateGuard.test.ts` names it
/// the worst copy of the six it audits, and gave it an `isError` arm that
/// could never fire because the command could not reject.
///
/// # Why this is not an error panel
///
/// `QueryError` blanks the list and offers a retry, which is right when
/// the whole query failed. This is the OTHER case: some of the scan
/// succeeded. The repositories that did read are real and worth showing --
/// the trade `ArtifactsPage` states -- so this sits ABOVE the list rather
/// than instead of it, and offers no retry: an unreadable path is
/// unreadable for a reason that a second identical walk will not change,
/// and #846's `retry: false` reasoning is about exactly this.
///
/// # Why the reasons are shown
///
/// A count alone ("3 paths could not be read") is unactionable. The Rust
/// side sends `<path>: <why>` per entry for the reason
/// `claude/transcript.rs` gives for a message over a boolean: git refusing
/// a repository owned by another uid, a permission wall, and git missing
/// from a GUI-launched app's PATH send the user to three different places,
/// and only the message distinguishes them.
export function PartialScanNotice({
  unreadable,
  /// What the numbers on this surface are, given the shortfall. Each
  /// surface says its own, because "at least N repositories" and "at
  /// least N orphans" are different claims and a shared phrasing would
  /// be vaguer than either.
  consequence,
}: {
  unreadable: readonly string[];
  consequence: string;
}) {
  if (unreadable.length === 0) return null;
  const n = unreadable.length;
  return (
    <div
      role="alert"
      className="border-b border-[#30363d] bg-[#161b22] px-4 py-2 text-xs text-[#8b949e]"
    >
      <p>
        {n} path{n === 1 ? "" : "s"} could not be read, so {consequence}
      </p>
      {/* Every entry, not the first few. There are at most a handful in
          practice -- one per unreadable directory under the scan roots --
          and a truncated list would hide the one the user needs. `<ul>`
          rather than a joined string so a screen reader announces a
          count. */}
      <ul className="mt-1 space-y-0.5">
        {unreadable.map((u) => (
          <li key={u} className="break-all font-mono text-[#6e7681]">
            {u}
          </li>
        ))}
      </ul>
    </div>
  );
}
