/** A peer's current track number, right-aligned. Shared by the Session peer
 *  list and the overlay HUD roster. */
export function TrackNumber({ track }: { track?: number | null }) {
  const cur = track != null && track > 0 ? track : null;
  if (cur == null) return <span className="w-4" />;
  return (
    <span className="w-4 shrink-0 text-right text-[12px] tabular-nums text-muted-foreground">
      {cur}
    </span>
  );
}
