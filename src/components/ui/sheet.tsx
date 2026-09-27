import * as React from "react"
import { Dialog as SheetPrimitive } from "@base-ui/react/dialog"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

function Sheet({ ...props }: SheetPrimitive.Root.Props) {
  return <SheetPrimitive.Root data-slot="sheet" {...props} />
}



function SheetPortal({ ...props }: SheetPrimitive.Portal.Props) {
  return <SheetPrimitive.Portal data-slot="sheet-portal" {...props} />
}

function SheetOverlay({ className, ...props }: SheetPrimitive.Backdrop.Props) {
  return (
    <SheetPrimitive.Backdrop
      data-slot="sheet-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-black/10 transition-opacity duration-150 motion-reduce:transition-none data-ending-style:opacity-0 data-starting-style:opacity-0 supports-backdrop-filter:backdrop-blur-xs",
        className
      )}
      {...props}
    />
  )
}

/// Each side's default size, on the SAME tailwind-merge keys a caller
/// uses, so a caller's `w-*`, `h-*` or `max-w-*` REPLACES the default
/// instead of sitting beside it (#1314).
///
/// These used to live in the base string keyed by side:
/// `data-[side=right]:w-3/4`, `data-[side=right]:sm:max-w-sm`,
/// `data-[side=bottom]:h-auto`. `cn` is `twMerge`, which keys a
/// `data-[side=right]:` class separately from a bare one, so a caller's
/// `w-full` or `sm:max-w-md` survived the merge beside the default, then
/// lost in the browser, because the attribute selector is more specific.
/// Measured in Chrome before this change: `HelpButton` asked for 448px
/// and got 384px; the phone subagent sheet asked for `h-[90dvh]` and got
/// its content height (93px for a one-line body); the navigation sheet
/// asked for `w-72` (288px) and got 3/4 of the viewport (292.5px at 390).
///
/// The side is known here, in JS, so no `data-[side=*]` variant is needed
/// to choose the size. The cap is bare `max-w-sm`, not `sm:max-w-sm`, for
/// the reason #1306 gave `DialogContent`: callers write bare widths, and
/// a breakpoint-keyed cap is a separate key they would silently lose to.
///
/// The safe-area padding below stays `data-[side=*]`-keyed ON PURPOSE:
/// there the separate key is the point, so it survives a caller's `p-0`
/// (#648).
const SIDE_SIZE = {
  left: "h-full w-3/4 max-w-sm",
  right: "h-full w-3/4 max-w-sm",
  top: "h-auto",
  bottom: "h-auto",
} as const

function SheetContent({
  className,
  children,
  side = "right",
  showCloseButton = true,
  ...props
}: SheetPrimitive.Popup.Props & {
  side?: "top" | "right" | "bottom" | "left"
  showCloseButton?: boolean
}) {
  return (
    <SheetPortal>
      <SheetOverlay />
      <SheetPrimitive.Popup
        data-slot="sheet-content"
        data-side={side}
        className={cn(
          // Safe-area padding per side, so a sheet does not paint under the
          // status bar or the home indicator (#648). `env()` is zero on
          // every desktop platform, so these are literally no-ops there;
          // the phone is the only place they have an effect.
          "data-[side=left]:pt-[env(safe-area-inset-top)] data-[side=left]:pb-[env(safe-area-inset-bottom)] data-[side=right]:pt-[env(safe-area-inset-top)] data-[side=right]:pb-[env(safe-area-inset-bottom)] data-[side=top]:pt-[env(safe-area-inset-top)] data-[side=bottom]:pb-[env(safe-area-inset-bottom)]",
          "fixed z-50 flex flex-col gap-4 bg-popover bg-clip-padding text-sm text-popover-foreground shadow-lg transition duration-200 ease-in-out motion-reduce:transition-none data-ending-style:opacity-0 data-starting-style:opacity-0 data-[side=bottom]:inset-x-0 data-[side=bottom]:bottom-0 data-[side=bottom]:border-t data-[side=bottom]:data-ending-style:translate-y-[2.5rem] data-[side=bottom]:data-starting-style:translate-y-[2.5rem] data-[side=left]:inset-y-0 data-[side=left]:left-0 data-[side=left]:border-r data-[side=left]:data-ending-style:translate-x-[-2.5rem] data-[side=left]:data-starting-style:translate-x-[-2.5rem] data-[side=right]:inset-y-0 data-[side=right]:right-0 data-[side=right]:border-l data-[side=right]:data-ending-style:translate-x-[2.5rem] data-[side=right]:data-starting-style:translate-x-[2.5rem] data-[side=top]:inset-x-0 data-[side=top]:top-0 data-[side=top]:border-b data-[side=top]:data-ending-style:translate-y-[-2.5rem] data-[side=top]:data-starting-style:translate-y-[-2.5rem]",
          // Each side's size, on plain keys so the caller's replaces it
          // (#1314) -- see `SIDE_SIZE`.
          SIDE_SIZE[side],
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <SheetPrimitive.Close
            data-slot="sheet-close"
            render={
              <Button
                variant="ghost"
                className="absolute top-3 right-3"
                size="icon-sm"
              />
            }
          >
            <XIcon
            />
            <span className="sr-only">Close</span>
          </SheetPrimitive.Close>
        )}
      </SheetPrimitive.Popup>
    </SheetPortal>
  )
}

function SheetHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sheet-header"
      className={cn("flex flex-col gap-0.5 p-4", className)}
      {...props}
    />
  )
}


function SheetTitle({ className, ...props }: SheetPrimitive.Title.Props) {
  return (
    <SheetPrimitive.Title
      data-slot="sheet-title"
      className={cn(
        "font-heading text-base font-medium text-foreground",
        className
      )}
      {...props}
    />
  )
}


// A SUBSET of the generated component. `SheetTrigger`, `SheetClose`,
// `SheetFooter` and `SheetDescription` were removed rather than exported
// unused: knip fails the build on an unused export, and exempting this
// directory wholesale would also hide a component nobody renders --
// which is what that gate is for. Re-add one from the shadcn source if
// a help panel ever needs it.
export {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
}
