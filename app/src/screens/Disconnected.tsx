import { motion } from "motion/react";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

async function launchReaper() {
  try {
    await openPath("/Applications/REAPER.app");
  } catch {
    toast.error(t("couldntOpenReaper"));
  }
}

// Shown until a REAPER instance with the ReaMesh extension answers on
// localhost. A calm "breathing" beacon reads as "listening on the network"
// without the busy spin of a progress wheel; it flashes to a found-blue the
// instant REAPER answers (`found`), just before the screen hands off.
export function Disconnected({ found = false }: { found?: boolean }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-6 px-10 text-center">
      <BreathingBeacon found={found} />

      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">
          {found ? t("reaperFound") : t("waitingForReaper")}
        </h2>
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          {found ? t("connecting") : t("startReaperToConnect")}
        </p>
      </div>

      <div className="flex w-full max-w-[240px] flex-col items-center gap-3">
        <Button className="w-full" onClick={launchReaper} disabled={found}>
          {t("openReaper")}
        </Button>
      </div>
    </div>
  );
}

// Concrete color values (mirror --muted-foreground / --found in index.css).
// Hardcoded rather than `var(...)` because Motion can only interpolate between
// real colors; a CSS variable would snap instead of easing grey→blue. The
// theme is always-dark (one value each), so there's no light-mode variant.
const SCAN_COLOR = "#8b8b94"; // --muted-foreground
const FOUND_COLOR = "#6aa6f0"; // --found

// A center dot with two rings that expand and fade outward on a stagger:
// a heartbeat, not a spinner. Muted grey while scanning; when REAPER answers
// the beacon eases its color to found-blue *in place*; the breathing loop
// keeps running, so it never replays its entrance, it just tints over.
function BreathingBeacon({ found }: { found: boolean }) {
  const color = found ? FOUND_COLOR : SCAN_COLOR;
  return (
    <div className="flex h-11 w-11 items-center justify-center">
      <div className="relative flex items-center justify-center">
        {[0, 1].map((i) => (
          <motion.span
            key={i}
            className="absolute size-[9px] rounded-full border-[1.5px]"
            initial={{ scale: 1, opacity: 0, borderColor: SCAN_COLOR }}
            animate={{ scale: 4.2, opacity: [0, 0.5, 0], borderColor: color }}
            transition={{
              scale: {
                duration: 2,
                repeat: Infinity,
                ease: [0, 0.2, 0.8, 1],
                delay: i,
              },
              // Start and end at zero opacity to hide the loop boundary. Fade
              // out before maximum scale so the outer edge does not linger.
              opacity: {
                duration: 2,
                repeat: Infinity,
                ease: "easeOut",
                times: [0, 0.2, 0.8],
                delay: i,
              },
              // Color changes independently so the pulse does not restart.
              borderColor: { duration: 0.45, ease: "easeOut" },
            }}
          />
        ))}
        <motion.span
          className="size-[9px] rounded-full"
          initial={{ backgroundColor: SCAN_COLOR }}
          animate={{ scale: found ? [1, 1.35, 1] : 1, backgroundColor: color }}
          transition={{
            scale: { duration: 0.5, ease: "easeOut" },
            backgroundColor: { duration: 0.45, ease: "easeOut" },
          }}
        />
      </div>
    </div>
  );
}
