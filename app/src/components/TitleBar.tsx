import { useConnections, setActivePort } from "@/lib/connection";
import { cn } from "@/lib/utils";

// macOS-style overlay title bar: the traffic lights float over this strip,
// so it stays draggable (data-tauri-drag-region) and keeps ~76px clear on
// the left. When several REAPER instances run (the two-instances-on-one-Mac
// dev setup), each becomes a small project-name tab.
export function TitleBar() {
  const { instances, activePort } = useConnections();

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-11 shrink-0 items-center border-b border-border bg-background/80 pl-[76px] pr-3"
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
    </header>
  );
}
