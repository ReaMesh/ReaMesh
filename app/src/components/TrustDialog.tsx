// Summarize the download, claimed host, and plugins before joining. The user
// can join normally or open a view-only copy without third-party effects. The
// extension checks the claimed plugin list again before opening the project.

import { useMemo, useState } from "react";
import { ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatBytes } from "@/lib/format";
import { hostSeenCount } from "@/lib/knownHosts";
import { t } from "@/lib/i18n";
import { parseFxName, pluginStatus, useInstalledPlugins, type PluginStatus } from "@/lib/plugins";
import type { SessionPreview } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

const STATUS_DOT: Record<PluginStatus, string> = {
  have: "bg-emerald-400",
  missing: "bg-amber-400",
  unknown: "bg-muted-foreground/50",
};

function statusLabel(s: PluginStatus): string {
  return s === "have" ? t("pluginInstalled") : s === "missing" ? t("pluginMissing") : t("pluginUnknown");
}

function PluginList({ plugins }: { plugins: string[] }) {
  const installed = useInstalledPlugins();
  const [copied, setCopied] = useState(false);
  const rows = useMemo(
    () =>
      plugins.map((raw) => ({
        raw,
        parsed: parseFxName(raw),
        status: pluginStatus(raw, installed),
      })),
    [plugins, installed],
  );
  const missing = rows.filter((r) => r.status === "missing");
  const anyKnown = rows.some((r) => r.status !== "unknown");

  const copyMissing = async () => {
    try {
      await navigator.clipboard.writeText(missing.map((m) => m.raw).join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Leave the list visible if clipboard access fails.
    }
  };

  return (
    <div className="space-y-2">
      <p className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {t("pluginsTitle")}
      </p>
      <div className="scroll-slim max-h-36 space-y-0.5 overflow-y-auto rounded-lg bg-secondary/50 p-1.5">
        {rows.map((r) => (
          <div key={r.raw} className="flex items-center gap-2 px-1 py-0.5" title={r.raw}>
            <span
              aria-hidden="true"
              className={cn("size-1.5 shrink-0 rounded-full", STATUS_DOT[r.status])}
            />
            <span className="min-w-0 flex-1 truncate text-[12px]">
              {r.parsed.name || r.raw}
              {r.parsed.vendor && (
                <span className="text-muted-foreground"> · {r.parsed.vendor}</span>
              )}
            </span>
            {r.parsed.fmt && (
              <span className="shrink-0 text-[10px] uppercase text-muted-foreground">
                {r.parsed.fmt}
              </span>
            )}
            <span className="sr-only">{statusLabel(r.status)}</span>
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between gap-2">
        <p
          className={cn(
            "text-[11px]",
            missing.length > 0 ? "text-amber-500" : "text-muted-foreground",
          )}
        >
          {!anyKnown
            ? t("pluginsUnknown")
            : missing.length > 0
              ? t("pluginsMissingSummary", { missing: missing.length, total: rows.length })
              : t("pluginsAllInstalled", { total: rows.length })}
        </p>
        {missing.length > 0 && (
          <button
            type="button"
            onClick={copyMissing}
            className={cn(
              "shrink-0 rounded px-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground",
              focusRing,
            )}
          >
            {copied ? t("missingCopied") : t("copyMissing")}
          </button>
        )}
      </div>
      {missing.length > 0 && (
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          {t("missingPluginNote")}
        </p>
      )}
    </div>
  );
}

export function TrustDialog({
  open,
  onOpenChange,
  name,
  hostName,
  hostPeerId,
  bytes,
  files,
  preview,
  onJoin,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Session display name. */
  name: string;
  /** The host's self-reported display name ("" / undefined = none claimed). */
  hostName?: string;
  /** The host's peer id, for the "joined before" memory. */
  hostPeerId?: string;
  /** Announce-level size info (fallback when the preview lacks it). */
  bytes?: number;
  files?: number;
  /** Manifest preview from a successful probe (may be null on older hosts). */
  preview?: SessionPreview | null;
  onJoin: (nofx: boolean) => void;
}) {
  const seen = hostPeerId ? hostSeenCount(hostPeerId) : 0;
  const totalBytes = preview?.bytes ?? bytes ?? 0;
  const totalFiles = preview?.files ?? files ?? 0;
  const tracks = preview?.meta?.tracks;
  const plugins = preview?.meta?.plugins;
  const stats = [
    typeof tracks === "number" && tracks > 0 ? t("previewTracks", { count: tracks }) : null,
    totalBytes > 0
      ? t("filesAndSize", { count: totalFiles, size: formatBytes(totalBytes) })
      : null,
  ].filter(Boolean);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[360px] gap-4 rounded-2xl">
        <DialogHeader>
          <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
            <ShieldAlert className="size-5 text-amber-500" />
          </div>
          <DialogTitle className="text-[15px]">{t("trustTitle")}</DialogTitle>
          <DialogDescription className="text-[13px] leading-relaxed">
            {t("trustBody", { name })}
            {hostName && (
              <span className="block pt-2">{t("trustHostClaims", { host: hostName })}</span>
            )}
            {hostPeerId && (
              <span className="block pt-1 text-[12px]">
                {seen === 0
                  ? t("hostFirstTime")
                  : seen === 1
                    ? t("hostSeenOnce")
                    : t("hostSeenBefore", { count: seen })}
              </span>
            )}
          </DialogDescription>
        </DialogHeader>

        {(stats.length > 0 || (plugins && plugins.length > 0)) && (
          <div className="space-y-3">
            {stats.length > 0 && (
              <p className="text-[12px] text-muted-foreground">{stats.join(" · ")}</p>
            )}
            {plugins && plugins.length > 0 && <PluginList plugins={plugins} />}
            <p className="text-[10px] leading-relaxed text-muted-foreground/80">
              {t("reportedByHost")}
            </p>
          </div>
        )}

        <DialogFooter className="flex-col gap-2 sm:flex-col sm:gap-2">
          <div className="flex w-full gap-2">
            <Button variant="outline" className="flex-1" onClick={() => onOpenChange(false)}>
              {t("cancel")}
            </Button>
            <Button className="flex-1" onClick={() => onJoin(false)}>
              {t("joinAnyway")}
            </Button>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="w-full text-[12px] text-muted-foreground"
            title={t("joinNoFxHint")}
            onClick={() => onJoin(true)}
          >
            {t("joinNoFx")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
