import { t } from "@/lib/i18n";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { cn } from "@/lib/utils";

/** The track a peer is on, by name, or in words ("Track 7") when it has none or
 *  the user asked for numbers only, as plain text in the right column of the Session
 *  roster and the overlay HUD. A name of only spaces counts as none, it would show
 *  as nothing. Tracks can share a name, so the title says which one. Renders
 *  nothing when they are not on a track. */
export function TrackLabel({
  track,
  name,
  small,
}: {
  track?: number | null;
  name?: string;
  small?: boolean;
}) {
  const { trackNumbersOnly } = useOverlaySettings();
  const cur = track != null && track > 0 ? track : null;
  if (cur == null) return null;
  const number = t("trackN", { n: cur });
  const label = trackNumbersOnly ? "" : name?.trim();
  return (
    <span
      title={label ? `${number}: ${label}` : undefined}
      className={cn(
        "shrink-0 truncate tabular-nums text-muted-foreground",
        small ? "max-w-[92px] text-[11px]" : "max-w-[112px] text-[12px]",
      )}
    >
      {label || number}
    </span>
  );
}
