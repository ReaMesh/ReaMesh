import { Input } from "@/components/ui/input";
import { inkFor } from "@/lib/colors";
import { sendProfile } from "@/lib/connection";
import { t } from "@/lib/i18n";
import { PALETTE, saveProfile, setProfileEditorOpen, useProfile, type Profile } from "@/lib/profile";
import { initials } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

/** Persist an edit and push it to every connected REAPER at once. */
export function updateProfile(p: Profile) {
  saveProfile(p);
  sendProfile();
}

// Name field + colour swatches. Lives in the panel under the title-bar chip
// and, on the Lobby, in the card the chip morphs into.
export function ProfileEditor({
  autoFocus,
  ringOffset = "ring-offset-popover",
}: {
  autoFocus?: boolean;
  /** Surface the swatches sit on, so the selection ring's gap matches it. */
  ringOffset?: string;
}) {
  const profile = useProfile();
  return (
    <div className="flex items-center gap-3.5">
      <div
        aria-hidden="true"
        className="flex size-11 shrink-0 items-center justify-center rounded-full text-sm font-semibold transition-colors duration-300 ease-out"
        style={{ backgroundColor: profile.color, color: inkFor(profile.color) }}
      >
        {initials(profile.name || "?")}
      </div>
      <div className="min-w-0 flex-1 space-y-2.5">
        <Input
          value={profile.name}
          onKeyDown={(e) => e.key === "Enter" && setProfileEditorOpen(false)}
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
                "relative size-[18px] shrink-0 rounded-full ring-2 ring-transparent ring-offset-2 transition-shadow duration-200",
                ringOffset,
                profile.color !== c && "hover:ring-muted-foreground/50",
                focusRing,
              )}
              style={{ backgroundColor: c }}
            >
              {/* Plain CSS, no layout animation: the ring has to stay glued to
                  its swatch while the editor around it unfolds. */}
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
    </div>
  );
}
