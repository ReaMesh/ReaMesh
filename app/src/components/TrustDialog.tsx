// Show the host, project size, and local plugin availability before joining.
// The extension verifies the manifest's plugin list against the downloaded project.

import { useMemo, useState } from "react";
import { TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFact,
  DialogFacts,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useActiveInstance } from "@/lib/connection";
import { formatBytes, reaperVersionKey } from "@/lib/format";
import { t } from "@/lib/i18n";
import { hostSeenCount } from "@/lib/knownHosts";
import { parseFxName, pluginStatus, useInstalledPlugins, type PluginStatus } from "@/lib/plugins";
import type { SessionPreview } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

const STATUS_DOT: Record<PluginStatus, string> = {
  have: "bg-emerald-400",
  missing: "bg-amber-400",
  unknown: "bg-muted-foreground/50",
};

// Sort missing plugins first.
const STATUS_ORDER: Record<PluginStatus, number> = { missing: 0, unknown: 1, have: 2 };

function statusLabel(s: PluginStatus): string {
  return s === "have" ? t("pluginInstalled") : s === "missing" ? t("pluginMissing") : t("pluginUnknown");
}

function PluginList({ plugins }: { plugins: string[] }) {
  const installed = useInstalledPlugins();
  const [copied, setCopied] = useState(false);
  const rows = useMemo(
    () =>
      plugins
        .map((raw) => ({ raw, parsed: parseFxName(raw), status: pluginStatus(raw, installed) }))
        .sort((a, b) => STATUS_ORDER[a.status] - STATUS_ORDER[b.status]),
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

  const summary = !anyKnown
    ? t("pluginsUnknown", { total: rows.length })
    : missing.length > 0
      ? t("pluginsMissingSummary", { missing: missing.length, total: rows.length })
      : t("pluginsAllInstalled", { total: rows.length });

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between gap-2">
        <p
          className={cn(
            "text-[12px] font-medium",
            missing.length > 0 ? "text-amber-500" : "text-muted-foreground",
          )}
        >
          {summary}
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
      <div className="scroll-slim max-h-36 space-y-0.5 overflow-y-auto rounded-xl bg-white/[0.04] p-1.5">
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
              <span className="shrink-0 text-[11px] text-muted-foreground">
                {r.parsed.fmt}
              </span>
            )}
            <span className="sr-only">{statusLabel(r.status)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

export function TrustDialog({
  open,
  onOpenChange,
  name,
  hostId,
  hostName,
  hostColor,
  bytes,
  files,
  preview,
  onJoin,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Session display name. */
  name: string;
  /** The host's lasting peer id: keys the "joined N times" memory. */
  hostId?: string;
  /** The host's self-reported display name ("" / undefined = none claimed). */
  hostName?: string;
  hostColor?: string;
  /** Announce-level size info (fallback when the preview lacks it). */
  bytes?: number;
  files?: number;
  /** Manifest preview from a successful probe (may be null on older hosts). */
  preview?: SessionPreview | null;
  onJoin: (nofx: boolean) => void;
}) {
  const [nofx, setNofx] = useState(false);
  const totalBytes = preview?.bytes ?? bytes ?? 0;
  const totalFiles = preview?.files ?? files ?? 0;
  const tracks = preview?.meta?.tracks;
  const plugins = preview?.meta?.plugins;
  // Read when the dialog opens: the count is a memory of trusting this identity.
  const seen = useMemo(() => (open && hostId ? hostSeenCount(hostId) : 0), [open, hostId]);

  // A host on another REAPER version: state chunks are REAPER's own format, so
  // a newer host's project may not load faithfully here (and the other way
  // round for edits). Silent when either side doesn't report a version.
  const hostReaper = preview?.meta?.reaper;
  const localReaper = useActiveInstance()?.state?.reaper;
  const mismatch =
    hostReaper && localReaper && hostReaper !== localReaper
      ? t(
          reaperVersionKey(hostReaper) > reaperVersionKey(localReaper)
            ? "reaperMismatchHostNewer"
            : "reaperMismatchHostOlder",
          { host: hostReaper, local: localReaper },
        )
      : null;

  const stats = [
    typeof tracks === "number" && tracks > 0 ? t("previewTracks", { count: tracks }) : null,
    totalBytes > 0
      ? t("previewFiles", { count: totalFiles, size: formatBytes(totalBytes) })
      : null,
  ].filter(Boolean);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("trustTitle", { name })}</DialogTitle>
          <DialogDescription>{t("trustBody")}</DialogDescription>
        </DialogHeader>

        <DialogFacts>
          {(hostName || hostId) && (
            <DialogFact label={t("trustHostLabel")}>
              {hostName && (
                <>
                  <span
                    aria-hidden="true"
                    className="size-2 shrink-0 rounded-full"
                    style={{ backgroundColor: hostColor || undefined }}
                  />
                  <span className="truncate">{hostName}</span>
                </>
              )}
              {hostId && (
                <span className="shrink-0 font-normal text-muted-foreground">
                  {hostName && "· "}
                  {seen > 0 ? t("hostSeenTimes", { count: seen }) : t("hostSeenNever")}
                </span>
              )}
            </DialogFact>
          )}
          {stats.length > 0 && (
            <DialogFact label={t("trustProjectLabel")}>
              <span className="truncate tabular-nums">{stats.join(" · ")}</span>
            </DialogFact>
          )}
        </DialogFacts>

        {mismatch && (
          <p
            role="status"
            className="flex items-start gap-2 rounded-[10px] bg-amber-500/10 px-2.5 py-2 text-[12px] leading-snug text-amber-500"
          >
            <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {mismatch}
          </p>
        )}

        {plugins && plugins.length > 0 && <PluginList plugins={plugins} />}

        {/* An equal choice, not a fine-print link: without effects no plugin
            code runs and the project opens view-only. */}
        <div
          role="radiogroup"
          aria-label={t("joinModeLabel")}
          className="flex gap-1 rounded-[10px] bg-white/[0.04] p-1"
        >
          {[false, true].map((v) => (
            <button
              key={String(v)}
              type="button"
              role="radio"
              aria-checked={nofx === v}
              title={v ? t("joinNoFxHint") : undefined}
              onClick={() => setNofx(v)}
              className={cn(
                "h-7 flex-1 rounded-md text-[12px] font-medium transition-colors",
                nofx === v
                  ? "bg-white/[0.14] text-foreground shadow-sm ring-1 ring-inset ring-white/10"
                  : "text-muted-foreground hover:text-foreground/80",
                focusRing,
              )}
            >
              {v ? t("joinWithoutFx") : t("joinWithFx")}
            </button>
          ))}
        </div>

        <DialogFooter>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button onClick={() => onJoin(nofx)}>{t("joinAnyway")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
