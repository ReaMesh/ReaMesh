import { cn } from "@/lib/utils";
import { inkFor } from "@/lib/colors";
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
        "flex shrink-0 items-center justify-center rounded-full font-semibold",
        sizes[size],
        className,
      )}
      // Ink chosen from the background rather than fixed: peer colours are not
      // limited to the app's palette (the beacon carries any hex, and the
      // extension derives an arbitrary hue from the peer id when it's unset).
      style={{ backgroundColor: peer.color, color: inkFor(peer.color) }}
      role="img"
      aria-label={displayName(peer)}
    >
      {initials(displayName(peer))}
    </div>
  );
}
