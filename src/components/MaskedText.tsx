import { Fragment } from "react";
import { MASK_LABELS, splitMasked } from "@/lib/masked";

/// Transcript text with each span the desktop masked drawn as a
/// "hidden" pill (#1488).
///
/// On the desktop nothing is masked and this renders the string itself,
/// adding no DOM. On a phone each `⟦hidden:<kind>⟧` marker becomes a pill
/// whose title says what kind of value was there -- never the value,
/// which did not leave the desktop.
export function MaskedText({ text }: { text: string }) {
  const parts = splitMasked(text);
  if (parts.length === 1 && "text" in parts[0]) return <>{text}</>;
  return (
    <>
      {parts.map((part, i) =>
        // Index keys: the parts ARE positional, and two runs of the same
        // text at different offsets are different parts.
        "text" in part ? (
          <Fragment key={i}>{part.text}</Fragment>
        ) : (
          <span
            key={i}
            title={`Hidden on this phone: ${MASK_LABELS[part.hidden]}`}
            aria-label={`hidden ${MASK_LABELS[part.hidden]}`}
            className="mx-0.5 inline-block rounded bg-[#30363d] px-1.5 align-baseline font-sans text-[10px] not-italic text-[#8b949e]"
          >
            hidden
          </span>
        ),
      )}
    </>
  );
}
