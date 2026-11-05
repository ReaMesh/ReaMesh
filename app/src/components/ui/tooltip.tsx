"use client"

import * as React from "react"
import { Tooltip as TooltipPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

function TooltipProvider({
  delayDuration = 0,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
}

function Tooltip({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

function TooltipTrigger({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

function TooltipContent({
  className,
  sideOffset = 0,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn(
          // Dark like the surfaces around it, set apart by its border and
          // shadow. After the wait it settles out of the trigger's direction;
          // moving on to the next tooltip within the skip window (instant-open)
          // has nothing to announce, so that one just appears. It leaves the
          // way it came, a little quicker.
          "z-50 w-fit origin-(--radix-tooltip-content-transform-origin) rounded-lg border border-border bg-background px-3 py-1.5 text-xs text-balance text-foreground shadow-lg motion-reduce:animate-none",
          "data-[state=delayed-open]:animate-in data-[state=delayed-open]:duration-220 data-[state=delayed-open]:ease-[cubic-bezier(0.16,1,0.3,1)] data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-[0.97]",
          "data-[side=bottom]:slide-in-from-top-1 data-[side=top]:slide-in-from-bottom-1 data-[side=left]:slide-in-from-right-1 data-[side=right]:slide-in-from-left-1",
          "data-[state=closed]:animate-out data-[state=closed]:duration-160 data-[state=closed]:ease-[cubic-bezier(0.4,0,1,1)] data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-[0.97]",
          "data-[side=bottom]:slide-out-to-top-[3px] data-[side=top]:slide-out-to-bottom-[3px] data-[side=left]:slide-out-to-right-[3px] data-[side=right]:slide-out-to-left-[3px]",
          className
        )}
        {...props}
      >
        {children}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  )
}

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider }
