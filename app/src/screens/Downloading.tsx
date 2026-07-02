import { Check } from "lucide-react";
import { motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { send } from "@/lib/connection";
import { formatBytes, useRate } from "@/lib/format";
import { resolveLang, t } from "@/lib/i18n";
import type { ExtState } from "@/lib/types";
import { cn } from "@/lib/utils";

// Joining is three distinct waits (reach the host → transfer the project →
// REAPER opens it); showing them as steps makes each one feel shorter than a
// single bar that crawls, stalls at 100%, then sits there.
const PHASES = ["phaseConnect", "phaseDownload", "phaseOpen"] as const;

function phaseIndex(state: ExtState): number {
  if (state.opening || state.state === "ready") return 2;
  if (state.state === "downloading") return 1;
  return 0; // connecting: join sent, waiting for the host's manifest
}

function StepDot({ status }: { status: "done" | "active" | "pending" }) {
  return (
    <span
      className={cn(
        "grid size-4 shrink-0 place-items-center rounded-full transition-colors duration-300",
        status === "done" && "bg-foreground text-background",
        status === "active" && "bg-foreground/15",
        status === "pending" && "bg-secondary",
      )}
    >
      {status === "done" ? (
        <Check className="size-2.5" strokeWidth={3} />
      ) : (
        <span
          className={cn(
            "size-1.5 rounded-full",
            status === "active" ? "animate-pulse bg-foreground" : "bg-muted-foreground/40",
          )}
        />
      )}
    </span>
  );
}

function Steps({ current }: { current: number }) {
  return (
    <div className="flex w-full items-center">
      {PHASES.map((key, i) => (
        <div key={key} className={cn("flex items-center", i > 0 && "flex-1")}>
          {i > 0 && (
            <div className="relative mx-2 h-px flex-1 overflow-hidden rounded bg-border">
              <motion.div
                className="absolute inset-y-0 left-0 bg-foreground/60"
                initial={false}
                animate={{ width: current >= i ? "100%" : "0%" }}
                transition={{ duration: 0.4, ease: "easeOut" }}
              />
            </div>
          )}
          <div className="flex items-center gap-1.5">
            <StepDot status={i < current ? "done" : i === current ? "active" : "pending"} />
            <span
              className={cn(
                "text-[11px] font-medium transition-colors duration-300",
                i === current ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {t(key)}
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}

/** Slim indeterminate sweep for the phases that have no measurable progress. */
function SweepBar() {
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
      <motion.div
        className="h-full w-1/3 rounded-full bg-foreground/50"
        animate={{ x: ["-120%", "320%"] }}
        transition={{ duration: 1.6, repeat: Infinity, ease: "easeInOut" }}
      />
    </div>
  );
}

function eta(remaining: number, rate: number): string | null {
  if (rate <= 0 || remaining <= 0) return null;
  const s = remaining / rate;
  if (s < 90) return t("etaSeconds", { s: Math.max(1, Math.round(s)) });
  return t("etaMinutes", { m: Math.round(s / 60) });
}

export function Downloading({ state }: { state: ExtState }) {
  const phase = phaseIndex(state);
  const lang = resolveLang();
  const pct = Math.round((state.progress || 0) * 100);
  const done = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const filesLeft = Math.max(0, (state.files_total || 0) - (state.files_done || 0));
  const rate = useRate(done);
  const remainingTime = eta(total - done, rate);

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-8 px-8">
      <div className="w-full space-y-3 text-center">
        <p className="text-[13px] text-muted-foreground">{t("joiningLabel")}</p>
        <h2 className="truncate text-lg font-semibold tracking-tight">
          {state.session || t("sessionFallback")}
        </h2>
      </div>

      <div className="w-full space-y-4">
        <Steps current={phase} />

        {phase === 1 ? (
          <div className="space-y-2">
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
              <motion.div
                className="h-full rounded-full bg-foreground"
                initial={{ width: 0 }}
                animate={{ width: `${pct}%` }}
                transition={{ type: "spring", stiffness: 80, damping: 20 }}
              />
            </div>
            <div className="flex justify-between text-[12px] text-muted-foreground">
              <span className="tabular-nums">
                {total > 0
                  ? `${formatBytes(done, lang)} / ${formatBytes(total, lang)}`
                  : t("downloadingProject")}
              </span>
              <span className="tabular-nums">
                {rate > 0 && `${formatBytes(rate, lang)}/s · `}
                {pct}%
              </span>
            </div>
            <div className="flex justify-between text-[12px] text-muted-foreground/70">
              <span className="tabular-nums">
                {filesLeft > 0 &&
                  (filesLeft === 1
                    ? t("oneFileRemaining")
                    : t("filesRemaining", { count: filesLeft }))}
              </span>
              <span className="tabular-nums">{remainingTime}</span>
            </div>
          </div>
        ) : (
          <div className="space-y-2">
            <SweepBar />
            <p className="text-center text-[12px] text-muted-foreground">
              {phase === 0 ? t("connectingToHost") : t("openingInReaper")}
            </p>
          </div>
        )}
      </div>

      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        onClick={() => send({ cmd: "leave" })}
        disabled={phase === 2}
      >
        {t("cancel")}
      </Button>
    </div>
  );
}
