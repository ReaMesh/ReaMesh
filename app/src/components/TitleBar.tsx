import { useEffect, useState } from "react";
import { Copy, Minus, Settings, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { BarMark } from "@/components/BarMark";
import { ProfileChip } from "@/components/ProfileChip";
import { useConnections, setActivePort } from "@/lib/connection";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";

// Title bar: a 44px draggable strip at the top of the window on both
// platforms.
//
// On macOS the native traffic lights sit over its left end (titleBarStyle
// Overlay), the mark + wordmark sit centred, and the right end carries the
// REAPER instance tabs (when several run) and the identity chip. Settings lives
// in the application menu.
//
// On Windows the window is undecorated (tauri.windows.conf.json), so this strip
// IS the title bar: app name at the left, instance tabs, the Settings gear,
// and our own minimize / maximize / close buttons at the right, the layout
// Windows Terminal, Spotify and the Windows Settings app use. Double-click on
// the strip toggles maximize (Tauri's drag-region script handles that).
export function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { instances, activePort } = useConnections();

  const tabs = instances.length > 1 && (
    // Use a radio group so assistive technology can identify the active REAPER.
    // Include the port to distinguish instances with the same project name.
    <nav role="radiogroup" aria-label={t("reaperInstances")} className="flex items-center gap-1">
      {instances.map((inst) => {
        const label = inst.state?.project || `REAPER :${inst.port}`;
        const active = inst.port === activePort;
        return (
          <button
            key={inst.port}
            type="button"
            role="radio"
            aria-checked={active}
            title={`${label} · :${inst.port}`}
            onClick={() => setActivePort(inst.port)}
            className={cn(
              "max-w-28 truncate rounded-md px-2 py-1 text-[11px] transition-colors",
              active
                ? "bg-secondary text-foreground"
                : "text-muted-foreground hover:text-foreground",
              focusRing,
            )}
          >
            {label}
          </button>
        );
      })}
    </nav>
  );

  if (isWindows) {
    return (
      <header
        data-tauri-drag-region
        className="relative flex h-11 shrink-0 select-none items-center bg-background/80 pl-3"
      >
        <Wordmark />
        <div className="ml-auto flex h-full items-center gap-1">
          {tabs}
          <ProfileChip />
          <button
            type="button"
            onClick={onOpenSettings}
            aria-label={t("settings")}
            title={t("settings")}
            className={cn(
              "mr-1 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground",
              focusRing,
            )}
          >
            <Settings className="size-4" aria-hidden="true" />
          </button>
          <WindowControls />
        </div>
      </header>
    );
  }

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-11 shrink-0 items-center bg-background/80 pl-[76px] pr-2.5"
    >
      {/* Centred in the window, not in the space left of the chip. Several
          REAPER instances need the room, so their tabs take its place. */}
      {!tabs && (
        <div className="pointer-events-none absolute inset-0 grid place-items-center">
          <Wordmark />
        </div>
      )}
      <div className="ml-auto flex items-center gap-2">
        {tabs}
        <ProfileChip />
      </div>
    </header>
  );
}

function Wordmark() {
  return (
    <span className="pointer-events-none flex select-none items-center gap-[7px] text-[12px] font-semibold tracking-[0.01em] text-muted-foreground">
      <BarMark height={11} />
      ReaMesh
    </span>
  );
}

// Windows caption buttons. Full-height, 46px wide, square (no rounding) like
// the native ones; close turns Windows' signature red on hover. Every window
// call is wrapped so the plain-browser dev harness (no Tauri) stays usable.
function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    try {
      const win = getCurrentWindow();
      const sync = () => win.isMaximized().then(setMaximized).catch(() => {});
      sync();
      win
        .onResized(sync)
        .then((fn) => {
          if (cancelled) fn();
          else unlisten = fn;
        })
        .catch(() => {});
    } catch {
      // Not running inside Tauri.
    }
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const call = (fn: (w: ReturnType<typeof getCurrentWindow>) => Promise<void>) => () => {
    try {
      fn(getCurrentWindow()).catch(() => {});
    } catch {
      // Not running inside Tauri.
    }
  };

  const base = cn(
    "flex h-full w-[46px] items-center justify-center text-foreground/80 transition-colors",
    focusRing,
  );
  return (
    <div className="flex h-full items-stretch">
      <button
        type="button"
        onClick={call((w) => w.minimize())}
        aria-label={t("minimizeWindow")}
        title={t("minimizeWindow")}
        className={cn(base, "hover:bg-secondary hover:text-foreground")}
      >
        <Minus className="size-4" aria-hidden="true" />
      </button>
      <button
        type="button"
        onClick={call((w) => w.toggleMaximize())}
        aria-label={maximized ? t("restoreWindow") : t("maximizeWindow")}
        title={maximized ? t("restoreWindow") : t("maximizeWindow")}
        className={cn(base, "hover:bg-secondary hover:text-foreground")}
      >
        {maximized ? (
          <Copy className="size-3.5 -scale-x-100" aria-hidden="true" />
        ) : (
          <Square className="size-3.5" aria-hidden="true" />
        )}
      </button>
      <button
        type="button"
        onClick={call((w) => w.close())}
        aria-label={t("closeWindow")}
        title={t("closeWindow")}
        className={cn(base, "hover:bg-[#c42b1c] hover:text-white")}
      >
        <X className="size-4" aria-hidden="true" />
      </button>
    </div>
  );
}
