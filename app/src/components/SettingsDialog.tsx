import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  getReaperPath,
  installExtension,
  refreshStatus,
  setReaperPath,
  useReaperStatus,
} from "@/lib/install";

// App settings, opened from the native menu (ReaMesh → Settings…, Cmd+,).
// Holds the REAPER folder override (portable installs) and the extension
// install/update controls.
export function SettingsDialog() {
  const [open_, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const status = useReaperStatus();
  const hasCustom = !!getReaperPath();

  useEffect(() => {
    const un = listen("menu:settings", () => {
      refreshStatus();
      setOpen(true);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const pickFolder = async () => {
    const dir = await open({
      directory: true,
      title: "Locate your REAPER folder",
    });
    if (typeof dir === "string") {
      setReaperPath(dir);
      await refreshStatus();
    }
  };

  const useDefault = async () => {
    setReaperPath(null);
    await refreshStatus();
  };

  const install = async () => {
    setBusy(true);
    try {
      await installExtension();
      toast.success("Extension installed. Restart REAPER to load it.");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const extBadge = !status ? null : !status.installed ? (
    <Badge variant="outline" className="text-[11px] text-muted-foreground">
      Not installed
    </Badge>
  ) : status.upToDate ? (
    <Badge
      variant="outline"
      className="border-emerald-500/30 text-[11px] text-emerald-500"
    >
      Installed
    </Badge>
  ) : (
    <Badge
      variant="outline"
      className="border-amber-500/30 text-[11px] text-amber-500"
    >
      Update available
    </Badge>
  );

  return (
    <Dialog open={open_} onOpenChange={setOpen}>
      <DialogContent className="max-w-[360px] gap-5 rounded-2xl">
        <DialogHeader>
          <DialogTitle className="text-[15px]">Settings</DialogTitle>
          <DialogDescription className="sr-only">
            ReaMesh settings
          </DialogDescription>
        </DialogHeader>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            REAPER folder
          </h3>
          <div className="space-y-3 rounded-xl bg-card p-3">
            <div className="flex items-center justify-between gap-2">
              <p className="break-all text-[12px] leading-relaxed text-foreground/80">
                {status?.resourcePath ?? "Unknown"}
              </p>
              {status && !status.reaperFound && (
                <Badge
                  variant="outline"
                  className="shrink-0 border-amber-500/30 text-[11px] text-amber-500"
                >
                  Not found
                </Badge>
              )}
            </div>
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                className="h-7 flex-1 text-[12px]"
                onClick={pickFolder}
              >
                Change…
              </Button>
              {hasCustom && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 flex-1 text-[12px]"
                  onClick={useDefault}
                >
                  Use default
                </Button>
              )}
            </div>
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            REAPER extension
          </h3>
          <div className="space-y-3 rounded-xl bg-card p-3">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[13px]">reaper_reamesh</span>
              {extBadge}
            </div>
            <Button
              size="sm"
              className="h-7 w-full text-[12px]"
              disabled={busy || !status?.reaperFound}
              onClick={install}
            >
              {busy
                ? "Installing…"
                : !status?.installed
                  ? "Install extension"
                  : status.upToDate
                    ? "Reinstall extension"
                    : "Update extension"}
            </Button>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              REAPER loads the extension at startup. Restart it after
              installing or updating.
            </p>
          </div>
        </section>
      </DialogContent>
    </Dialog>
  );
}
