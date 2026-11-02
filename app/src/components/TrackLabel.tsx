import { t } from "@/lib/i18n";
import { cn } from "@/lib/utils";

/** The track a peer is on, by name, or in words ("Track 7") when it has none, as
 *  plain text in the right column of the Session roster and the overlay HUD.
 *  Renders nothing when they are not on a track. */
export function TrackLabel({
  track,
  name,
  small,
}: {
  track?: number | null;
  name?: string;
  small?: boolean;
}) {
  const cur = track != null && track > 0 ? track : null;
  if (cur == null) return null;
  return (
    <span
      title={name || undefined}
      className={cn(
        "shrink-0 truncate tabular-nums text-muted-foreground",
        small ? "max-w-[92px] text-[11px]" : "max-w-[112px] text-[12px]",
      )}
    >
      {name || t("trackN", { n: cur })}
    </span>
  );
}
