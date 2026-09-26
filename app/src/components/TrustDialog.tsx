// Show the host, project size, and local plugin availability before joining.
// The extension verifies the manifest's plugin list against the downloaded project.

import { useMemo, useState } from "react";
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

// Sort missing plugins first.
const STATUS_ORDER: Record<PluginStatus, number> = { missing: 0, unknown: 1, have: 2 };

/** A plugin list: name, then vendor, one plugin per row. Tall enough for
 *  eight rows; a longer list scrolls with the ninth row half in view, so the
 *  cut never hides that there is more. */
export function PluginBox({ children }: { children: React.ReactNode }) {
  return (
    <div className="scroll-slim max-h-[216px] overflow-y-auto rounded-xl bg-white/[0.04] p-1.5">
      {children}
    </div>
  );
}

export function PluginRow({ raw, status }: { raw: string; status?: PluginStatus }) {
  const p = parseFxName(raw);
  return (
    <div className="flex min-h-6 items-center gap-2.5 px-1 py-[3px] text-[12px]" title={raw}>
      <span className="min-w-0 flex-1 truncate">{p.name || raw}</span>
      {p.vendor && (
        <span className="max-w-[120px] shrink-0 truncate text-muted-foreground">{p.vendor}</span>
      )}
      {/* A fixed column on every row, so the vendors line up whether or not
          the row says "missing". */}
      {status && (
        <span className="w-10 shrink-0 text-right text-[11px] text-amber-500">
          {status === "missing" ? (
            t("pluginMissingShort")
          ) : (
            <span className="sr-only">
              {status === "have" ? t("pluginInstalled") : t("pluginUnknown")}
            </span>
          )}
        </span>
      )}
    </div>
  );
}

function PluginList({ plugins }: { plugins: string[] }) {
  const installed = useInstalledPlugins();
  const [copied, setCopied] = useState(false);
  const rows = useMemo(
    () =>
      plugins
        .map((raw) => ({ raw, status: pluginStatus(raw, installed) }))
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
      <PluginBox>
        {rows.map((r) => (
          <PluginRow key={r.raw} raw={r.raw} status={r.status} />
        ))}
      </PluginBox>
    </div>
  );
}

export function TrustDialog({
  open,
  onOpenChange,
  name,
  hostId,
  hostName,
  bytes,
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
  /** Announce-level size (fallback when the preview lacks it). */
  bytes?: number;
  /** Manifest preview from a successful probe (may be null on older hosts). */
  preview?: SessionPreview | null;
  onJoin: (nofx: boolean) => void;
}) {
  const [nofx, setNofx] = useState(false);
  const totalBytes = preview?.bytes ?? bytes ?? 0;
  const plugins = preview?.meta?.plugins;
  // Read when the dialog opens: the count is a memory of trusting this identity.
  const seen = useMemo(() => (open && hostId ? hostSeenCount(hostId) : 0), [open, hostId]);

  // A host on another REAPER version: state chunks are REAPER's own format, so
  // a newer host's project may not load faithfully here (and the other way
  // round for edits). Silent when either side doesn't report a version.
  const hostReaper = preview?.meta?.reaper;
  const localReaper = useActiveInstance()?.state?.reaper;
  const mismatch = !!hostReaper && !!localReaper && hostReaper !== localReaper;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("trustTitle", { name })}</DialogTitle>
          <DialogDescription>{t("trustBody")}</DialogDescription>
        </DialogHeader>

        {/* Only what could change the decision: who hosts it, how big it is,
            and a REAPER version that doesn't match. */}
        <DialogFacts>
          {(hostName || hostId) && (
            <DialogFact label={t("trustHostLabel")}>
              {hostName && <span className="truncate">{hostName}</span>}
              {hostId && (
                <span className="shrink-0 font-normal text-muted-foreground">
                  {seen > 0 ? t("hostSeenTimes", { count: seen }) : t("hostSeenNever")}
                </span>
              )}
            </DialogFact>
          )}
          {totalBytes > 0 && (
            <DialogFact label={t("trustSizeLabel")}>
              <span className="tabular-nums">{formatBytes(totalBytes)}</span>
            </DialogFact>
          )}
          {mismatch && (
            <DialogFact label="REAPER">
              <span
                className="tabular-nums text-amber-500"
                title={t(
                  reaperVersionKey(hostReaper) > reaperVersionKey(localReaper)
                    ? "reaperMismatchHostNewer"
                    : "reaperMismatchHostOlder",
                  { host: hostReaper, local: localReaper },
                )}
              >
                {t("reaperYou", { host: hostReaper, local: localReaper })}
              </span>
            </DialogFact>
          )}
        </DialogFacts>

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
