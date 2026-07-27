import { useEffect, useState } from "react";
import { motion } from "motion/react";
import { CircleSlash } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { FirewallNotice } from "@/components/FirewallNotice";
import { isWindows } from "@/lib/platform";
// Concrete color values (mirrors of index.css variables) because Motion can
// only interpolate between real colors: a CSS variable would snap instead of
// easing grey→blue.
import { FOUND_COLOR, SCAN_COLOR } from "@/lib/colors";
import { t } from "@/lib/i18n";

async function launchReaper() {
  try {
    // macOS has one canonical bundle path; on Windows the .exe location
    // varies by install, so the backend resolves it (the webview can't).
    if (isWindows) await invoke("launch_reaper");
    else await openPath("/Applications/REAPER.app");
  } catch {
    toast.error(t("couldntOpenReaper"));
  }
}

/** Poll the marker written when the extension is loaded but all control ports
 *  are occupied. Without it, this state looks like a missing extension. */
function usePortTrouble(): boolean {
  const [portless, setPortless] = useState(false);
  useEffect(() => {
    let alive = true;
    const check = () =>
      invoke<boolean>("port_trouble")
        .then((v) => {
          if (alive) setPortless(!!v);
        })
        .catch(() => {});
    check();
    const id = setInterval(check, 7000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);
  return portless;
}

// Show this screen until a local REAPER instance answers. `closed` distinguishes
// a lost connection from first-run setup, and `found` drives the handoff state.
export function Disconnected({
  found = false,
  closed = false,
  closedWasSession = false,
}: {
  found?: boolean;
  closed?: boolean;
  closedWasSession?: boolean;
}) {
  const portless = usePortTrouble();
  const title = found ? t("reaperFound") : closed ? t("reaperClosed") : t("waitingForReaper");
  const body = found
    ? t("connecting")
    : closed
      ? closedWasSession
        ? `${t("reaperClosedSession")} ${t("reaperClosedBody")}`
        : t("reaperClosedBody")
      : t("startReaperToConnect");
  return (
    <div className="flex flex-1 flex-col items-center [justify-content:safe_center] overflow-y-auto gap-6 px-10 text-center">
      <BreathingBeacon found={found} />

      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">{title}</h2>
        <p className="text-[13px] leading-relaxed text-muted-foreground">{body}</p>
      </div>

      <div className="flex w-full max-w-[240px] flex-col items-center gap-3">
        <Button className="w-full" onClick={launchReaper} disabled={found}>
          {t("openReaper")}
        </Button>
        {/* Explain why a loaded extension may still be unreachable. */}
        {portless && !found && (
          <div role="status" className="space-y-1 rounded-lg bg-amber-500/10 p-2.5 text-left">
            <p className="flex items-start gap-2 text-[12px] font-semibold leading-relaxed text-amber-500">
              <CircleSlash className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              {t("portTroubleTitle")}
            </p>
            <p className="text-[11px] leading-relaxed text-amber-500">{t("portTroubleBody")}</p>
          </div>
        )}
        {/* Show firewall advice before Windows opens its permission dialog. */}
        {!found && <FirewallNotice dismissible />}
      </div>
    </div>
  );
}

// Two staggered rings pulse around the center dot. When REAPER answers, the
// existing animation changes color instead of restarting.
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
