import { cn } from "@/lib/utils";

// Grouped-list building blocks shared by every screen: a quiet section label,
// a card that holds rows, and the hairline between rows (inset past the
// 32px avatar so faces line up in one column).

export function SectionLabel({
  children,
  aside,
  className,
}: {
  children: React.ReactNode;
  aside?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex shrink-0 items-baseline justify-between gap-2 px-1", className)}>
      <h3 className="text-[12px] font-semibold text-foreground/65">{children}</h3>
      {aside != null && (
        <span className="text-[12px] tabular-nums text-muted-foreground">{aside}</span>
      )}
    </div>
  );
}

export function Group({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      className={cn("flex shrink-0 flex-col overflow-hidden rounded-xl bg-card", className)}
      {...props}
    />
  );
}

export function Divider({ inset = true }: { inset?: boolean }) {
  return <div aria-hidden="true" className={cn("h-px shrink-0 bg-white/[0.07]", inset && "ml-14")} />;
}
