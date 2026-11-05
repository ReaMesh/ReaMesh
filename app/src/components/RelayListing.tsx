import { useId, useState } from "react";
import { ChevronDown } from "lucide-react";
import { LayoutGroup, motion, useReducedMotion } from "motion/react";
import { Popover } from "radix-ui";
import { Hint } from "@/components/Hint";
import { Switch } from "@/components/ui/switch";
import type { RelayCode } from "@/lib/relay";
import { relayName, setRelayListed, useHiddenRelays } from "@/lib/relayLists";
import { cn, focusRing } from "@/lib/utils";

// Where a session is visible: one switch per relay that keeps a list, and
// nothing else. It opens from a quiet line with a chevron, the same one on the
// host card and in a running session, and looks like the app's other small
// menus (components/ui/menu.tsx): same surface, row height and gliding highlight.
// What the choice means is the line's tooltip, and was said once when the relay
// was added.

const HIGHLIGHT = { type: "spring", stiffness: 360, damping: 32, mass: 0.6 } as const;

function ListingRow({ relay, hidden }: { relay: RelayCode; hidden: string[] }) {
  const reduce = useReducedMotion();
  const [lit, setLit] = useState(false);
  const name = relayName(relay);
  const listed = !hidden.includes(relay.peer);
  return (
    <div
      className="relative isolate flex h-8 cursor-default items-center gap-3 rounded-lg px-2 text-[12.5px] select-none"
      onPointerEnter={() => setLit(true)}
      onPointerLeave={() => setLit(false)}
      // The whole row is the target; the switch is what a keyboard reaches.
      onClick={(e) => {
        if (!(e.target as HTMLElement).closest('[role="switch"]')) setRelayListed(relay.peer, !listed);
      }}
    >
      {lit && (
        <motion.span
          layoutId="listing-highlight"
          aria-hidden="true"
          className="absolute inset-0 -z-10 rounded-lg bg-white/[0.07]"
          transition={reduce ? { duration: 0 } : HIGHLIGHT}
        />
      )}
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <Switch label={name} checked={listed} onChange={(on) => setRelayListed(relay.peer, on)} />
    </div>
  );
}

/** A line of small text that opens the switches. */
export function RelayListingLine({
  relays,
  note,
  side,
  offset = 4,
  warn,
  children,
}: {
  relays: RelayCode[];
  /** What the current choice means, for the tooltip. */
  note: string;
  side: "top" | "bottom";
  /** Distance from the line, to clear what sits right next to it. */
  offset?: number;
  warn?: boolean;
  children: React.ReactNode;
}) {
  const hidden = useHiddenRelays();
  const [open, setOpen] = useState(false);
  const group = useId();
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Hint label={open ? undefined : note}>
        <Popover.Trigger asChild>
          <button
            type="button"
            className={cn(
              "flex max-w-full items-center gap-[3px] rounded text-left text-[12px] transition-colors",
              warn ? "text-amber-500" : "text-muted-foreground hover:text-foreground",
              focusRing,
            )}
          >
            <span className="truncate">{children}</span>
            <ChevronDown className="size-3 shrink-0" aria-hidden="true" />
          </button>
        </Popover.Trigger>
      </Hint>
      <Popover.Portal>
        <Popover.Content
          side={side}
          align="start"
          sideOffset={offset}
          collisionPadding={10}
          // Focus stays on the line: the switches are a Tab away, and a mouse
          // click shouldn't light one up.
          onOpenAutoFocus={(e) => e.preventDefault()}
          className="z-50 min-w-44 max-w-64 origin-(--radix-popover-content-transform-origin) rounded-xl border bg-popover p-1 shadow-xl outline-none duration-180 ease-[cubic-bezier(0.16,1,0.3,1)] data-[state=closed]:animate-out data-[state=closed]:duration-120 data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
        >
          <LayoutGroup id={group}>
            {relays.map((r) => (
              <ListingRow key={r.peer} relay={r} hidden={hidden} />
            ))}
          </LayoutGroup>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
