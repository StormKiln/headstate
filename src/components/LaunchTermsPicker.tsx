import { useEffect, useState } from "react";
import {
  claudeLaunchTerms,
  type LaunchPreview,
  type LaunchTermOptions,
  type LaunchTerms,
} from "@/api/tauri";

/// Choosing which model and how much autonomy, and reading the line
/// that will run before it runs (#1214).
///
/// # Why the choices are fetched rather than listed here
///
/// `claude_launch_terms` serves the tokens `terms::Terms::parse`
/// accepts, so the list this renders and the list Rust admits are the
/// SAME list. A `const MODELS = ["opus", "sonnet"]` here would compile,
/// render and drift -- and a drifted copy shows a button whose value
/// the backend then refuses, which is a launch that fails from a
/// control that promised it would work.
///
/// Nothing typed by a user reaches this component: the values are
/// tokens chosen from that served list, they travel as tokens, and Rust
/// turns them into `&'static str` flags. There is no free-text field
/// here and there must not be one.
///
/// # Why the preview is not optional
///
/// The clipboard path let the user read the line before pasting it. A
/// spawn path takes that away, which is the same loss
/// `LaunchError::CwdMissing` reasons about -- spawning on someone's
/// behalf has to be stricter, because they are not reading the line
/// before it runs. So the built argv is shown, from Rust, as the words
/// the spawner will receive.

/// How the preview is fetched. Supplied by the caller because the two
/// launch sites build their command from different things -- a worktree
/// from its repo, path and branch; a session from its id and cwd -- and
/// this component knows about neither.
export type PreviewFn = (terms: LaunchTerms) => Promise<LaunchPreview>;

/// One argv word, rendered so its boundaries are visible.
///
/// Each word is its own chip rather than a space-joined line, which is
/// the whole point: "could a `;` in this path become a second command"
/// is a question about where the word boundaries are, and a joined
/// string answers it by hiding them. A path containing a space shows as
/// ONE chip, which is the visible form of the property `launch.rs`
/// guarantees.
function ArgvWord({ word }: { word: string }) {
  return (
    <code className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 font-mono text-[11px] text-[#e6edf3]">
      {word === "" ? <span className="text-[#8b949e]">(empty)</span> : word}
    </code>
  );
}

export function LaunchTermsPicker({
  terms,
  onChange,
  preview,
  disabled = false,
}: {
  terms: LaunchTerms;
  onChange: (next: LaunchTerms) => void;
  preview: PreviewFn;
  /// Set while a launch is in flight, so the terms cannot be changed
  /// out from under a command already on its way.
  disabled?: boolean;
}) {
  const [options, setOptions] = useState<LaunchTermOptions | null>(null);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const [built, setBuilt] = useState<LaunchPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  // The vocabulary, once. It is a constant on the Rust side, so there
  // is nothing to refetch and no staleness to manage.
  useEffect(() => {
    let live = true;
    void claudeLaunchTerms().then(
      (o) => {
        if (live) setOptions(o);
      },
      (e: unknown) => {
        // NAMED rather than silently rendering an empty picker. A
        // picker with no choices reads as "this build offers none",
        // which is a different fact from "the list could not be read".
        if (live) setOptionsError(typeof e === "string" ? e : "the choices could not be read");
      },
    );
    return () => {
      live = false;
    };
  }, []);

  // The built line, refetched whenever the terms change -- because the
  // whole claim of this panel is that what is shown is what will run,
  // and a preview one choice behind would be a false one.
  //
  // `live` guards against an out-of-order response overwriting a newer
  // one: the model select can be clicked twice faster than two round
  // trips resolve, and the stale answer would otherwise win.
  useEffect(() => {
    let live = true;
    void preview(terms).then(
      (p) => {
        if (!live) return;
        setBuilt(p);
        setPreviewError(null);
      },
      (e: unknown) => {
        if (!live) return;
        // The preview reports the same refusals the launch would --
        // "no terminal configured", "the template is unusable" -- so
        // this is where the user finds out, before pressing anything.
        setBuilt(null);
        setPreviewError(typeof e === "string" ? e : "the command line could not be built");
      },
    );
    return () => {
      live = false;
    };
  }, [preview, terms]);

  const unattended =
    terms.permissionMode != null && (options?.unattended ?? []).includes(terms.permissionMode);

  return (
    <div className="mt-2 rounded-md border border-[#30363d] bg-[#161b22] p-2">
      {optionsError !== null ? (
        <p className="text-[11px] text-[#f85149]">
          The launch terms could not be read ({optionsError}), so this will start on whatever
          the binary does by default.
        </p>
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <label className="flex items-center gap-1.5 text-[11px] text-[#8b949e]">
            Model
            <select
              className="tap-target rounded-md border border-[#30363d] bg-[#0d1117] px-1.5 py-1 text-[11px] text-[#e6edf3]"
              disabled={disabled || options === null}
              value={terms.model ?? ""}
              onChange={(e) =>
                onChange({ ...terms, model: e.target.value === "" ? null : e.target.value })
              }
            >
              {/* The empty option is a real choice and says so: "say
                  nothing", which is what every launch did before this
                  existed and what the binary's own settings decide. */}
              <option value="">Leave to Claude Code</option>
              {(options?.models ?? []).map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-1.5 text-[11px] text-[#8b949e]">
            Permissions
            <select
              className="tap-target rounded-md border border-[#30363d] bg-[#0d1117] px-1.5 py-1 text-[11px] text-[#e6edf3]"
              disabled={disabled || options === null}
              value={terms.permissionMode ?? ""}
              onChange={(e) =>
                onChange({
                  ...terms,
                  permissionMode: e.target.value === "" ? null : e.target.value,
                })
              }
            >
              <option value="">Leave to Claude Code</option>
              {(options?.permissionModes ?? []).map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </label>
        </div>
      )}

      {/* The one consequence worth stating in words. Which mode is the
          unattended one comes from Rust (`is_unattended`), not from a
          second judgment here. */}
      {unattended ? (
        <p className="mt-1.5 text-[11px] text-[#d29922]">
          This session will act without asking. Nothing it does in that worktree will stop for
          a prompt.
        </p>
      ) : null}

      <p className="mt-2 text-[11px] text-[#8b949e]">
        These names are Claude Code&apos;s, and Headstate cannot ask the installed binary which
        it accepts. If one is wrong the session will refuse to start, which is why the exact
        line is below.
      </p>

      <div className="mt-2">
        <p className="text-[11px] font-semibold text-[#e6edf3]">This is what will run:</p>
        {previewError !== null ? (
          <p className="mt-1 text-[11px] text-[#f85149]">{previewError}</p>
        ) : built === null ? (
          <p className="mt-1 text-[11px] text-[#8b949e]">Building the command line…</p>
        ) : (
          <>
            <div className="mt-1 flex flex-wrap items-center gap-1">
              <ArgvWord word={built.program} />
              {built.args.map((a, i) => (
                // The index is the key on purpose: these are positional
                // argv slots, two of which can legitimately be the same
                // string, and the position IS the identity.
                <ArgvWord key={i} word={a} />
              ))}
            </div>
            <p className="mt-1 text-[11px] text-[#8b949e]">
              Each box is one argument. Nothing is passed through a shell, so a space or a{" "}
              <code className="font-mono">;</code> inside a box stays inside it.
            </p>
          </>
        )}
      </div>
    </div>
  );
}
