import { ChevronDown } from "lucide-react";
import { Popover } from "radix-ui";
import { ProfileEditor } from "@/components/ProfileEditor";
import { inkFor } from "@/lib/colors";
import { t } from "@/lib/i18n";
import { useProfile } from "@/lib/profile";
import { initials } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

// Who you are, always one click away: a small identity chip in the title bar
// that opens the name + colour editor. It replaces the Lobby's profile card,
// so the Lobby is about sessions and the identity is editable on every screen.
export function ProfileChip() {
  const profile = useProfile();
  const named = profile.name.trim().length > 0;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label={t("editProfile")}
          className={cn(
            "flex h-7 max-w-36 shrink-0 items-center gap-[7px] rounded-full bg-white/[0.06] pl-1 pr-2 text-[12px] font-medium transition-colors hover:bg-white/10 data-[state=open]:bg-white/10",
            named ? "text-foreground" : "text-muted-foreground",
            focusRing,
          )}
        >
          <span
            aria-hidden="true"
            className="flex size-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold transition-colors duration-300"
            style={
              named
                ? { backgroundColor: profile.color, color: inkFor(profile.color) }
                : { backgroundColor: "#3a3a42", color: "#a9a9b3" }
            }
          >
            {named ? initials(profile.name) : "?"}
          </span>
          <span className="truncate">{named ? profile.name : t("yourName")}</span>
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          collisionPadding={10}
          className="z-50 w-[300px] rounded-xl border bg-popover p-3.5 shadow-xl outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
        >
          <ProfileEditor autoFocus layoutId="profile-color-ring-chip" />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
