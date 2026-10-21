import * as React from "react"
import { DropdownMenu as MenuPrimitive } from "radix-ui"
import { LayoutGroup, motion, useReducedMotion } from "motion/react"

import { cn } from "@/lib/utils"

// Small action menu (the roster's "⋯"). Same surface as dialogs and popovers.

// The highlight glides from row to row instead of jumping.
const HIGHLIGHT = { type: "spring", stiffness: 360, damping: 32, mass: 0.6 } as const

const Menu = MenuPrimitive.Root
const MenuTrigger = MenuPrimitive.Trigger

function MenuContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof MenuPrimitive.Content>) {
  // One highlight per open menu, so two menus never share it.
  const group = React.useId()
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Content
        align="end"
        sideOffset={4}
        collisionPadding={10}
        className={cn(
          // Grows out of the corner next to its trigger.
          "z-50 min-w-44 origin-(--radix-dropdown-menu-content-transform-origin) rounded-xl border bg-popover p-1 shadow-xl outline-none duration-180 ease-[cubic-bezier(0.16,1,0.3,1)] data-[state=closed]:animate-out data-[state=closed]:duration-120 data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
          className
        )}
        {...props}
      >
        <LayoutGroup id={group}>{children}</LayoutGroup>
      </MenuPrimitive.Content>
    </MenuPrimitive.Portal>
  )
}

function MenuItem({
  className,
  destructive,
  children,
  onFocus,
  onBlur,
  ...props
}: React.ComponentProps<typeof MenuPrimitive.Item> & { destructive?: boolean }) {
  const reduce = useReducedMotion()
  // Radix moves focus to the highlighted row, for the pointer and the keys alike.
  const [lit, setLit] = React.useState(false)
  return (
    <MenuPrimitive.Item
      className={cn(
        "relative isolate flex h-8 cursor-default items-center gap-2 rounded-lg px-2 text-[12.5px] outline-none select-none data-[disabled]:opacity-40 [&_svg]:size-3.5 [&_svg]:shrink-0 [&_svg]:text-muted-foreground",
        destructive && "text-red-400 [&_svg]:text-red-400",
        className
      )}
      onFocus={(e) => {
        setLit(true)
        onFocus?.(e)
      }}
      onBlur={(e) => {
        setLit(false)
        onBlur?.(e)
      }}
      {...props}
    >
      {lit && (
        <motion.span
          layoutId="menu-highlight"
          aria-hidden="true"
          className="absolute inset-0 -z-10 rounded-lg bg-white/[0.07]"
          transition={reduce ? { duration: 0 } : HIGHLIGHT}
        />
      )}
      {children}
    </MenuPrimitive.Item>
  )
}

function MenuSeparator() {
  return <MenuPrimitive.Separator className="mx-1 my-1 h-px bg-white/[0.07]" />
}

export { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger }
