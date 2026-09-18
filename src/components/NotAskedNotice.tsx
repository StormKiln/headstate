/// The panel shown when Headstate DECLINED to issue a request, as
/// distinct from one that was issued and failed.
///
/// # Why this is not `QueryError`
///
/// `QueryError` says something went wrong and offers "Try again". Both
/// halves are wrong here. Nothing went wrong -- no request was ever
/// constructed, because `GhClient` held no client -- and a retry cannot
/// succeed, since pressing it does not make a token appear. #1050 is
/// this exact defect on the stats surface: "a stats load the process
/// declined to issue reported that GitHub had not answered. Both halves
/// were wrong, and the retry offered could not work."
///
/// Withholding the retry follows `PartialScanNotice`'s reasoning: an
/// action whose second attempt is identical to its first should not be
/// offered as a remedy.
export function NotAskedNotice({ message, title }: { message: string; title?: string }) {
  return (
    <div
      role="status"
      className="rounded-md border border-[#30363d] bg-[#161b22] px-4 py-8 text-center"
    >
      <p className="text-sm font-semibold text-[#e6edf3]">
        {title ?? "Headstate has not asked GitHub"}
      </p>
      {/* The reason, then the remedy. The message is the Rust side's own
          prose with the wire marker stripped -- it already names the
          command to run. */}
      <p className="mx-auto mt-2 max-w-lg break-words text-sm text-[#8b949e]">{message}</p>
      <p className="mx-auto mt-2 max-w-lg text-xs text-[#8b949e]">
        Sign in and relaunch Headstate. Nothing was sent to GitHub, so there is nothing to
        retry.
      </p>
    </div>
  );
}
