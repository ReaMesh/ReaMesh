// Show the host, project size, and local plugin availability before joining.
// The extension verifies the manifest's plugin list against the downloaded project.

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
import { SwapText } from "@/components/Swap";
import { useActiveInstance } from "@/lib/connection";
import { findLink, joinInto, linkStateOf, type LinkedCopy, type LinkState } from "@/lib/copyLink";
import { formatBytes, reaperVersionKey } from "@/lib/format";
import { t } from "@/lib/i18n";
import { hostSeenCount } from "@/lib/knownHosts";
import { parseFxName, pluginStatus, useInstalledPlugins, type PluginStatus } from "@/lib/plugins";
import { folderName, refreshLinkedCopies } from "@/lib/savedCopies";
import type { SessionPreview } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

// Sort missing plugins first.
const STATUS_ORDER: Record<PluginStatus, number> = { missing: 0, unknown: 1, have: 2 };

/** A plugin list: name, then vendor, one plugin per row. Tall enough for
 *  eight rows; a longer list scrolls with the ninth row half in view, so the
 *  cut never hides that there is more. `relative` keeps the rows' screen-reader
 *  text inside the scroller, where it can't stretch the dialog around it. */
export function PluginBox({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("scroll-slim relative max-h-[216px] overflow-y-auto rounded-xl bg-white/[0.04] p-1.5", className)}>
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
    <div className="flex min-h-[86px] shrink flex-col gap-1.5">
      <div className="flex shrink-0 items-center justify-between gap-2">
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
            <SwapText value={copied ? "copied" : "copy"}>
              {copied ? t("missingCopied") : t("copyMissing")}
            </SwapText>
          </button>
        )}
      </div>
      {/* The one part of the dialog that gives way in a short window. */}
      <PluginBox className="min-h-0 flex-1">
        {rows.map((r) => (
          <PluginRow key={r.raw} raw={r.raw} status={r.status} />
        ))}
      </PluginBox>
    </div>
  );
}

const FOLD = { type: "spring", stiffness: 400, damping: 38 } as const;

/** Two options side by side, the chosen one on a surface that slides across. */
function Choice({
  label,
  layoutId,
  value,
  onChange,
  options,
}: {
  label: string;
  /** Unique per group on screen, or the surface jumps between them. */
  layoutId: string;
  value: boolean;
  onChange: (value: boolean) => void;
  /** The `false` option, then the `true` one. */
  options: [{ label: string; title?: string }, { label: string; title?: string }];
}) {
  return (
    <div role="radiogroup" aria-label={label} className="flex gap-1 rounded-[10px] bg-white/[0.04] p-1">
      {[false, true].map((v) => (
        <button
          key={String(v)}
          type="button"
          role="radio"
          aria-checked={value === v}
          title={options[v ? 1 : 0].title}
          onClick={() => onChange(v)}
          className={cn(
            "relative h-7 flex-1 rounded-md text-[12px] font-medium transition-colors duration-200",
            value === v ? "text-foreground" : "text-muted-foreground hover:text-foreground",
            focusRing,
          )}
        >
          {/* bg-secondary sat almost on the dialog's own colour. */}
          {value === v && (
            <motion.span
              layoutId={layoutId}
              className="absolute inset-0 rounded-md bg-foreground/[0.14]"
              transition={FOLD}
            />
          )}
          <span className="relative">{options[v ? 1 : 0].label}</span>
        </button>
      ))}
    </div>
  );
}

/** The linked copy this session would go into and how it compares, looked up as
 *  the dialog opens. `undefined` until that is known. */
function useCopyLink(open: boolean, hostId: string | undefined, preview: SessionPreview | null | undefined) {
  const [found, setFound] = useState<{ copy: LinkedCopy; state: LinkState } | null | undefined>();
  const project = preview?.meta?.project;
  const file = preview?.project_file;
  const hosted = preview?.project_hash ?? "";
  useEffect(() => {
    setFound(undefined);
    if (!open) return;
    let alive = true;
    (async () => {
      const copy = findLink(await refreshLinkedCopies(), project, hostId);
      if (!copy || !file) return alive && setFound(null);
      const local = await invoke<string | null>("link_state", { path: copy.path, projectFile: file }).catch(
        () => null,
      );
      if (alive) setFound({ copy, state: linkStateOf(local ?? null, copy.hash, hosted) });
    })();
    return () => {
      alive = false;
    };
  }, [open, project, hostId, file, hosted]);
  return found;
}

const LINK_STATE_TEXT = { same: "linkUpToDate", update: "linkWillUpdate", conflict: "linkChanged" } as const;

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
  /** `into` is what the `join` command needs for a linked copy. */
  onJoin: (nofx: boolean, into: { dest?: string; overwrite?: boolean }) => void;
}) {
  const [nofx, setNofx] = useState(false);
  // A temporary copy until asked otherwise, every time.
  const [overwrite, setOverwrite] = useState(false);
  useEffect(() => {
    if (open) setOverwrite(false);
  }, [open]);
  const found = useCopyLink(open, hostId, preview);
  const into = joinInto(found?.copy ?? null, found?.state ?? null, { nofx, overwrite });
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

  // The download would refuse to start for lack of room, so say it here. That is
  // asked of a temporary copy; a linked folder is left to the download itself.
  const noRoom = !into.dest && preview?.enough === false;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* The list takes what the rest leaves: in the smallest window the
          tighter gap puts its lower edge through a row, so it reads as more.
          With every row there is and no list left to give way, facts and list
          scroll: the choices and the buttons never leave the panel. */}
      <DialogContent className="flex flex-col gap-3.5 overflow-hidden">
        <DialogHeader className="shrink-0">
          <DialogTitle className="line-clamp-2 wrap-anywhere">{t("trustTitle", { name })}</DialogTitle>
          <DialogDescription>{t("trustBody")}</DialogDescription>
        </DialogHeader>

        <div className="scroll-slim flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto">
          {/* Only what could change the decision: who hosts it, how big it is,
              whether it fits, what happens to a copy of it, and a REAPER version
              that doesn't match. */}
          <DialogFacts className="shrink-0">
            {(hostName || hostId) && (
              <DialogFact label={t("trustHostLabel")}>
                <span className="flex min-w-0 flex-col items-end">
                  {hostName && <span className="max-w-full truncate">{hostName}</span>}
                  {hostId && (
                    <span className="font-normal text-muted-foreground">
                      {seen > 0 ? t("hostSeenTimes", { count: seen }) : t("hostSeenNever")}
                    </span>
                  )}
                </span>
              </DialogFact>
            )}
            {totalBytes > 0 && (
              <DialogFact label={t("trustSizeLabel")}>
                <span className="tabular-nums">{formatBytes(totalBytes)}</span>
              </DialogFact>
            )}
            {noRoom && (
              <DialogFact label={t("hostFreeLabel")}>
                <span className="tabular-nums">{formatBytes(preview.free ?? 0)}</span>
                <span className="font-normal text-red-400">{t("hostFreeShort")}</span>
              </DialogFact>
            )}
            {found && !nofx && (
              <DialogFact label={t("yourCopy")}>
                <span className="min-w-0 truncate" title={found.copy.path}>
                  {folderName(found.copy.path)}
                </span>
                <span
                  className={cn(
                    "shrink-0 font-normal",
                    found.state === "conflict" ? "text-amber-500" : "text-muted-foreground",
                  )}
                >
                  {t(LINK_STATE_TEXT[found.state])}
                </span>
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
        </div>

        {/* An equal choice, not a fine-print link: without effects no plugin
            code runs and the project opens view-only. */}
        <div className="shrink-0">
          <Choice
            label={t("joinModeLabel")}
            layoutId="join-mode-pill"
            value={nofx}
            onChange={setNofx}
            options={[{ label: t("joinWithFx") }, { label: t("joinWithoutFx"), title: t("joinNoFxHint") }]}
          />
        </div>

        {/* Your own changes and the host's can't both stay in that folder. A view-
            only join never goes there, so it doesn't come up. */}
        <AnimatePresence initial={false}>
          {found?.state === "conflict" && !nofx && (
            <motion.div
              key="link-choice"
              initial={{ height: 0, marginTop: -14, opacity: 0 }}
              animate={{ height: "auto", marginTop: 0, opacity: 1 }}
              exit={{ height: 0, marginTop: -14, opacity: 0 }}
              transition={FOLD}
              className="shrink-0 overflow-hidden"
            >
              <Choice
                label={t("linkChoiceLabel")}
                layoutId="link-mode-pill"
                value={overwrite}
                onChange={setOverwrite}
                options={[{ label: t("linkTemporary") }, { label: t("linkOverwrite"), title: t("linkOverwriteHint") }]}
              />
            </motion.div>
          )}
        </AnimatePresence>

        <DialogFooter className="shrink-0">
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button disabled={noRoom || found === undefined} onClick={() => onJoin(nofx, into)}>
            {t("joinAnyway")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
