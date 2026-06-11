import { cn } from "@/lib/utils";
import { displayName, initials, type Peer } from "@/lib/types";

// Use the presence beacon's color so each peer looks consistent across
// the roster, toasts, and overlay.
export function PeerAvatar({
  peer,
  size = "md",
  className,
}: {
  peer: Peer;
  size?: "sm" | "md" | "lg";
  className?: string;
}) {
  const sizes = {
    sm: "size-6 text-[10px]",
    md: "size-8 text-xs",
    lg: "size-10 text-sm",
  };
  return (
    <div
      className={cn(
        "flex shrink-0 items-center justify-center rounded-full font-semibold text-black/80",
        sizes[size],
        className,
      )}
      style={{ backgroundColor: peer.color }}
      title={displayName(peer)}
    >
      {initials(displayName(peer))}
    </div>
  );
}
