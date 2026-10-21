import { Input } from "@/components/ui/input";
import { inkFor } from "@/lib/colors";
import { sendProfile } from "@/lib/connection";
import { t } from "@/lib/i18n";
import { PALETTE, announceProfile, saveProfile, useProfile, type Profile } from "@/lib/profile";
import { initials } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

/** Persist an edit and push it to every connected REAPER and the other
 *  windows at once. */
export function updateProfile(p: Profile) {
  saveProfile(p);
  sendProfile();
  announceProfile(p);
}

// Name field + colour swatches: the body of the Lobby's name card, where the
// row with your avatar sits right above them.
export function ProfileFields({
  autoFocus,
  onDone,
}: {
  autoFocus?: boolean;
  /** Enter or Escape in the name field: the Lobby folds its card away. */
  onDone?: () => void;
}) {
  const profile = useProfile();
  return (
    <div className="min-w-0 flex-1 space-y-2.5">
      <Input
        value={profile.name}
        onKeyDown={(e) => (e.key === "Enter" || e.key === "Escape") && onDone?.()}
        autoFocus={autoFocus}
        aria-label={t("yourName")}
        placeholder={t("yourName")}
        maxLength={32}
        className="h-8 border-0 bg-muted text-[13px]"
        onChange={(e) => updateProfile({ ...profile, name: e.target.value })}
      />
      {/* A radio group, not eight unrelated buttons: this colour is the
          user's identity in every roster, toast and overlay. */}
      <div role="radiogroup" aria-label={t("identityColor")} className="flex gap-[9px] px-0.5">
        {PALETTE.map((c, i) => (
          <button
            key={c}
            type="button"
            role="radio"
            aria-checked={profile.color === c}
            aria-label={t("identityColorN", { n: i + 1 })}
            onClick={() => updateProfile({ ...profile, color: c })}
            className={cn(
              "relative size-[18px] shrink-0 rounded-full ring-2 ring-transparent ring-offset-2 ring-offset-card transition-shadow duration-200",
              profile.color !== c && "hover:ring-muted-foreground/50",
              focusRing,
            )}
            style={{ backgroundColor: c }}
          >
            {/* Plain CSS, no layout animation: the ring has to stay glued to
                its swatch while the card around it unfolds. */}
            <span
              aria-hidden="true"
              className={cn(
                "absolute -inset-1 rounded-full border-2 border-foreground/70 transition-[opacity,scale] duration-200 ease-out",
                profile.color === c ? "scale-100 opacity-100" : "scale-75 opacity-0",
              )}
            />
          </button>
        ))}
      </div>
    </div>
  );
}

// The Profile section in Settings: your avatar next to the same fields.
export function ProfileEditor() {
  const profile = useProfile();
  return (
    <div className="flex items-center gap-3.5">
      <div
        aria-hidden="true"
        className="flex size-11 shrink-0 items-center justify-center rounded-full text-[15px] font-semibold transition-colors duration-300 ease-out"
        style={{ backgroundColor: profile.color, color: inkFor(profile.color) }}
      >
        {initials(profile.name)}
      </div>
      <ProfileFields />
    </div>
  );
}
