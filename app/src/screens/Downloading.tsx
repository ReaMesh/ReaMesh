import { Check } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { Rolling } from "@/components/Rolling";
import { FG_COLOR, FOUND_COLOR, SCAN_COLOR } from "@/lib/colors";
import { send } from "@/lib/connection";
import { formatBytes, useRate } from "@/lib/format";
import { resolveLang, t } from "@/lib/i18n";
import type { ExtState } from "@/lib/types";

// The beacon covers three stages: connecting, downloading, and opening the
// project in REAPER. Phase 3 is the brief completion state owned by App.
function phaseIndex(state: ExtState): number {
  if (state.opening || state.state === "ready") return 2;
  if (state.state === "downloading") return 1;
  return 0; // connecting: join sent, waiting for the host's manifest
}

const R = 30; // ring radius inside the 76px beacon box
const CIRC = 2 * Math.PI * R;

/** One expanding-and-fading breath ring: the Disconnected beacon's pulse.
 *  Opacity is 0 at both ends so the loop boundary never snap-flashes, and hits
 *  full transparency at 80% so the ring doesn't linger at its outer edge. */
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
  const pct = Math.round(p * 100);

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
            <Rolling text={`${pct}%`} />
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
            className="absolute inset-0 truncate text-center text-[11px] text-muted-foreground/60"
          >
            {name}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}

function eta(remaining: number, rate: number): string | null {
  if (rate <= 0 || remaining <= 0) return null;
  const s = remaining / rate;
  if (s < 90) return t("etaSeconds", { s: Math.max(1, Math.round(s)) });
  return t("etaMinutes", { m: Math.round(s / 60) });
}

// `done` is App's completion beat: the project just opened (mode is already
// client) and this screen is held for a moment so the ring can sweep to the
// found-blue and the checkmark can pop before handing off to Session.
export function Downloading({ state, done = false }: { state: ExtState; done?: boolean }) {
  const phase = done ? 3 : phaseIndex(state);
  const lang = resolveLang();
  const doneBytes = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const rate = useRate(doneBytes);
  const remainingTime = eta(total - doneBytes, rate);

  // While downloading, the narration line *is* the numbers: one quiet line,
  // not a scatter of stat corners.
  const stats = [
    total > 0 ? `${formatBytes(doneBytes, lang)} / ${formatBytes(total, lang)}` : null,
    rate > 0 ? `${formatBytes(rate, lang)}/s` : null,
    remainingTime,
  ]
    .filter(Boolean)
    .join(" · ");

  const line =
    phase >= 3
      ? t("downloadReady")
      : phase === 2
        ? t("openingInReaper")
        : phase === 1
          ? stats || t("downloadingProject")
          : t("connectingToHost");

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-6 px-10 text-center">
      <ProgressBeacon phase={phase} progress={state.progress || 0} />

      <div className="w-full space-y-1.5">
        <h2 className="truncate text-[15px] font-semibold tracking-tight">
          {state.session || t("sessionFallback")}
        </h2>
        {/* One narration line per wait, crossfading on phase changes; the
            stats update in place without re-animating. Fixed-height so the
            swap never shifts the beacon above. */}
        <div className="relative h-[18px]">
          <AnimatePresence initial={false}>
            <motion.p
              key={phase}
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
        <CurrentFile path={phase === 1 ? state.current_file : undefined} />
      </div>

      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        onClick={() => send({ cmd: "leave" })}
        disabled={phase >= 2}
      >
        {t("cancel")}
      </Button>
    </div>
  );
}
