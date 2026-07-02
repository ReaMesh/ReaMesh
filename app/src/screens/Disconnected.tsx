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

// A center dot with two rings that expand and fade outward on a stagger:
// a heartbeat, not a spinner. Muted grey while scanning; the whole beacon
// switches to found-blue for the brief handoff beat.
function BreathingBeacon({ found }: { found: boolean }) {
  const color = found ? "var(--found)" : "var(--muted-foreground)";
  return (
    <div className="flex h-11 w-11 items-center justify-center">
      <div className="relative flex items-center justify-center">
        {[0, 1].map((i) => (
          <motion.span
            key={i}
            className="absolute size-[9px] rounded-full border-[1.5px]"
            style={{ borderColor: color }}
            initial={{ scale: 1, opacity: 0.55 }}
            animate={{ scale: 4.2, opacity: 0 }}
            transition={{
              scale: {
                duration: 2,
                repeat: Infinity,
                ease: [0, 0.2, 0.8, 1],
                delay: i,
              },
              // Fade out ahead of the scale (easeOut = fast initial drop) so the
              // ring is fully transparent well before it reaches the outer edge;
              // otherwise the largest ring lingers faint and reads as a flash
              // right before it vanishes.
              opacity: {
                duration: 2,
                repeat: Infinity,
                ease: "easeOut",
                delay: i,
              },
            }}
          />
        ))}
        <motion.span
          className="size-[9px] rounded-full"
          style={{ backgroundColor: color }}
          animate={found ? { scale: [1, 1.35, 1] } : { scale: 1 }}
          transition={{ duration: 0.5, ease: "easeOut" }}
        />
      </div>
    </div>
  );
}
