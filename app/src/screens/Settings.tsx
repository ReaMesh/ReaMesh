import { useState } from "react";
import { ArrowLeft, Check, ExternalLink } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { getLangPref, setLangPref, t, useLang, type LangPref } from "@/lib/i18n";
import {
  getReaperPath,
  installExtension,
  refreshStatus,
  setReaperPath,
  useReaperStatus,
} from "@/lib/install";
import {
  type OverlaySettings,
  setOverlaySetting,
  useOverlaySettings,
} from "@/lib/overlaySettings";
import { cn } from "@/lib/utils";

const OVERLAY_TOGGLES: { key: keyof OverlaySettings; label: () => string }[] = [
  { key: "roster", label: () => t("olRoster") },
  { key: "activity", label: () => t("olActivity") },
  { key: "playheads", label: () => t("olPlayheads") },
  { key: "ghosts", label: () => t("olSelections") },
];

// Full-screen app settings, opened from the native menu on non-macOS
// platforms (macOS opens the native AppKit settings window instead). Holds
// the REAPER folder override (portable installs), the extension
// install/update controls, and the app language.
export function Settings({ onBack }: { onBack: () => void }) {
  const [busy, setBusy] = useState(false);
  const status = useReaperStatus();
  const hasCustom = !!getReaperPath();
  const overlay = useOverlaySettings();
  useLang(); // re-render when the language changes

  const pickFolder = async () => {
    const dir = await open({
      directory: true,
      title: t("locateReaperFolder"),
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
      toast.success(t("extInstalledToast"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const extBadge = !status ? null : !status.installed ? (
    <Badge variant="outline" className="text-[11px] text-muted-foreground">
      {t("notInstalled")}
    </Badge>
  ) : status.upToDate ? (
    <Badge
      variant="outline"
      className="border-emerald-500/30 text-[11px] text-emerald-500"
    >
      {t("installed")}
    </Badge>
  ) : (
    <Badge
      variant="outline"
      className="border-amber-500/30 text-[11px] text-amber-500"
    >
      {t("updateAvailable")}
    </Badge>
  );

  const langOptions: { value: LangPref; label: string }[] = [
    { value: "system", label: t("languageSystem") },
    { value: "en", label: "English" },
    { value: "de", label: "Deutsch" },
  ];

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
            {t("back")}
          </Button>
          <h1 className="text-[15px] font-semibold">{t("settings")}</h1>
        </div>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("language")}
          </h3>
          <div className="flex gap-1 rounded-xl bg-card p-1.5">
            {langOptions.map((o) => (
              <button
                key={o.value}
                onClick={() => setLangPref(o.value)}
                className={cn(
                  "flex-1 rounded-lg px-2 py-1.5 text-[12px] transition-colors",
                  getLangPref() === o.value
                    ? "bg-secondary text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {o.label}
              </button>
            ))}
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("reaperFolder")}
          </h3>
          <div className="space-y-3 rounded-xl bg-card p-3.5">
            <div className="flex items-center justify-between gap-2">
              <p className="break-all rounded-md bg-secondary/60 px-2.5 py-1.5 text-[12px] leading-relaxed text-foreground/80">
                {status?.resourcePath ?? t("notFoundBadge")}
              </p>
              {status && !status.reaperFound && (
                <Badge
                  variant="outline"
                  className="shrink-0 border-amber-500/30 text-[11px] text-amber-500"
                >
                  {t("notFoundBadge")}
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
                {t("change")}
              </Button>
              {hasCustom && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 flex-1 text-[12px]"
                  onClick={useDefault}
                >
                  {t("useDefault")}
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
                {t("dontHaveReaper")}
                <ExternalLink className="size-3" />
              </Button>
            )}
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("reaperExtension")}
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
                ? t("installing")
                : !status?.installed
                  ? t("installExtension")
                  : status.upToDate
                    ? t("reinstallExtension")
                    : t("updateExtension")}
            </Button>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              {t("restartNote")}
            </p>
          </div>
        </section>

        <section className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("overlaySettingsTitle")}
          </h3>
          <div className="flex flex-col gap-0.5 rounded-xl bg-card p-1.5">
            {OVERLAY_TOGGLES.map((row) => {
              const on = overlay[row.key];
              return (
                <button
                  key={row.key}
                  type="button"
                  onClick={() => setOverlaySetting(row.key, !on)}
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] transition-colors hover:bg-secondary/60"
                >
                  <span
                    className={cn(
                      "grid size-4 shrink-0 place-items-center rounded-[5px] border transition-colors",
                      on
                        ? "border-foreground/30 bg-foreground/85 text-background"
                        : "border-border text-transparent",
                    )}
                  >
                    <Check className="size-3" strokeWidth={3} />
                  </span>
                  <span className="flex-1 text-foreground/90">{row.label()}</span>
                </button>
              );
            })}
          </div>
        </section>
      </div>
    </div>
  );
}
