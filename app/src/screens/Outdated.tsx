import { CircleArrowUp } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import type { ExtState } from "@/lib/types";

// The extension answered, but from a REAPER older than it supports: it runs as
// a stub that accepts no commands, so this screen replaces the lobby until the
// user updates REAPER and starts it again.
export function Outdated({ info }: { info: NonNullable<ExtState["unsupported"]> }) {
  return (
    <div className="flex flex-1 flex-col items-center [justify-content:safe_center] overflow-y-auto gap-6 px-10 text-center">
      <div className="flex size-11 items-center justify-center rounded-full bg-amber-500/15">
        <CircleArrowUp className="size-5 text-amber-500" aria-hidden="true" />
      </div>

      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">{t("reaperOutdatedTitle")}</h2>
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          {info.version
            ? t("reaperOutdatedBody", { min: info.min, version: info.version })
            : t("reaperOutdatedBodyUnknown", { min: info.min })}
        </p>
      </div>

      <div className="flex w-full max-w-[240px] flex-col items-center gap-3">
        <Button className="w-full" onClick={() => openUrl("https://www.reaper.fm/download.php")}>
          {t("getReaperUpdate")}
        </Button>
      </div>
    </div>
  );
}
