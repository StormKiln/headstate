import { toast } from "sonner";
import { copyText } from "@/lib/clipboard";

/// What the success toast says, computed after the copy succeeds.
export interface CopiedToast {
  title: string;
  description?: string;
}

/// Copy some markdown, with a toast either way (#1399, lifted for #1578).
///
/// One implementation for every "Copy … as markdown" in the app: the
/// advice panel's report and groups, and the Ready for review strip. The
/// caller says what the toast should claim, because only it knows what
/// was counted.
///
/// The markdown is built on click, not on render: a list is rendered
/// far more often than it is copied. `copyText` reports the no-clipboard
/// case, and the toast is what makes the click visible either way --
/// never a silent failure.
export function CopyMarkdownButton({
  label,
  accessibleName,
  markdown,
  copied,
  className = "",
}: {
  label: string;
  /// Only where `label` alone would be ambiguous. It must START with the
  /// visible label, so a voice user can say what they see.
  accessibleName?: string;
  markdown: () => string;
  copied: () => CopiedToast;
  className?: string;
}) {
  return (
    <button
      type="button"
      aria-label={accessibleName}
      onClick={() => {
        void copyText(markdown()).then((failure) => {
          if (failure !== null) {
            toast.error("Could not copy the markdown", { description: failure });
            return;
          }
          const { title, description } = copied();
          toast.success(title, description === undefined ? undefined : { description });
        });
      }}
      className={`tap-target text-left text-[11px] text-[#58a6ff] hover:underline ${className}`}
    >
      {label}
    </button>
  );
}
