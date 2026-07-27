import { Settings2 } from "lucide-react";
import { useConnections, setActivePort } from "@/lib/connection";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";

// Right-hand inset for a screen's topmost row, so whatever it right-aligns
// (the Hosting badge, a card's contents) clears the floating Settings gear.
// Empty on macOS, where the gear does not exist and the title bar is a real
// strip in the layout.
export const topRightGutter = isWindows ? "pr-9" : "";

// Title bar. The two platforms need opposite things from it:
//
// On macOS this 44px strip leaves room for the traffic lights and provides a
// draggable area.
//
// On Windows the controls float above the screen and leave room for the native
// title bar.
//
// Windows needs the gear because macOS exposes Settings in the application menu.
export function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { instances, activePort } = useConnections();

  const tabs = instances.length > 1 && (
    // Use a radio group so assistive technology can identify the active REAPER.
    // Include the port to distinguish instances with the same project name.
    <nav
      role="radiogroup"
      aria-label={t("reaperInstances")}
      className="pointer-events-auto flex items-center gap-1"
    >
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
      // Let clicks pass through the title-bar layer outside its controls.
      <div className="pointer-events-none absolute right-0 top-0 z-50 flex items-center gap-1 p-2">
        {tabs}
        <button
          onClick={onOpenSettings}
          aria-label={t("settings")}
          title={t("settings")}
          className={cn(
            "pointer-events-auto rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground",
            focusRing,
          )}
        >
          <Settings2 className="size-4" aria-hidden="true" />
        </button>
      </div>
    );
  }

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-11 shrink-0 items-center bg-background/80 pl-[76px] pr-3"
    >
      {tabs && <div className="ml-auto">{tabs}</div>}
    </header>
  );
}
