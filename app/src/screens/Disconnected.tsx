import { useEffect, useState } from "react";
import { CircleSlash } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ArcRing } from "@/components/ArcRing";
import { FirewallNotice } from "@/components/FirewallNotice";
import { isWindows } from "@/lib/platform";
// Concrete color values (mirrors of index.css variables) because Motion can
// only interpolate between real colors: a CSS variable would snap instead of
// easing grey→blue.
import { FG_COLOR, FOUND_COLOR } from "@/lib/colors";
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
      <ScanSpinner found={found} />

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

// Spin while searching for REAPER. On connection, finish the turn,
// close the ring, and transition to the success color.
function ScanSpinner({ found }: { found: boolean }) {
  return (
    <div className="relative size-11">
      <ArcRing
        size={44}
        radius={11}
        strokeWidth={2.5}
        spinning={!found}
        progress={1}
        color={found ? FOUND_COLOR : FG_COLOR}
        track={false}
      />
    </div>
  );
}
