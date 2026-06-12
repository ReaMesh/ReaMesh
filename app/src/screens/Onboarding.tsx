import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { CheckCircle2, FolderSearch, Puzzle, TriangleAlert } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  installExtension,
  refreshStatus,
  setReaperPath,
  type ReaperStatus,
} from "@/lib/install";

// First-run onboarding: shown until the bundled REAPER extension is
// installed. Two entry states: REAPER's folder wasn't found (point us at
// it), or it was (one-click install), plus a success step that hands off
// to the normal app on Finish.

const BAR_COLORS = ["#f0846b", "#e8d35c", "#7bd88f", "#5b8def"];
const BAR_HEIGHTS = [22, 34, 28, 38];

function Wordmark() {
  return (
    <div className="flex flex-col items-center gap-4">
      <div className="flex h-10 items-end gap-1.5">
        {BAR_COLORS.map((c, i) => (
          <motion.div
            key={c}
            className="w-2.5 rounded-full"
            style={{ backgroundColor: c }}
            initial={{ height: 8, opacity: 0 }}
            animate={{ height: BAR_HEIGHTS[i], opacity: 1 }}
            transition={{
              type: "spring",
              stiffness: 350,
              damping: 28,
              delay: 0.08 * i,
            }}
          />
        ))}
      </div>
      <div className="space-y-1 text-center">
        <h1 className="text-lg font-semibold tracking-tight">
          Welcome to ReaMesh
        </h1>
        <p className="text-[13px] text-muted-foreground">
          Real-time collaboration for REAPER
        </p>
      </div>
    </div>
  );
}

function LocateStep({ status }: { status: ReaperStatus }) {
  const pick = async () => {
    const dir = await open({
      directory: true,
      title: "Locate your REAPER folder",
    });
    if (typeof dir === "string") {
      setReaperPath(dir);
      await refreshStatus();
    }
  };

  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-amber-500/15">
          <FolderSearch className="size-4.5 text-amber-500" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">REAPER not found</h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            There's no REAPER folder at{" "}
            <span className="break-all text-foreground/80">
              {status.resourcePath}
            </span>
            . REAPER may not be installed, or it keeps its data somewhere
            else (portable install).
          </p>
        </div>
      </div>
      <div className="flex gap-2">
        <Button size="sm" className="h-8 flex-1 text-[12px]" onClick={pick}>
          Locate REAPER folder…
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-8 flex-1 text-[12px]"
          onClick={() => refreshStatus()}
        >
          Check again
        </Button>
      </div>
    </div>
  );
}

function InstallStep({
  status,
  onInstalled,
}: {
  status: ReaperStatus;
  onInstalled: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const install = async () => {
    setBusy(true);
    try {
      await installExtension();
      onInstalled();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-secondary">
          <Puzzle className="size-4.5 text-foreground/80" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">
            Install the REAPER extension
          </h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            ReaMesh adds a small extension to REAPER so this app can talk to
            it. It goes into{" "}
            <span className="break-all text-foreground/80">
              {status.userPlugins}
            </span>
            .
          </p>
        </div>
      </div>
      {!status.isReaperDir && (
        <p className="flex items-start gap-2 rounded-lg bg-amber-500/10 p-2.5 text-[12px] leading-relaxed text-amber-500">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          This folder doesn't look like a REAPER data folder yet (no
          reaper.ini). Installing is harmless, but double-check the location
          in Settings if REAPER doesn't pick it up.
        </p>
      )}
      <Button
        size="sm"
        className="h-8 w-full text-[12px]"
        disabled={busy}
        onClick={install}
      >
        {busy ? "Installing…" : "Install extension"}
      </Button>
    </div>
  );
}

function DoneStep() {
  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-emerald-500/15">
          <CheckCircle2 className="size-4.5 text-emerald-500" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">You're all set</h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            The extension is installed. Start REAPER (or restart it if it's
            already running) and ReaMesh will connect automatically.
          </p>
        </div>
      </div>
      <Button
        size="sm"
        className="h-8 w-full text-[12px]"
        onClick={() => refreshStatus()}
      >
        Finish
      </Button>
    </div>
  );
}

export function Onboarding({ status }: { status: ReaperStatus }) {
  // Local "installed just now" flag so the success step stays visible until
  // Finish refreshes the status and the App-level gate moves on.
  const [done, setDone] = useState(false);
  const step = done ? "done" : status.reaperFound ? "install" : "locate";

  return (
    <div className="flex flex-1 flex-col justify-center gap-8 px-6 pb-10">
      <Wordmark />
      <AnimatePresence mode="wait">
        <motion.div
          key={step}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          transition={{ duration: 0.18, ease: "easeOut" }}
        >
          {step === "done" ? (
            <DoneStep />
          ) : step === "install" ? (
            <InstallStep status={status} onInstalled={() => setDone(true)} />
          ) : (
            <LocateStep status={status} />
          )}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}
