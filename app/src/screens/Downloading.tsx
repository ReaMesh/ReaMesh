import { useEffect } from "react";
import { Check } from "lucide-react";
import {
  AnimatePresence,
  motion,
  useReducedMotion,
  useSpring,
  useTransform,
} from "motion/react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ArcRing } from "@/components/ArcRing";
import { HostCrown } from "@/components/HostCrown";
import { SlowHostLine } from "@/components/LinkNotice";
import { PeerAvatar } from "@/components/PeerAvatar";
import { PluginBox, PluginRow } from "@/components/TrustDialog";
import { FOUND_COLOR } from "@/lib/colors";
import { send } from "@/lib/connection";
import { formatBytes, useTransfer } from "@/lib/format";
import { resolveLang, t } from "@/lib/i18n";
import { displayName, type ExtState } from "@/lib/types";
import { cn } from "@/lib/utils";

// The beacon covers three stages: connecting, downloading, and opening the
// project in REAPER. Phase 3 is the brief completion state owned by App.
function phaseIndex(state: ExtState): number {
  // The extension publishes one final downloading frame before handling a
  // failure. Keep that frame in the transfer phase until the screen changes.
  if (state.state === "failed" || state.state === "ended") return 1;
  if (state.opening || state.state === "ready") return 2;
  if (state.state === "downloading") return 1;
  return 0; // connecting: join sent, waiting for the host's manifest
}

/** Use the same spring for the percentage and ring to keep them in sync. */
function GlidingPercent({ progress }: { progress: number }) {
  const reduce = useReducedMotion();
  const spring = useSpring(0, { stiffness: 80, damping: 20 });
  useEffect(() => {
    spring.set(progress);
  }, [spring, progress]);
  const text = useTransform(
    spring,
    (v) => `${Math.round(Math.max(0, Math.min(1, v)) * 100)}%`,
  );
  if (reduce) return <>{`${Math.round(progress * 100)}%`}</>;
  return <motion.span>{text}</motion.span>;
}

/** Join progress indicator: one ring throughout. It spins as an open arc while
 *  connecting, shows the transfer progress, spins again while REAPER opens the
 *  project, and closes around a checkmark when done. */
function ProgressBeacon({ phase, progress }: { phase: number; progress: number }) {
  const done = phase >= 3;
  const spinning = phase === 0 || phase === 2;

  return (
    <div className="relative grid size-[132px] place-items-center">
      <ArcRing
        size={132}
        radius={54}
        strokeWidth={5}
        spinning={spinning}
        progress={done ? 1 : progress}
        color={FOUND_COLOR}
      />

      <AnimatePresence mode="wait" initial={false}>
        {spinning ? null : done ? (
          <motion.span
            key="check"
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            transition={{ type: "spring", stiffness: 500, damping: 22 }}
            className="grid place-items-center"
            style={{ color: FOUND_COLOR }}
          >
            <Check className="size-9" strokeWidth={2.25} />
          </motion.span>
        ) : (
          <motion.span
            key="pct"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="text-[26px] font-semibold tracking-[-0.01em] tabular-nums"
          >
            <GlidingPercent progress={progress} />
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The file being fetched, as plain text; the slot keeps its height when empty. */
function CurrentFile({ path }: { path?: string }) {
  const name = path ? (path.split(/[/\\]/).pop() ?? path) : "";
  return (
    <div className="h-[18px]">
      <AnimatePresence initial={false}>
        {name && (
          <motion.p
            key="file"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="truncate text-[12px] text-muted-foreground"
          >
            {name}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Stands in for the host's name in a sentence, where their crown goes. */
const HOST_SLOT = "\u0001";

/** Who is already there, as one line: their faces and one sentence, the host
 *  first and crowned. */
function WhoIsThere({ state }: { state: ExtState }) {
  const others = state.peers.filter((p) => !p.is_self);
  const host = others.find((p) => p.is_owner);
  if (!host) return null;
  const guests = others.filter((p) => !p.is_owner);
  const sentence =
    guests.length === 0
      ? t("alreadyHereOne", { name: HOST_SLOT })
      : guests.length === 1
        ? t("alreadyHereTwo", { a: HOST_SLOT, b: displayName(guests[0]) })
        : t("alreadyHereMany", { a: HOST_SLOT, count: guests.length });
  const [before, after = ""] = sentence.split(HOST_SLOT);
  return (
    <div className="flex max-w-full items-center gap-2.5">
      <div className="flex shrink-0 items-center">
        {[host, ...guests].slice(0, 3).map((p, i) => (
          <div key={p.id} className={cn("relative", i > 0 && "-ml-1.5")} style={{ zIndex: 3 - i }}>
            <PeerAvatar peer={p} size="sm" className="ring-2 ring-background" />
          </div>
        ))}
      </div>
      <p className="min-w-0 truncate text-[13px] text-foreground/65">
        {before}
        <span className="inline-flex items-center gap-1 align-bottom">
          {displayName(host)}
          <HostCrown hint={false} />
        </span>
        {after}
      </p>
    </div>
  );
}

/** Format the estimated time remaining for the status line. */
function eta(remaining: number, rate: number): string | null {
  if (rate <= 0 || remaining <= 0) return null;
  const s = remaining / rate;
  if (s < 90) return t("etaAboutSeconds", { s: Math.max(1, Math.round(s)) });
  return t("etaAboutMinutes", { m: Math.round(s / 60) });
}

// App holds `done` briefly after the project opens so the completion animation
// can finish before the session screen appears.
export function Downloading({
  state,
  done = false,
  port,
}: {
  state: ExtState;
  done?: boolean;
  port?: number | null;
}) {
  const phase = done ? 3 : phaseIndex(state);
  const lang = resolveLang();
  const doneBytes = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const { rate, stalled } = useTransfer(doneBytes, total);
  const remainingTime = stalled ? null : eta(total - doneBytes, rate);

  // One number under the narration: how much of it is here. The time left
  // already stands above it.
  const detail =
    total > 0
      ? t("bytesOf", { done: formatBytes(doneBytes, lang), total: formatBytes(total, lang) })
      : "";

  const line =
    phase >= 3
      ? t("downloadReady")
      : phase === 2
        ? t("openingInReaper")
        : phase === 1
          ? stalled
            ? t("downloadStalled")
            : remainingTime || t("downloadingProject")
          : t("connectingToHost");
  // Only animate when the status type changes. ETA updates stay in place.
  const lineKey =
    phase === 1 ? (stalled ? "stall" : remainingTime ? "eta" : "dl") : String(phase);

  return (
    <div className="flex flex-1 flex-col items-center overflow-y-auto px-6 pt-7 text-center">
      <div
        role="progressbar"
        aria-label={t("downloadProgress")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={phase === 1 ? Math.round((state.progress || 0) * 100) : undefined}
      >
        <ProgressBeacon phase={phase} progress={state.progress || 0} />
      </div>

      <div className="mt-5 w-full space-y-1">
        <h2
          className="truncate text-[18px] font-semibold tracking-[-0.01em]"
          title={state.session || undefined}
        >
          {state.session || t("sessionFallback")}
        </h2>
        {/* One narration line per wait, crossfading on phase changes; the
            stats update in place without re-animating. Fixed-height so the
            swap never shifts the beacon above. */}
        <div className="relative h-[18px]" role="status" aria-live="polite">
          <AnimatePresence initial={false}>
            <motion.p
              key={lineKey}
              initial={{ y: 6, opacity: 0 }}
              animate={{ y: 0, opacity: 1 }}
              exit={{ y: -6, opacity: 0 }}
              transition={{ duration: 0.2, ease: "easeOut" }}
              className="absolute inset-0 truncate text-[13px] tabular-nums text-foreground/65"
              style={phase >= 3 ? { color: FOUND_COLOR } : undefined}
            >
              {line}
            </motion.p>
          </AnimatePresence>
        </div>
        {/* Bytes, dimmed under the narration; fixed-height so the other
            phases hold the same layout with the slot empty. */}
        <div className="h-[18px]">
          <AnimatePresence initial={false}>
            {phase === 1 && detail && (
              <motion.p
                key="detail"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.2, ease: "easeOut" }}
                className="truncate text-[12px] tabular-nums text-muted-foreground"
              >
                {detail}
              </motion.p>
            )}
          </AnimatePresence>
        </div>
        <CurrentFile path={phase === 1 ? state.current_file : undefined} />
      </div>

      {/* The host answers pings from the manifest on, so a slow VPN hop shows
          here before the first minute of "why is this taking so long". */}
      <div className="mt-7 flex w-full flex-col items-center gap-2">
        <WhoIsThere state={state} />
        {phase === 1 && <SlowHostLine peers={state.peers} />}
      </div>

      {/* The verification dialog below has its own Cancel; one is enough. */}
      {!state.verify && (
        <Button
          variant="ghost"
          size="sm"
          className="mt-auto mb-4 text-foreground/65"
          onClick={() => send({ cmd: "leave" }, port ?? undefined)}
          // Allow cancellation during stalls. Disable it only while REAPER is
          // actively opening the project.
          disabled={!stalled && phase >= 2}
        >
          {t("cancel")}
        </Button>
      )}

      {/* Hold projects with unlisted plugins until the user decides. */}
      {/* The dialog closes only through its explicit actions. */}
      <Dialog open={!!state.verify}>
        <DialogContent
          showCloseButton={false}
          onEscapeKeyDown={(e) => e.preventDefault()}
          onInteractOutside={(e) => e.preventDefault()}
        >
          <DialogHeader>
            <DialogTitle>{t("verifyTitle")}</DialogTitle>
            <DialogDescription>
              {state.verify?.listed ? t("verifyBodyListed") : t("verifyBodyNoList")}
            </DialogDescription>
          </DialogHeader>
          <PluginBox>
            {(state.verify?.extras ?? []).map((raw) => (
              <PluginRow key={raw} raw={raw} />
            ))}
          </PluginBox>
          <DialogFooter>
            <Button
              variant="secondary"
              onClick={() => send({ cmd: "leave" }, port ?? undefined)}
            >
              {t("cancel")}
            </Button>
            <Button
              onClick={() => send({ cmd: "open_confirm" }, port ?? undefined)}
            >
              {t("verifyContinue")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
