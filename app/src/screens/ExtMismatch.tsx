import { useState } from "react";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { installExtension, refreshStatus, useReaperStatus } from "@/lib/install";
import { checkForUpdate, restartToUpdate, useUpdate } from "@/lib/updater";

// The REAPER that answered runs an extension from another build (its `proto`
// or its version differs from ours). App and extension release together, so
// instead of driving a mismatched pair this screen replaces the lobby until
// they match: usually REAPER still has the previous extension loaded after an
// app update, sometimes the app is the older half.
export function ExtMismatch({ skew }: { skew: "newer" | "older" }) {
  const update = useUpdate();
  const status = useReaperStatus();
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState("");
  if (skew === "older") {
    // The app installs its copy at launch; if that didn't land (an unusual
    // REAPER folder, permissions), a restart alone would load the old one.
    const notInstalled = !!status && !status.upToDate;
    return (
      <div className="flex flex-1 flex-col items-center [justify-content:safe_center] gap-6 overflow-y-auto px-10 text-center">
        <div className="space-y-1.5">
          <h2 className="text-[15px] font-semibold">
            {notInstalled ? t("installExtTitle") : t("extOldTitle")}
          </h2>
          <p className="text-[13px] leading-relaxed text-balance text-muted-foreground">
            {notInstalled ? t("extOldInstallBody") : t("extOldBody")}
          </p>
          {error && <p className="text-[12px] text-destructive">{error}</p>}
        </div>
        {notInstalled && (
          <Button
            className="w-full max-w-[240px]"
            disabled={installing}
            onClick={() => {
              setInstalling(true);
              setError("");
              installExtension()
                .catch((e) => setError(String(e)))
                .finally(() => {
                  setInstalling(false);
                  refreshStatus();
                });
            }}
          >
            {installing ? t("installing") : t("installExtension")}
          </Button>
        )}
      </div>
    );
  }
  const ready = update.phase === "ready" && update.can_restart;
  return (
    <div className="flex flex-1 flex-col items-center [justify-content:safe_center] gap-6 overflow-y-auto px-10 text-center">
      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">{t("appOldTitle")}</h2>
        <p className="text-[13px] leading-relaxed text-balance text-muted-foreground">
          {t("appOldBody")}
        </p>
      </div>
      <Button
        className="w-full max-w-[240px]"
        disabled={update.phase === "checking" || update.phase === "downloading"}
        onClick={() => (ready ? restartToUpdate().catch(() => {}) : checkForUpdate())}
      >
        {ready
          ? t("restartNow")
          : update.phase === "downloading"
            ? t("updateDownloading", { version: update.version })
            : t("checkUpdates")}
      </Button>
    </div>
  );
}
