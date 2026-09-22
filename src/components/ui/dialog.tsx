import * as React from "react"
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

function Dialog({ ...props }: DialogPrimitive.Root.Props) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({ ...props }: DialogPrimitive.Trigger.Props) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: DialogPrimitive.Portal.Props) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) {
  return (
    <DialogPrimitive.Backdrop
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 isolate z-50 bg-black/10 duration-100 supports-backdrop-filter:backdrop-blur-xs data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean
}) {
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Popup
        data-slot="dialog-content"
        className={cn(
          // WIDTH IS TWO CLASSES, AND THEY ARE BOTH UNPREFIXED ON PURPOSE.
          //
          // `w-[calc(100%-2rem)]` carries the side margin, NOT `max-w-`.
          // `max-w-sm` is the default cap, and it is deliberately NOT
          // spelt `sm:max-w-sm` (#1306). `twMerge` keys `max-w-*` and
          // `sm:max-w-*` SEPARATELY, so a `sm:`-prefixed cap here is not
          // replaced by a caller's bare `max-w-2xl` -- it survives
          // beside it, and above 640px the media-query rule wins on
          // specificity. Every one of the ~25 call sites passed the bare
          // form, so every dialog in the app rendered at 384px whatever
          // width it asked for: measured in Chrome at 1280px, `max-w-lg`,
          // `max-w-2xl` and `max-w-md` all computed to 384px, as did
          // `UpdateWizard`'s `w-[min(46rem,92vw)]`, which sets no
          // `max-w-` at all.
          //
          // Keeping the cap on the SAME key the callers use is what
          // disarms it: `cn("max-w-sm", "max-w-2xl")` is `"max-w-2xl"`,
          // so a caller's plain spelling simply works and there is no
          // longer a breakpoint-keyed cap for a third caller to lose to.
          // The `sm:` prefix also bought nothing below 640px -- the
          // `w-*` margin is narrower than 24rem on any phone, so the cap
          // never applied there anyway (measured: 358px at a 390px
          // viewport, cap inactive).
          //
          // Why the MARGIN is on `w-*` and not `max-w-*`, which is the
          // same trap one key over and the reason this comment exists:
          // the margin used to live on `max-w-[calc(100%-2rem)]`, and
          // `twMerge` treats `max-w-*` as one conflict key -- so all 18
          // call sites, every one of which passes its own `max-w-lg` or
          // `max-w-2xl`, REMOVED it rather than combining with it. At
          // 390px the popup was then exactly 390px: edge to edge, zero
          // side margin, rounded corners clipped by the screen, and no
          // backdrop strip left to tap to dismiss. `w-*` is a key the
          // callers do not set, so the margin now survives them.
          // The height budget subtracts the safe-area insets as well as the
          // 2rem margin, so a tall dialog on a notched phone stops short of
          // the status bar and the home indicator instead of scrolling its
          // first row underneath them (#648). `env()` is zero on the
          // desktop, where this reduces to the original calc.
          "fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100dvh-2rem-env(safe-area-inset-top)-env(safe-area-inset-bottom))] w-[calc(100%-2rem)] max-w-sm -translate-x-1/2 -translate-y-1/2 gap-4 overflow-y-auto overscroll-contain rounded-xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-foreground/10 duration-100 outline-none data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            render={
              <Button
                variant="ghost"
                className="absolute top-2 right-2"
                size="icon-sm"
              />
            }
          >
            <XIcon
            />
            <span className="sr-only">Close</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Popup>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-base leading-none font-medium",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
}
