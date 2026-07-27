import { useEffect } from "react";
import { Check, ShieldAlert } from "lucide-react";
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
import { FG_COLOR, FOUND_COLOR, SCAN_COLOR } from "@/lib/colors";
import { send } from "@/lib/connection";
import { formatBytes, useTransfer } from "@/lib/format";
import { resolveLang, t } from "@/lib/i18n";
import { parseFxName } from "@/lib/plugins";
import type { ExtState } from "@/lib/types";

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

const R = 30; // ring radius inside the 76px beacon box
const CIRC = 2 * Math.PI * R;

/** Keep the percentage and ring on the same spring. Rolling digits are too
 *  distracting at the transfer update rate. */
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

/** Expanding pulse shared with the disconnected screen. The ring is transparent
 *  at both ends of the loop to avoid a visible restart. */
function Breath({
  className,
  toScale,
  delay = 0,
}: {
  className: string;
  toScale: number;
  delay?: number;
}) {
  return (
    <motion.span
      className={className}
      style={{ borderColor: SCAN_COLOR }}
      initial={{ scale: 1, opacity: 0 }}
      animate={{ scale: toScale, opacity: [0, 0.5, 0] }}
      exit={{ opacity: 0, transition: { duration: 0.2 } }}
      transition={{
        scale: { duration: 2, repeat: Infinity, ease: [0, 0.2, 0.8, 1], delay },
        opacity: { duration: 2, repeat: Infinity, ease: "easeOut", times: [0, 0.2, 0.8], delay },
      }}
    />
  );
}

/** Join progress indicator: pulsing dot while connecting, progress ring during
 *  transfer, full ring while opening, and a checkmark when done. */
function ProgressBeacon({ phase, progress }: { phase: number; progress: number }) {
  const done = phase >= 3;
  // The opening phase holds the ring visually full even if the last progress
  // report landed a hair short of 1.
  const p = phase >= 2 ? 1 : Math.min(1, Math.max(0, progress));

  return (
    <div className="relative grid size-[76px] place-items-center">
      <AnimatePresence>
        {phase === 0 && (
          <>
            {/* Match the pulse used on the disconnected screen. */}
            <Breath
              key="b0"
              className="absolute size-[9px] rounded-full border-[1.5px]"
              toScale={4.2}
            />
            <Breath
              key="b1"
              className="absolute size-[9px] rounded-full border-[1.5px]"
              toScale={4.2}
              delay={1}
            />
          </>
        )}
        {/* Pulse the full ring while REAPER opens the project. */}
        {phase === 2 && (
          <Breath
            key="ring-breath"
            className="absolute inset-1 rounded-full border-[1.5px]"
            toScale={1.3}
          />
        )}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {phase >= 1 && (
          <motion.svg
            key="ring"
            viewBox="0 0 76 76"
            className="absolute inset-0 -rotate-90"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.35, ease: "easeOut" }}
          >
            <circle
              cx="38"
              cy="38"
              r={R}
              fill="none"
              strokeWidth="3"
              className="stroke-secondary"
            />
            <motion.circle
              cx="38"
              cy="38"
              r={R}
              fill="none"
              strokeWidth="3"
              strokeLinecap="round"
              strokeDasharray={CIRC}
              initial={{ strokeDashoffset: CIRC, stroke: FG_COLOR }}
              animate={{
                strokeDashoffset: CIRC * (1 - p),
                stroke: done ? FOUND_COLOR : FG_COLOR,
              }}
              transition={{
                strokeDashoffset: { type: "spring", stiffness: 80, damping: 20 },
                stroke: { duration: 0.45, ease: "easeOut" },
              }}
            />
          </motion.svg>
        )}
      </AnimatePresence>

      <AnimatePresence mode="wait" initial={false}>
        {phase === 0 ? (
          <motion.span
            key="dot"
            className="size-[9px] rounded-full"
            style={{ backgroundColor: SCAN_COLOR }}
            exit={{ scale: 0.5, opacity: 0 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
          />
        ) : done ? (
          <motion.span
            key="check"
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            transition={{ type: "spring", stiffness: 500, damping: 22 }}
            className="grid place-items-center"
            style={{ color: FOUND_COLOR }}
          >
            <Check className="size-5" strokeWidth={2.5} />
          </motion.span>
        ) : (
          <motion.span
            key="pct"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="text-[13px] font-semibold tabular-nums"
          >
            <GlidingPercent progress={p} />
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Show the active filename without moving the rows above it. */
function CurrentFile({ path }: { path?: string }) {
  const name = path ? (path.split(/[/\\]/).pop() ?? path) : "";
  return (
    <div className="relative h-4 overflow-hidden">
      <AnimatePresence initial={false}>
        {name && (
          <motion.p
            key={name}
            initial={{ y: 10, opacity: 0 }}
            animate={{ y: 0, opacity: 1 }}
            exit={{ y: -10, opacity: 0 }}
            transition={{ duration: 0.2, ease: "easeOut" }}
            className="absolute inset-0 truncate text-center text-[11px] text-muted-foreground"
          >
            {name}
          </motion.p>
        )}
      </AnimatePresence>
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
  const filesDone = state.files_done || 0;
  const filesTotal = state.files_total || 0;

  // Keep file count, bytes, and rate in a secondary line. The file count still
  // provides useful progress when one large file stalls the byte counter.
  const detail = [
    filesTotal > 0 ? t("fileOfTotal", { done: filesDone, total: filesTotal }) : null,
    total > 0 ? `${formatBytes(doneBytes, lang)} / ${formatBytes(total, lang)}` : null,
    rate > 0 ? `${formatBytes(rate, lang)}/s` : null,
  ]
    .filter(Boolean)
    .join(" · ");

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
    <div className="flex flex-1 flex-col items-center [justify-content:safe_center] overflow-y-auto gap-6 px-10 text-center">
      <div
        role="progressbar"
        aria-label={t("downloadProgress")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={phase === 1 ? Math.round((state.progress || 0) * 100) : undefined}
      >
        <ProgressBeacon phase={phase} progress={state.progress || 0} />
      </div>

      <div className="w-full space-y-1.5">
        <h2
          className="truncate text-[15px] font-semibold tracking-tight"
          title={state.session || undefined}
        >
          {state.session || t("sessionFallback")}
        </h2>
        {/* Crossfade on phase changes; update stats in place. Reserve the line
            height so status changes do not move the progress ring. */}
        <div className="relative h-[18px]" role="status" aria-live="polite">
          <AnimatePresence initial={false}>
            <motion.p
              key={lineKey}
              initial={{ y: 6, opacity: 0 }}
              animate={{ y: 0, opacity: 1 }}
              exit={{ y: -6, opacity: 0 }}
              transition={{ duration: 0.2, ease: "easeOut" }}
              className="absolute inset-0 truncate text-[13px] tabular-nums text-muted-foreground"
              style={phase >= 3 ? { color: FOUND_COLOR } : undefined}
            >
              {line}
            </motion.p>
          </AnimatePresence>
        </div>
        {/* Reserve the stats row's height even in phases with no transfer data. */}
        <div className="h-4">
          <AnimatePresence initial={false}>
            {phase === 1 && detail && (
              <motion.p
                key="detail"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.2, ease: "easeOut" }}
                className="truncate text-[11px] tabular-nums text-muted-foreground"
              >
                {detail}
              </motion.p>
            )}
          </AnimatePresence>
        </div>
        <CurrentFile path={phase === 1 ? state.current_file : undefined} />
      </div>

      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        onClick={() => send({ cmd: "leave" }, port ?? undefined)}
        // Allow cancellation during stalls and plugin verification. Disable it
        // only while REAPER is actively opening the project.
        disabled={!stalled && phase >= 2 && !state.verify}
      >
        {t("cancel")}
      </Button>

      {/* The received project loads plugins the host's list didn't mention (or
          the host listed nothing); the extension holds the open until the
          user decides. This is what makes the join preview a verified claim
          rather than a reassuring picture. */}
      <Dialog
        open={!!state.verify}
        onOpenChange={(open) => !open && send({ cmd: "leave" }, port ?? undefined)}
      >
        <DialogContent className="max-w-[360px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <ShieldAlert className="size-5 text-amber-500" aria-hidden="true" />
            </div>
            <DialogTitle className="text-[15px]">{t("verifyTitle")}</DialogTitle>
            <DialogDescription className="text-left text-[13px] leading-relaxed">
              {state.verify?.listed
                ? t("verifyBodyListed", { count: state.verify.extras.length })
                : t("verifyBodyNoList")}
            </DialogDescription>
          </DialogHeader>
          <div className="scroll-slim max-h-36 space-y-0.5 overflow-y-auto rounded-lg bg-secondary/50 p-1.5">
            {(state.verify?.extras ?? []).map((raw) => {
              const p = parseFxName(raw);
              return (
                <div key={raw} className="flex items-center gap-2 px-1 py-0.5" title={raw}>
                  <span
                    aria-hidden="true"
                    className="size-1.5 shrink-0 rounded-full bg-amber-400"
                  />
                  <span className="min-w-0 flex-1 truncate text-[12px]">
                    {p.name || raw}
                    {p.vendor && <span className="text-muted-foreground"> · {p.vendor}</span>}
                  </span>
                  {p.fmt && (
                    <span className="shrink-0 text-[10px] uppercase text-muted-foreground">
                      {p.fmt}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button
              variant="outline"
              className="flex-1"
              onClick={() => send({ cmd: "leave" }, port ?? undefined)}
            >
              {t("cancel")}
            </Button>
            <Button
              className="flex-1"
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
