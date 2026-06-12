import { useState } from "react";
import { ArrowLeft, ExternalLink } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  getReaperPath,
  installExtension,
  refreshStatus,
  setReaperPath,
  useReaperStatus,
} from "@/lib/install";

// Full-screen app settings, opened from the native menu (ReaMesh →
// Settings…, Cmd+,). Holds the REAPER folder override (portable installs)
// and the extension install/update controls.
export function Settings({ onBack }: { onBack: () => void }) {
  const [busy, setBusy] = useState(false);
  const status = useReaperStatus();
  const hasCustom = !!getReaperPath();

  const pickFolder = async () => {
    const dir = await open({
      directory: true,
      title: "Locate your REAPER folder",
    });
    if (typeof dir === "string") {
      await setReaperPath(dir);
      await refreshStatus();
    }
  };

  const useDefault = async () => {
    await setReaperPath(null);
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
    <div className="flex flex-1 flex-col overflow-y-auto">
      <div className="mx-auto w-full max-w-md space-y-6 px-6 py-5">
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            className="-ml-2 h-8 gap-1.5 px-2 text-[13px] text-muted-foreground hover:text-foreground"
            onClick={onBack}
          >
            <ArrowLeft className="size-4" />
            Back
          </Button>
          <h1 className="text-[15px] font-semibold">Settings</h1>
        </div>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            REAPER folder
          </h3>
          <div className="space-y-3 rounded-xl bg-card p-3.5">
            <div className="flex items-center justify-between gap-2">
              <p className="break-all rounded-md bg-secondary/60 px-2.5 py-1.5 text-[12px] leading-relaxed text-foreground/80">
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
            {status && !status.reaperFound && (
              <Button
                variant="ghost"
                size="sm"
                className="h-7 w-full gap-1.5 text-[12px] text-muted-foreground hover:text-foreground"
                onClick={() => openUrl("https://www.reaper.fm/download.php")}
              >
                Don't have REAPER? Download it
                <ExternalLink className="size-3" />
              </Button>
            )}
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            REAPER extension
          </h3>
          <div className="space-y-3 rounded-xl bg-card p-3.5">
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
      </div>
    </div>
  );
}
