import { useEffect, useRef } from "react";
import { AnimatePresence, motion } from "motion/react";
import { ChevronDown } from "lucide-react";
import { ProfileEditor } from "@/components/ProfileEditor";
import { inkFor } from "@/lib/colors";
import { t } from "@/lib/i18n";
import { setProfileEditorOpen, useProfile, useProfileEditor } from "@/lib/profile";
import { initials } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

/** One curve for everything the chip unfolds: panel, Lobby card, chevron. */
export const SURFACE_EASE = { duration: 0.28, ease: [0.32, 0.72, 0, 1] } as const;

// Who you are, always one click away: a small identity chip in the title bar.
// It stays put and toggles the name + colour editor, its chevron turning over:
// a panel growing out of the chip's corner on most screens, a card unfolding at
// the top of the page on the Lobby (which renders that card itself, see
// `ProfileCard` there).
export function ProfileChip() {
  const profile = useProfile();
  const { open, inline } = useProfileEditor();
  const named = profile.name.trim().length > 0;
  const root = useRef<HTMLDivElement>(null);
  const floating = open && !inline;

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setProfileEditorOpen(false);
    // Only the floating panel closes on an outside click; the Lobby card is
    // part of the page. The chip itself is inside `root`, so its own click
    // toggles instead of closing and reopening.
    const onDown = (e: MouseEvent) => {
      if (floating && e.target instanceof Node && !root.current?.contains(e.target))
        setProfileEditorOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open, floating]);

  return (
    <div ref={root} className="relative shrink-0">
      <button
        type="button"
        aria-label={t("editProfile")}
        aria-expanded={open}
        onClick={() => setProfileEditorOpen(!open)}
        className={cn(
          "flex h-7 max-w-36 shrink-0 items-center gap-[7px] rounded-full pl-1 pr-2 text-[12px] font-medium transition-colors hover:bg-white/10",
          open ? "bg-white/10" : "bg-white/[0.06]",
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
        <motion.span
          className="flex shrink-0"
          initial={false}
          animate={{ rotate: open ? 180 : 0 }}
          transition={SURFACE_EASE}
        >
          <ChevronDown className="size-3 text-muted-foreground" aria-hidden="true" />
        </motion.span>
      </button>
      <AnimatePresence>
        {floating && (
          <motion.div
            role="dialog"
            aria-label={t("editProfile")}
            initial={{ opacity: 0, scale: 0.9, y: -6 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.9, y: -6 }}
            transition={SURFACE_EASE}
            style={{ transformOrigin: "top right" }}
            className="absolute top-full right-0 z-50 mt-1.5 w-[300px] rounded-[14px] border bg-popover p-3.5 shadow-xl"
          >
            <ProfileEditor autoFocus />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
