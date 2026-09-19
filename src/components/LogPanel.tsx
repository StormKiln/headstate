import { useState } from "react";
import { toast } from "sonner";
import { useLogTail } from "@/api/hooks";
import { copyText } from "@/lib/clipboard";
import { QueryError, errorMessage } from "./QueryError";

/// Bytes as a short human figure.
///
/// Rounded DOWN for the excerpt and the total alike, so "the last 64 KB
/// of 4.2 MB" never claims more than is there.
function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.floor(n / 1024)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/// The end of the diagnostic log, in the app (#1147).
///
/// Until now `reveal_log` was the only log-facing affordance: it opened
/// a Finder window, and a user who hit a failure had to leave the app,
/// find `~/Library/Logs` and read the file in another program. On the
/// phone they could not do even that, because there is no Finder to
/// reveal into -- and the log is precisely where the cause is written.
///
/// Collapsed by default. Reading the tail costs a file read and, on a
/// paired phone, a payload over the transport; a user who has not asked
/// for it should pay for neither.
///
/// The text is redacted on the Rust side before it crosses, which is
/// what makes this safe both on the remote transport and in the bug
/// report the copy button exists to feed.
export function LogPanel() {
  const [open, setOpen] = useState(false);
  const tail = useLogTail(open);

  return (
    <div className="mt-3">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
      >
        {open ? "Hide the log text" : "Read the log here"}
      </button>

      {open ? (
        <div className="mt-2">
          {tail.isLoading ? (
            <p className="text-xs text-[#8b949e]">Reading the log…</p>
          ) : tail.isError ? (
            // The message distinguishes "nothing logged yet" from "could
            // not read it" -- the Rust side keeps them apart precisely
            // so this does not report a fresh install as a failure.
            <QueryError
              title="Could not show the log"
              message={errorMessage(tail.error)}
              onRetry={() => void tail.refetch()}
            />
          ) : tail.data ? (
            <>
              {/* What this IS, before the text itself. A panel that
                  silently shows the tail is one where a user scrolls to
                  the top, sees no error and concludes there was none --
                  which is a false answer to the question they came
                  with. */}
              <p className="text-[11px] text-[#8b949e]">
                {tail.data.truncated
                  ? `Showing the last ${bytes(tail.data.total - tail.data.offset)} of ${bytes(
                      tail.data.total,
                    )}.`
                  : tail.data.total === 0
                    ? "The log is empty."
                    : `Showing all ${bytes(tail.data.total)}.`}{" "}
                <span className="break-all">{tail.data.path}</span>
              </p>
              <pre className="mt-1 max-h-96 overflow-auto rounded border border-[#30363d] bg-[#0d1117] px-2 py-1.5 font-mono text-[11px] leading-relaxed text-[#e6edf3]">
                {tail.data.text}
              </pre>
              <div className="mt-1.5 flex gap-2">
                <button
                  type="button"
                  onClick={() => {
                    const d = tail.data;
                    if (!d) return;
                    void copyText(d.text).then((failure) =>
                      failure === null
                        ? toast.success("Log copied to the clipboard")
                        : toast.error("Could not copy the log", { description: failure }),
                    );
                  }}
                  className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d]"
                >
                  Copy
                </button>
                {/* Manual rather than polled: the interesting moment is
                    "something just went wrong, show me", and a
                    background poll would spend a file read on every user
                    who leaves this page open and never looks. */}
                <button
                  type="button"
                  onClick={() => void tail.refetch()}
                  disabled={tail.isFetching}
                  className="tap-target rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50"
                >
                  {tail.isFetching ? "Refreshing…" : "Refresh"}
                </button>
              </div>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
