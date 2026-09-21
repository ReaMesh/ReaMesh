import { t } from "@/lib/i18n";
import { cn } from "@/lib/utils";

/** A peer's current track as a labelled pill ("Track 7"). Shared by the
 *  Session roster and the overlay HUD; renders nothing when they are not on a
 *  track. */
export function TrackNumber({ track, small }: { track?: number | null; small?: boolean }) {
  const cur = track != null && track > 0 ? track : null;
  if (cur == null) return null;
  return (
    <span
      className={cn(
        "shrink-0 rounded-md bg-white/5 font-medium tabular-nums text-foreground/65",
        small ? "px-1.5 py-px text-[10.5px]" : "px-[7px] py-0.5 text-[11px]",
      )}
    >
      {t("trackN", { n: cur })}
    </span>
  );
}
