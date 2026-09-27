import { useContext, useId, useState, type ReactNode } from "react";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "../ui/sheet";
import { InSheet } from "./inSheet";
import { palette } from "./palette";
import type { ToolVariant } from "./types";

/// A collapsed region: "12 lines ▸" that opens in place on the desktop
/// and in a sheet on the phone (#1483).
///
/// The button's accessible name carries what it hides and how much
/// ("Output, 12 lines"), and `aria-expanded` its state, so a screen
/// reader announces the line count before the reader opens it (#1489).
export function Fold({
  label,
  count,
  variant,
  title,
  defaultOpen = false,
  children,
}: {
  /// What is folded: "Output", "Files", "Report".
  label: string;
  /// "12 lines", "at least 40 lines", "3 files".
  count: string;
  variant: ToolVariant;
  /// The sheet's heading on the phone.
  title: string;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const inSheet = useContext(InSheet);
  const [open, setOpen] = useState(defaultOpen || inSheet);
  const regionId = useId();
  const name = `${label}, ${count}`;

  if (variant === "compact" && !inSheet) {
    return (
      <Sheet open={open} onOpenChange={setOpen}>
        <button
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-label={name}
          onClick={() => setOpen(true)}
          className="mt-0.5 min-h-8 text-[12px] underline-offset-2 hover:underline focus-visible:outline focus-visible:outline-2"
          style={{ color: palette.muted }}
        >
          {count} ▸
        </button>
        <SheetContent side="bottom" className="max-h-[85vh] overflow-y-auto">
          <SheetHeader>
            <SheetTitle>{title}</SheetTitle>
          </SheetHeader>
          <div className="px-4 pb-4">{children}</div>
        </SheetContent>
      </Sheet>
    );
  }

  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={regionId}
        aria-label={name}
        onClick={() => setOpen((o) => !o)}
        className={`mt-0.5 hover:underline focus-visible:outline focus-visible:outline-2 ${
          // In a phone sheet: a finger's target, not a pointer's.
          variant === "compact" ? "min-h-8 text-[12px]" : "text-[11px]"
        }`}
        style={{ color: palette.muted }}
      >
        {count} {open ? "▾" : "▸"}
      </button>
      <div id={regionId} hidden={!open}>
        {open ? children : null}
      </div>
    </div>
  );
}
