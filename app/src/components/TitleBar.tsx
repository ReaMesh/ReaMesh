import { Settings2 } from "lucide-react";
import { useConnections, setActivePort } from "@/lib/connection";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn } from "@/lib/utils";

// Overlay title bar. On macOS the traffic lights float over this strip, so it
// stays draggable (data-tauri-drag-region) and keeps ~76px clear on the left;
// Windows draws its own decorations above it, so that reserved gutter would
// just be a hole. When several REAPER instances run, each becomes a small
// project-name tab.
//
// The gear is Windows-only on purpose: macOS reaches Settings through the
// application menu (ReaMesh → Settings…, Cmd+,), which Windows has no
// equivalent of; without it Settings would be unreachable there.
export function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { instances, activePort } = useConnections();

  return (
    <header
      data-tauri-drag-region
      className={cn(
        "relative flex h-11 shrink-0 items-center bg-background/80 pr-3",
        isWindows ? "pl-3" : "pl-[76px]",
      )}
    >
      {instances.length > 1 && (
        <nav className="ml-auto flex items-center gap-1">
          {instances.map((inst) => (
            <button
              key={inst.port}
              onClick={() => setActivePort(inst.port)}
              className={cn(
                "max-w-28 truncate rounded-md px-2 py-1 text-[11px] transition-colors",
                inst.port === activePort
                  ? "bg-secondary text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {inst.state?.project || `REAPER :${inst.port}`}
            </button>
          ))}
        </nav>
      )}
      {isWindows && (
        <button
          onClick={onOpenSettings}
          aria-label={t("settings")}
          title={t("settings")}
          className={cn(
            "rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground",
            instances.length > 1 ? "ml-1" : "ml-auto",
          )}
        >
          <Settings2 className="size-4" />
        </button>
      )}
    </header>
  );
}
