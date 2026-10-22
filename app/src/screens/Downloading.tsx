import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Check, Crown } from "lucide-react";
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
import { SlowHostLine } from "@/components/LinkNotice";
import { PeerAvatar } from "@/components/PeerAvatar";
import { PluginBox, PluginRow } from "@/components/TrustDialog";
import { FOUND_COLOR } from "@/lib/colors";
import { send } from "@/lib/connection";
import { formatBytes, useTransfer } from "@/lib/format";
import { resolveLang, t } from "@/lib/i18n";
import { displayName, type ExtState, type Peer } from "@/lib/types";

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

// The narration line's swap: the arriving line springs in, the leaving one is
// a little quicker so the two cross as one change (as in components/Swap.tsx).
const LINE_IN = {
  scale: { type: "spring", stiffness: 460, damping: 30, mass: 0.55 },
  default: { duration: 0.2, ease: [0.16, 1, 0.3, 1] },
} as const;
const LINE_OUT = { duration: 0.16, ease: [0.16, 1, 0.3, 1] } as const;

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

/** How many faces fit before the rest collapse into a count. */
const MAX_FACES = 5;

/** Who is already there: just their faces, the host first with a crown badge,
 *  names on hover. */
function WhoIsThere({ state }: { state: ExtState }) {
  const others = state.peers.filter((p) => !p.is_self);
  const host = others.find((p) => p.is_owner);
  if (!host) return null;
  const people = [host, ...others.filter((p) => !p.is_owner)];
  return <Faces shown={people.slice(0, MAX_FACES)} rest={Math.max(0, people.length - MAX_FACES)} />;
}

// One name tag for the whole row instead of a tooltip per face: it glides from
// face to face and fits its width to the name, so moving along the row reads
// as one thing following the pointer rather than tags closing and opening.
const TAG_GLIDE = { type: "spring", stiffness: 360, damping: 32, mass: 0.6 } as const;
const TAG_EASE = [0.16, 1, 0.3, 1] as const;
const TAG_DELAY_MS = 120; // as long as the other hints in the app wait
const TAG_WARM_MS = 300; // shortly after it closed, the next face shows at once

function Faces({ shown, rest }: { shown: Peer[]; rest: number }) {
  const faces = useRef<(HTMLSpanElement | null)[]>([]);
  const measure = useRef<HTMLSpanElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const closedAt = useRef(0);
  const [at, setAt] = useState<number | null>(null);
  const [width, setWidth] = useState(0);
  // Gliding from one face to the next; a fresh tag just takes its size.
  const [moved, setMoved] = useState(false);

  const label = (p: Peer) => (p.is_owner ? `${displayName(p)} · ${t("sessionHost")}` : displayName(p));
  const current = at != null ? shown[at] : undefined;
  const text = current ? label(current) : "";
  const face = at != null ? faces.current[at] : null;
  const x = face ? face.offsetLeft + face.offsetWidth / 2 : 0;

  useLayoutEffect(() => {
    // + 2 for the border; the measuring span carries the padding.
    if (text && measure.current) setWidth(measure.current.offsetWidth + 2);
  }, [text]);
  useEffect(() => () => clearTimeout(timer.current), []);

  const enter = (i: number) => {
    clearTimeout(timer.current);
    setMoved(at != null);
    if (at != null || Date.now() - closedAt.current < TAG_WARM_MS) setAt(i);
    else timer.current = setTimeout(() => setAt(i), TAG_DELAY_MS);
  };
  const leave = () => {
    clearTimeout(timer.current);
    if (at != null) closedAt.current = Date.now();
    setAt(null);
  };

  return (
    <div className="relative flex items-center gap-2" onPointerLeave={leave}>
      {shown.map((p, i) => (
        <span
          key={p.id}
          ref={(el) => {
            faces.current[i] = el;
          }}
          className="relative"
          onPointerEnter={() => enter(i)}
        >
          <PeerAvatar peer={p} size="sm" />
          {p.is_owner && (
            <span className="absolute -top-1.5 -right-1.5 grid size-3.5 place-items-center rounded-full bg-background">
              <Crown className="size-2.5 text-amber-400" aria-hidden="true" />
              <span className="sr-only">{t("sessionHost")}</span>
            </span>
          )}
        </span>
      ))}
      {rest > 0 && <span className="text-[12px] tabular-nums text-muted-foreground">+{rest}</span>}
      {/* Sizes the tag for the name it is about to show. */}
      <span
        ref={measure}
        aria-hidden="true"
        className="invisible absolute top-0 left-0 px-2 text-[11px] font-medium whitespace-nowrap"
      >
        {text}
      </span>
      {/* The faces carry their names for screen readers; the tag is for eyes. */}
      <AnimatePresence>
        {current && (
          <motion.div
            key="tag"
            aria-hidden="true"
            className="pointer-events-none absolute top-full left-0 z-50 mt-1.5"
            initial={{ x, opacity: 0, scale: 0.95 }}
            animate={{ x, opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.12, ease: TAG_EASE } }}
            transition={{ x: TAG_GLIDE, default: { duration: 0.18, ease: TAG_EASE } }}
          >
            <motion.div
              className="-translate-x-1/2 overflow-hidden rounded-lg border border-border bg-background px-2 py-1 text-[11px] font-medium whitespace-nowrap text-foreground shadow-lg"
              initial={false}
              animate={{ width }}
              transition={moved ? TAG_GLIDE : { duration: 0 }}
            >
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={text}
                  className="inline-block"
                  initial={{ opacity: 0, filter: "blur(2px)" }}
                  animate={{ opacity: 1, filter: "blur(0px)" }}
                  exit={{ opacity: 0, filter: "blur(2px)" }}
                  transition={{ duration: 0.16, ease: TAG_EASE }}
                >
                  {text}
                </motion.span>
              </AnimatePresence>
            </motion.div>
          </motion.div>
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
        {/* One narration line per wait, trading places on phase changes with
            the blur swap the join button uses; the stats update in place
            without re-animating. Fixed-height so the swap never shifts the
            beacon above. */}
        <div className="relative h-[18px]" role="status" aria-live="polite">
          <AnimatePresence initial={false}>
            <motion.p
              key={lineKey}
              initial={{ opacity: 0, scale: 0.9, filter: "blur(2px)" }}
              animate={{ opacity: 1, scale: 1, filter: "blur(0px)", transition: LINE_IN }}
              exit={{ opacity: 0, scale: 0.96, filter: "blur(2px)", transition: LINE_OUT }}
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
