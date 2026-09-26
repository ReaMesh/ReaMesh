import { t } from "@/lib/i18n";
import { cn } from "@/lib/utils";

/** A peer's current track in words ("Track 7"), as plain text in the right
 *  column of the Session roster and the overlay HUD. Renders nothing when they
 *  are not on a track. */
export function TrackNumber({ track, small }: { track?: number | null; small?: boolean }) {
  const cur = track != null && track > 0 ? track : null;
  if (cur == null) return null;
  return (
    <span
      className={cn(
        "shrink-0 whitespace-nowrap tabular-nums text-muted-foreground",
        small ? "text-[11px]" : "text-[12px]",
      )}
    >
      {t("trackN", { n: cur })}
    </span>
  );
}
