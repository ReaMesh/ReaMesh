// Show the host, project size, and local plugin availability before joining.
// The extension verifies the manifest's plugin list against the downloaded project.
// The caution about plugin code is for a host never joined before: said on
// every join, it stops being read.

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowUpRight, ChevronRight } from "lucide-react";
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
import { Hint } from "@/components/Hint";
import { useActiveInstance } from "@/lib/connection";
import { findLink, joinInto, linkLabel, linkStateOf, type LinkedCopy, type LinkState } from "@/lib/copyLink";
import { formatBytes, reaperVersionKey } from "@/lib/format";
import { noticeApplies, noticeLine } from "@/lib/relayInfo";
import { t } from "@/lib/i18n";
import { hostSeenCount } from "@/lib/knownHosts";
import { formatMismatch, parseFxName, pluginStatus, useInstalledPlugins, type PluginStatus } from "@/lib/plugins";
import { pluginSite } from "@/lib/pluginSites";
import { refreshLinkedCopies } from "@/lib/savedCopies";
import type { SessionPreview } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

/** A plugin list: name, then vendor, one plugin per row. Tall enough for
 *  eight rows; a longer list scrolls with the ninth row half in view, so the
 *  cut never hides that there is more. */
export function PluginBox({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div className={cn("scroll-slim relative max-h-[216px] overflow-y-auto rounded-xl bg-white/[0.04] p-1.5", className)}>
      {children}
    </div>
  );
}

export function PluginRow({ raw }: { raw: string }) {
  const p = parseFxName(raw);
  return (
    <Hint label={raw} slow>
      <div className="flex min-h-6 items-center gap-2.5 px-1 py-[3px] text-[12px]">
        <span className="min-w-0 flex-1 truncate">{p.name || raw}</span>
        {p.vendor && (
          <span className="max-w-[120px] shrink-0 truncate text-muted-foreground">{p.vendor}</span>
        )}
      </div>
    </Hint>
  );
}

const FOLD = { type: "spring", stiffness: 400, damping: 38 } as const;

/** A plugin this machine can't load: how many tracks use it, then where to get
 *  it, or which format it is installed in instead. */
function MissingRow({ raw, tracks, other }: { raw: string; tracks?: number; other: string | null }) {
  const p = parseFxName(raw);
  const site = pluginSite(p);
  return (
    <div className="flex min-h-6 items-center gap-2 px-1 py-[3px] text-[12px]">
      <Hint label={raw} slow>
        <span className="min-w-0 truncate">{p.name || raw}</span>
      </Hint>
      {tracks !== undefined && (
        <span className="shrink-0 text-muted-foreground">{t("pluginTracks", { count: tracks })}</span>
      )}
      <span className="min-w-2 flex-1" />
      {other ? (
        <span className="shrink-0 text-muted-foreground">{other}</span>
      ) : (
        <Hint label={site.url} slow>
          <button
            type="button"
            onClick={() => openUrl(site.url).catch(() => {})}
            className={cn(
              "inline-flex max-w-[190px] shrink-0 items-center gap-1.5 rounded text-found underline-offset-[3px] transition-colors duration-300 hover:underline group-data-[off]:text-muted-foreground",
              focusRing,
            )}
          >
            <span className="truncate">{site.host || t("pluginSearch")}</span>
            <ArrowUpRight className="size-3 shrink-0" aria-hidden="true" />
          </button>
        </Hint>
      )}
    </div>
  );
}

/** One line that says how many plugins a group holds and unfolds into them. */
function FoldedPlugins({ label, plugins }: { label: string; plugins: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "flex h-6 w-full items-center justify-between rounded px-1 text-[12px] font-medium text-muted-foreground transition-colors hover:text-foreground",
          focusRing,
        )}
      >
        {label}
        <ChevronRight
          className={cn("size-3.5 transition-transform duration-200", open && "rotate-90")}
          aria-hidden="true"
        />
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={FOLD}
            className="overflow-hidden"
          >
            {plugins.map((raw) => (
              <PluginRow key={raw} raw={raw} />
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The project's plugins by what joining means for each: the ones this machine
 *  can't load, then the rest folded into a line. Joining without effects loads
 *  none of them, so the list steps back and nothing in it is a warning. */
function PluginList({ plugins, tracks, nofx }: { plugins: string[]; tracks?: number[]; nofx: boolean }) {
  const installed = useInstalledPlugins();
  const { missing, rest, known } = useMemo(() => {
    const counted = Array.isArray(tracks) && tracks.length === plugins.length;
    const rows = plugins.map((raw, i) => {
      const n = counted ? Number(tracks[i]) : NaN;
      return {
        raw,
        status: pluginStatus(raw, installed),
        tracks: Number.isFinite(n) && n >= 1 ? Math.floor(n) : undefined,
        other: formatMismatch(raw, installed),
      };
    });
    const absent = (s: PluginStatus) => s === "missing" || s === "format";
    return {
      // What most of the project leans on comes first.
      missing: rows.filter((r) => absent(r.status)).sort((a, b) => (b.tracks ?? 0) - (a.tracks ?? 0)),
      rest: rows.filter((r) => !absent(r.status)).map((r) => r.raw),
      known: rows.some((r) => r.status !== "unknown"),
    };
  }, [plugins, tracks, installed]);

  return (
    <div
      data-off={nofx || undefined}
      className="group shrink-0 rounded-xl bg-white/[0.04] p-2 transition-opacity duration-300 data-[off]:opacity-40"
    >
      {missing.length > 0 && (
        <>
          <p className="flex h-6 items-center px-1 text-[12px] font-medium text-amber-500 transition-colors duration-300 group-data-[off]:text-muted-foreground">
            {t("pluginsMissing", { count: missing.length })}
          </p>
          {missing.map((r) => (
            <MissingRow
              key={r.raw}
              raw={r.raw}
              tracks={r.tracks}
              other={r.other && t("pluginOtherFormat", r.other)}
            />
          ))}
        </>
      )}
      {missing.length > 0 && rest.length > 0 && <div className="mx-1 my-[7px] h-px bg-white/[0.07]" />}
      {rest.length > 0 && (
        <FoldedPlugins
          label={
            !known
              ? t("pluginsUnknown", { total: rest.length })
              : missing.length > 0
                ? t("pluginsInstalled", { count: rest.length })
                : t("pluginsAllInstalled", { total: rest.length })
          }
          plugins={rest}
        />
      )}
    </div>
  );
}

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
        <Hint key={String(v)} label={options[v ? 1 : 0].title}>
          <button
            type="button"
            role="radio"
            aria-checked={value === v}
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
        </Hint>
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
  via,
  preview,
  viaInvite = false,
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
  /** Found on a relay's list: which relays, and the operator's name for the
   *  host's key. The host's name above is the host's own word, this is the
   *  relay's, so both are shown. */
  via?: { names: string; keyLabel: string };
  /** Manifest preview from a successful probe (may be null on older hosts). */
  preview?: SessionPreview | null;
  /** Joining by invite code, the path that can run over the host's relay. */
  viaInvite?: boolean;
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
  const caution = seen === 0;

  // A host on another REAPER version: state chunks are REAPER's own format, so
  // a newer host's project may not load faithfully here (and the other way
  // round for edits). Silent when either side doesn't report a version.
  const hostReaper = preview?.meta?.reaper;
  const localReaper = useActiveInstance()?.state?.reaper;
  // What the host's relay announces. On the LAN the relay plays no part.
  const relayNotices = viaInvite
    ? (Array.isArray(preview?.meta?.relay_notices) ? preview.meta.relay_notices : [])
        .filter((n) => typeof n?.text === "string" && noticeApplies(n))
        .slice(0, 3)
    : [];
  const mismatch = !!hostReaper && !!localReaper && hostReaper !== localReaper;

  // The download would refuse to start for lack of room, so say it here. That is
  // asked of a temporary copy; a linked folder is left to the download itself.
  const noRoom = !into.dest && preview?.enough === false;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* Facts and plugins scroll together when the window is too short for
          them: the choices and the buttons never leave the panel. */}
      <DialogContent
        className="flex flex-col gap-3.5 overflow-hidden"
        {...(caution ? {} : { "aria-describedby": undefined })}
      >
        <DialogHeader className="shrink-0">
          <DialogTitle className="line-clamp-2 wrap-anywhere">{t("trustTitle", { name })}</DialogTitle>
          {caution && <DialogDescription>{t(nofx ? "trustBodyNoFx" : "trustBody")}</DialogDescription>}
        </DialogHeader>

        <div className="scroll-slim flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto">
          {/* Only what could change the decision: who hosts it, how big it is,
              whether it fits, what happens to a copy of it, and a REAPER version
              that doesn't match. */}
          <DialogFacts className="shrink-0">
            {(hostName || hostId) && (
              <DialogFact label={t("trustHostLabel")}>
                {hostName && <span className="min-w-0 truncate">{hostName}</span>}
                {hostId && (
                  <span className={cn("shrink-0 font-normal", seen > 0 ? "text-muted-foreground" : "text-amber-500")}>
                    {seen > 0 ? t("hostSeenTimes", { count: seen }) : t("hostSeenNever")}
                  </span>
                )}
              </DialogFact>
            )}
            {via && (
              <DialogFact label={t("trustViaLabel")}>
                <span className="flex min-w-0 flex-col items-end">
                  <span className="max-w-full truncate">{via.names}</span>
                  {via.keyLabel && (
                    <span className="max-w-full truncate font-normal text-muted-foreground">
                      {t("trustViaKey", { key: via.keyLabel })}
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
                <Hint label={found.copy.path}>
                  <span className="min-w-0 truncate">
                    {linkLabel(found.copy)}
                  </span>
                </Hint>
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
                <Hint
                  label={t(
                    reaperVersionKey(hostReaper) > reaperVersionKey(localReaper)
                      ? "reaperMismatchHostNewer"
                      : "reaperMismatchHostOlder",
                    { host: hostReaper, local: localReaper },
                  )}
                >
                  <span className="tabular-nums text-amber-500">
                    {t("reaperYou", { host: hostReaper, local: localReaper })}
                  </span>
                </Hint>
              </DialogFact>
            )}
            {relayNotices.map((n) => (
              <DialogFact key={n.text} label={t("relayNoticeJoin")}>
                {noticeLine(n)}
              </DialogFact>
            ))}
          </DialogFacts>

          {plugins && plugins.length > 0 && (
            <PluginList plugins={plugins} tracks={preview?.meta?.plugin_tracks} nofx={nofx} />
          )}
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
            <SwapText value={nofx ? "view" : "join"}>{t(nofx ? "joinViewOnly" : "joinAnyway")}</SwapText>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
