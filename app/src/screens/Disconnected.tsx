import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { AnimatePresence, motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { ArcRing } from "@/components/ArcRing";
import { RMarkFill } from "@/components/RMark";
import { FirewallNotice } from "@/components/FirewallNotice";
import { isWindows } from "@/lib/platform";
import { FG_COLOR } from "@/lib/colors";
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
  // The title and the button say what to do; a line below only when there is
  // something they can't say.
  const body = !found && closed && closedWasSession ? t("reaperClosedSession") : "";
  return (
    <div className="flex flex-1 flex-col items-center [justify-content:safe_center] overflow-y-auto gap-6 px-10 text-center">
      <ScanSpinner found={found} />

      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">{title}</h2>
        {body && <p className="text-[13px] leading-relaxed text-muted-foreground">{body}</p>}
      </div>

      <div className="flex w-full max-w-[240px] flex-col gap-3">
        <Button className="w-full" onClick={launchReaper} disabled={found}>
          {t("openReaper")}
        </Button>
        {/* Explain why a loaded extension may still be unreachable. */}
        {portless && !found && (
          <p role="status" className="text-left text-[12px] leading-normal text-amber-500">
            {t("portTrouble")}
          </p>
        )}
        {/* Show firewall advice before Windows opens its permission dialog. */}
        {!found && <FirewallNotice dismissible className="mt-5" />}
      </div>
    </div>
  );
}

// Spin while searching for REAPER. On connection the ring runs on to the R's
// foot (FOOT_ANGLE) and its arc draws itself into that point while the ring
// shrinks towards it and fades; the R inks in from that same foot, so the
// spinner pours into the mark. A small swell when the ink is full closes it.
// App holds this screen (`foundBeat`) until all of it has played.
const MARK_H = 26;
// Where the foot of the R's left stroke sits in the 44px box with the mark
// centred at MARK_H: (18.5, 34), i.e. on the ring at 196° from 12 o'clock.
const FOOT_ORIGIN = "42% 77%";
const FOOT_ANGLE = 196;
const INK_DELAY = 0.3;
const INK_DURATION = 0.9;

function ScanSpinner({ found }: { found: boolean }) {
  return (
    <div className="relative grid size-11 place-items-center">
      <motion.div
        className="absolute inset-0"
        style={{ transformOrigin: FOOT_ORIGIN }}
        initial={false}
        animate={
          found
            ? {
                scale: 0.5,
                opacity: [1, 1, 0],
                filter: "blur(2px)",
                transition: {
                  duration: 0.55,
                  ease: [0.4, 0, 0.6, 1],
                  opacity: { duration: 0.55, times: [0, 0.45, 1] },
                },
              }
            : { scale: 1, opacity: 1, filter: "blur(0px)", transition: { duration: 0.3 } }
        }
      >
        <ArcRing
          size={44}
          radius={11}
          strokeWidth={2.5}
          spinning={!found}
          progress={0}
          color={FG_COLOR}
          track={false}
          restAngle={FOOT_ANGLE}
        />
      </motion.div>
      <AnimatePresence>
        {found && (
          <motion.div
            key="mark"
            initial={{ scale: 1 }}
            animate={{ scale: [1, 1.04, 1] }}
            exit={{ opacity: 0, transition: { duration: 0.2 } }}
            transition={{ delay: INK_DELAY + INK_DURATION - 0.05, duration: 0.4, ease: "easeInOut" }}
          >
            <RMarkFill height={MARK_H} delay={INK_DELAY} duration={INK_DURATION} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
