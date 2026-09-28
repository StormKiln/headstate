// Vendored from the shadcn registry, style `base-nova` (#1479):
// `npx shadcn@4.21.0 add message`, which resolved
// https://ui.shadcn.com/r/styles/base-nova/message.json on 2026-09-26.
// `cn` comes from `@/lib/utils` rather than the registry's `cn` npm
// package, for the reason `message-scroller.tsx` gives. A SUBSET:
// `MessageGroup` and `MessageAvatar` were removed rather than exported
// unused (knip; see `sheet.tsx`). Re-add them from the registry when a
// renderer needs them. `MessageFooter` was re-added verbatim from the
// same registry file for the desktop renderer's turn footer (#1480).
import * as React from "react"
import { cn } from "@/lib/utils"

function Message({
  className,
  align = "start",
  ...props
}: React.ComponentProps<"div"> & { align?: "start" | "end" }) {
  return (
    <div
      data-slot="message"
      data-align={align}
      className={cn(
        "group/message relative flex w-full min-w-0 gap-2 text-sm data-[align=end]:flex-row-reverse",
        className
      )}
      {...props}
    />
  )
}

function MessageContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="message-content"
      className={cn(
        "flex w-full min-w-0 flex-col gap-2.5 wrap-break-word group-data-[align=end]/message:*:data-slot:self-end",
        className
      )}
      {...props}
    />
  )
}

function MessageHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="message-header"
      className={cn(
        "flex max-w-full min-w-0 items-center px-3 text-xs font-medium text-muted-foreground group-has-data-[variant=ghost]/message:px-0",
        className
      )}
      {...props}
    />
  )
}

function MessageFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="message-footer"
      className={cn(
        "flex max-w-full min-w-0 items-center px-3 text-xs font-medium text-muted-foreground group-has-data-[variant=ghost]/message:px-0 group-data-[align=end]/message:justify-end",
        className
      )}
      {...props}
    />
  )
}

export { Message, MessageContent, MessageFooter, MessageHeader }
