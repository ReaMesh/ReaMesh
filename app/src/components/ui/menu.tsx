import * as React from "react"
import { DropdownMenu as MenuPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

// Small action menu (the roster's "⋯"). Same surface as dialogs and popovers.

const Menu = MenuPrimitive.Root
const MenuTrigger = MenuPrimitive.Trigger

function MenuContent({
  className,
  ...props
}: React.ComponentProps<typeof MenuPrimitive.Content>) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Content
        align="end"
        sideOffset={4}
        collisionPadding={10}
        className={cn(
          "z-50 min-w-44 rounded-xl border bg-popover p-1 shadow-xl outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
          className
        )}
        {...props}
      />
    </MenuPrimitive.Portal>
  )
}

function MenuItem({
  className,
  destructive,
  ...props
}: React.ComponentProps<typeof MenuPrimitive.Item> & { destructive?: boolean }) {
  return (
    <MenuPrimitive.Item
      className={cn(
        "flex h-8 cursor-default items-center gap-2 rounded-lg px-2 text-[12.5px] outline-none select-none data-[disabled]:opacity-40 data-[highlighted]:bg-white/[0.07] [&_svg]:size-3.5 [&_svg]:shrink-0 [&_svg]:text-muted-foreground",
        destructive && "text-red-400 [&_svg]:text-red-400",
        className
      )}
      {...props}
    />
  )
}

function MenuSeparator() {
  return <MenuPrimitive.Separator className="mx-1 my-1 h-px bg-white/[0.07]" />
}

export { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger }
