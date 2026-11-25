import { useEffect, useRef, useState } from "react";
import {
  AnimatePresence,
  animate,
  motion,
  useAnimate,
  useMotionValue,
  useMotionValueEvent,
  useReducedMotion,
  useSpring,
} from "motion/react";
import { Minus, X } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Group, Divider } from "@/components/Group";
import { RMark, RMarkFill } from "@/components/RMark";
import { Hint } from "@/components/Hint";
import { t, useLang, type MsgKey } from "@/lib/i18n";
import { formatBytes } from "@/lib/format";
import { cn, focusRing } from "@/lib/utils";
import {
  chooseDir,
  launchApp,
  onDone,
  onProgress,
  overall,
  quit,
  setupInfo,
  showWindow,
  startInstall,
  startUninstall,
  type Done,
  type Info,
  type Phase,
} from "./api";

// ReaMesh Setup: install, update or remove, in one small window drawn like the
// app. The R is the progress: it fills with ink as the files land, and drains
// again as an uninstall takes them away.

type Stage = "ask" | "work" | "done" | "error";

const PHASE: Record<Phase, MsgKey> = {
  verify: "setupPhaseVerify",
  close: "setupPhaseClose",
  copy: "setupPhaseCopy",
  register: "setupPhaseRegister",
  remove: "setupPhaseRemove",
  done: "setupPhaseRegister",
};

// The ink follows the real progress on a soft spring, so bursts of progress
// read as a steady pour.
const POUR = { type: "spring", stiffness: 110, damping: 24, restDelta: 0.0005 } as const;
const INTRO_EASE = [0.25, 0.1, 0.75, 0.9] as const;

// Stage content trades places by rising out of a slight blur, like the app's
// toasts.
const swap = {
  initial: { opacity: 0, y: 10, filter: "blur(6px)" },
  animate: { opacity: 1, y: 0, filter: "blur(0px)", transition: { duration: 0.34, ease: [0.16, 1, 0.3, 1] } },
  exit: { opacity: 0, y: -6, filter: "blur(4px)", transition: { duration: 0.16, ease: "easeIn" } },
} as const;

export function Setup() {
  useLang();
  const reduce = useReducedMotion();
  const [info, setInfo] = useState<Info | null>(null);
  const [stage, setStage] = useState<Stage>("ask");
  const [phase, setPhase] = useState<Phase>("verify");
  const [result, setResult] = useState<Done | null>(null);
  const [desktop, setDesktop] = useState(true);
  const [removeExt, setRemoveExt] = useState(true);
  const [removeData, setRemoveData] = useState(false);
  const [settle, setSettle] = useState(0);
  const [hero, animateHero] = useAnimate<HTMLDivElement>();
  const ready = info != null;

  // The R rises in with the first state, and gives a small settle when a job
  // has finished.
  useEffect(() => {
    if (!ready) return;
    if (reduce) animateHero(hero.current, { opacity: 1 }, { duration: 0 });
    else animateHero(hero.current, { opacity: [0, 1], y: [6, 0] }, { duration: 0.4, ease: "easeOut" });
  }, [ready, reduce, animateHero, hero]);
  useEffect(() => {
    if (!settle || reduce) return;
    animateHero(hero.current, { scale: [1, 1.045, 1] }, { duration: 0.55, ease: [0.16, 1, 0.3, 1], delay: 0.25 });
  }, [settle, reduce, animateHero, hero]);

  const ink = useMotionValue(0);
  const pct = useMotionValue(0);
  const shownPct = useSpring(pct, { stiffness: 110, damping: 24 });
  const [pctText, setPctText] = useState("0");
  useMotionValueEvent(shownPct, "change", (v) => setPctText(String(Math.round(v * 100))));

  const target = useRef(0);
  const following = useRef(false);
  const started = useRef(false);
  const mode = info?.mode ?? "install";

  const pour = (v: number) => {
    if (reduce) ink.set(v);
    else animate(ink, v, POUR);
  };

  // First look: read the state, let the R ink in, then show the window once
  // the page has painted (no white flash on open).
  useEffect(() => {
    setupInfo()
      .then((i) => {
        setInfo(i);
        if (i.mode === "uninstall") ink.set(1);
        else if (!i.passive) {
          if (reduce) ink.set(1);
          else animate(ink, 1, { duration: 2, delay: 0.25, ease: INTRO_EASE });
        }
        requestAnimationFrame(() => requestAnimationFrame(() => showWindow()));
      })
      .catch(() => showWindow());
  }, [ink, reduce]);

  // Whether ReaMesh is open, kept current while the user decides.
  useEffect(() => {
    if (stage !== "ask" || !info || info.passive) return;
    const id = setInterval(() => setupInfo().then(setInfo).catch(() => {}), 2000);
    return () => clearInterval(id);
  }, [stage, info]);

  useEffect(() => {
    const unProgress = onProgress((p, v) => {
      setPhase(p);
      const all = overall(mode, p, v);
      pct.set(all);
      target.current = mode === "install" ? all : 1 - all;
      if (following.current) pour(target.current);
    });
    const unDone = onDone((d) => {
      setResult(d);
      started.current = false;
      if (d.ok) {
        pct.set(1);
        target.current = mode === "install" ? 1 : 0;
        following.current = true;
        pour(target.current);
        setStage("done");
        setSettle((n) => n + 1);
      } else {
        following.current = false;
        setStage("error");
      }
    });
    return () => {
      unProgress.then((f) => f());
      unDone.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  const start = async () => {
    if (!info || started.current) return;
    started.current = true;
    setResult(null);
    setPhase(info.mode === "install" ? "verify" : "close");
    pct.jump(0);
    setStage("work");
    following.current = false;
    if (info.mode === "install") {
      // The intro's full R empties first, then fills with the real progress.
      target.current = 0;
      const drain = reduce ? (ink.set(0), Promise.resolve()) : animate(ink, 0, { duration: 0.42, ease: [0.4, 0, 0.2, 1] });
      Promise.resolve(drain).then(() => {
        following.current = true;
        pour(target.current);
      });
    } else {
      target.current = 1;
      following.current = true;
    }
    try {
      if (info.mode === "install") await startInstall(desktop && info.relation === "fresh");
      else await startUninstall(info.extensionInstalled && removeExt, info.dataExists && removeData);
    } catch (e) {
      started.current = false;
      setResult({ ok: false, error: String(e), restartReaper: false });
      setStage("error");
    }
  };

  // Passive (`/P`): no questions, progress only, gone when done.
  useEffect(() => {
    if (info?.passive && stage === "ask") start();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [info?.passive]);
  useEffect(() => {
    if (!info?.passive || stage !== "done") return;
    const id = setTimeout(() => (info.mode === "install" ? launchApp() : quit()), 900);
    return () => clearTimeout(id);
  }, [info, stage]);

  const retry = () => {
    setStage("ask");
    setupInfo().then(setInfo).catch(() => {});
    if (mode === "install") pour(1);
  };

  const pick = async () => {
    if (!info) return;
    const dir = await open({ directory: true, title: t("setupChooseFolder"), defaultPath: info.dir }).catch(() => null);
    if (typeof dir !== "string") return;
    try {
      await chooseDir(dir);
      setInfo(await setupInfo());
    } catch {
      // A folder that can't be used just leaves the old one in place.
    }
  };

  return (
    <div className="flex h-full flex-col">
      <TitleBar busy={stage === "work"} />
      <main className="flex min-h-0 flex-1 flex-col px-6 pb-6">
        <div className="flex flex-1 items-center justify-center">
          <div ref={hero} className="relative opacity-0">
            <RMark height={88} className="absolute inset-0 text-foreground/[0.08]" />
            <RMarkFill height={88} progress={ink} className="relative" />
          </div>
        </div>

        <div className="flex h-[232px] shrink-0 flex-col justify-end">
          <AnimatePresence mode="wait" initial={false}>
            {info && (
              <motion.section key={stage} {...swap} className="flex flex-col">
                {stage === "ask" &&
                  (info.mode === "install" ? (
                    <InstallAsk info={info} desktop={desktop} setDesktop={setDesktop} onPick={pick} onStart={start} />
                  ) : (
                    <UninstallAsk
                      info={info}
                      removeExt={removeExt}
                      setRemoveExt={setRemoveExt}
                      removeData={removeData}
                      setRemoveData={setRemoveData}
                      onStart={start}
                    />
                  ))}
                {stage === "work" && <Work phase={phase} pct={pctText} />}
                {stage === "done" && <Finished info={info} result={result} />}
                {stage === "error" && <Failed info={info} result={result} onRetry={retry} />}
              </motion.section>
            )}
          </AnimatePresence>
        </div>
      </main>
    </div>
  );
}

function Heading({ title, sub }: { title: string; sub?: React.ReactNode }) {
  return (
    <div className="space-y-1 text-center">
      <h1 className="text-[21px] leading-tight font-semibold tracking-[-0.015em]">{title}</h1>
      {sub != null && <p className="text-[13px] text-muted-foreground tabular-nums">{sub}</p>}
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-h-11 items-center gap-3 px-3.5 py-2">
      <span className="shrink-0 text-[13px]">{label}</span>
      <div className="flex min-w-0 flex-1 items-center justify-end gap-2">{children}</div>
    </div>
  );
}

function InstallAsk({
  info,
  desktop,
  setDesktop,
  onPick,
  onStart,
}: {
  info: Info;
  desktop: boolean;
  setDesktop: (v: boolean) => void;
  onPick: () => void;
  onStart: () => void;
}) {
  const fresh = info.relation === "fresh";
  const title: MsgKey = {
    fresh: "setupInstallTitle",
    update: "setupUpdateTitle",
    same: "setupReinstallTitle",
    downgrade: "setupDowngradeTitle",
  }[info.relation] as MsgKey;
  const cta: MsgKey = fresh ? "setupInstall" : info.relation === "update" ? "setupUpdate" : "setupReinstall";
  const sub =
    info.relation === "update" || info.relation === "downgrade"
      ? t("setupFromTo", { from: info.installedVersion ?? "?", to: info.version ?? "" })
      : t("setupVersion", { version: info.version ?? "" });
  const need = info.needBytes + 64 * 1024 * 1024;
  const noSpace = info.freeBytes != null && info.freeBytes < need;

  return (
    <>
      <Heading title={t(title)} sub={sub} />
      <Group className="mt-5">
        <Row label={t("setupFolder")}>
          <span className="truncate text-[12px] text-muted-foreground" title={info.dir}>
            {info.dir}
          </span>
          {fresh && (
            <Button variant="ghost" size="sm" className="h-7 shrink-0 px-2.5 text-[12px]" onClick={onPick}>
              {t("change")}
            </Button>
          )}
        </Row>
        {fresh && (
          <>
            <Divider inset={false} />
            <Row label={t("setupDesktopShortcut")}>
              <Switch checked={desktop} onChange={setDesktop} label={t("setupDesktopShortcut")} />
            </Row>
          </>
        )}
      </Group>
      <Note show={noSpace} tone="error">
        {t("setupNoSpace", { size: formatBytes(need) })}
      </Note>
      <Note show={!noSpace && info.appRunning} tone="warn">
        {t("setupAppRunning")}
      </Note>
      <Button className="mt-4 h-9 w-full text-[13px]" disabled={noSpace} onClick={onStart}>
        {t(cta)}
      </Button>
    </>
  );
}

function UninstallAsk({
  info,
  removeExt,
  setRemoveExt,
  removeData,
  setRemoveData,
  onStart,
}: {
  info: Info;
  removeExt: boolean;
  setRemoveExt: (v: boolean) => void;
  removeData: boolean;
  setRemoveData: (v: boolean) => void;
  onStart: () => void;
}) {
  const rows = [
    info.extensionInstalled && (
      <Row key="ext" label={t("setupRemoveExtension")}>
        <Switch checked={removeExt} onChange={setRemoveExt} label={t("setupRemoveExtension")} />
      </Row>
    ),
    info.dataExists && (
      <Row key="data" label={t("setupRemoveData")}>
        <Switch checked={removeData} onChange={setRemoveData} label={t("setupRemoveData")} />
      </Row>
    ),
  ].filter(Boolean);
  return (
    <>
      <Heading
        title={t("setupUninstallTitle")}
        sub={info.installedVersion ? t("setupVersion", { version: info.installedVersion }) : undefined}
      />
      {rows.length > 0 && (
        <Group className="mt-5">
          {rows.map((r, i) => (
            <div key={i}>
              {i > 0 && <Divider inset={false} />}
              {r}
            </div>
          ))}
        </Group>
      )}
      <Note show={info.appRunning} tone="warn">
        {t("setupAppRunning")}
      </Note>
      <div className="mt-4 flex gap-2">
        <Button variant="secondary" className="h-9 flex-1 text-[13px]" onClick={() => quit()}>
          {t("cancel")}
        </Button>
        <Button variant="destructive" className="h-9 flex-1 text-[13px]" onClick={onStart}>
          {t("setupUninstall")}
        </Button>
      </div>
    </>
  );
}

function Work({ phase, pct }: { phase: Phase; pct: string }) {
  return (
    <div className="flex flex-col items-center pb-12">
      <h1 className="grid text-[17px] font-semibold">
        <AnimatePresence initial={false}>
          <motion.span
            key={phase}
            className="col-start-1 row-start-1 text-center"
            initial={{ opacity: 0, y: 5 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -5 }}
            transition={{ duration: 0.22, ease: "easeOut" }}
          >
            {t(PHASE[phase])}
          </motion.span>
        </AnimatePresence>
      </h1>
      <p className="mt-1 text-[13px] text-muted-foreground tabular-nums" aria-live="polite">
        {pct} %
      </p>
    </div>
  );
}

function Finished({ info, result }: { info: Info; result: Done | null }) {
  if (info.mode === "uninstall") {
    return (
      <>
        <Heading
          title={t("setupRemoved")}
          sub={result?.restartReaper ? t("setupRestartReaper") : undefined}
        />
        {!info.passive && (
          <Button className="mt-6 h-9 w-full text-[13px]" onClick={() => quit()}>
            {t("close")}
          </Button>
        )}
      </>
    );
  }
  const updated = info.relation !== "fresh";
  return (
    <>
      <Heading
        title={t(updated ? "setupUpdated" : "setupInstalled")}
        sub={t("setupVersion", { version: info.version ?? "" })}
      />
      {!info.passive && (
        <div className="mt-6 flex gap-2">
          <Button variant="secondary" className="h-9 flex-1 text-[13px]" onClick={() => quit()}>
            {t("close")}
          </Button>
          <Button className="h-9 flex-1 text-[13px]" onClick={() => launchApp()}>
            {t("setupOpen")}
          </Button>
        </div>
      )}
    </>
  );
}

function Failed({ info, result, onRetry }: { info: Info; result: Done | null; onRetry: () => void }) {
  return (
    <>
      <Heading title={t(info.mode === "install" ? "setupFailed" : "setupUninstallFailed")} />
      {result?.error && (
        <p className="mt-3 line-clamp-3 text-center text-[12px] leading-normal break-words text-muted-foreground select-text">
          {result.error}
        </p>
      )}
      <div className="mt-5 flex gap-2">
        <Button variant="secondary" className="h-9 flex-1 text-[13px]" onClick={() => quit()}>
          {t("close")}
        </Button>
        <Button className="h-9 flex-1 text-[13px]" onClick={onRetry}>
          {t("setupTryAgain")}
        </Button>
      </div>
    </>
  );
}

/** A one-line note under the card that unfolds on the app's spring. */
function Note({ show, tone, children }: { show: boolean; tone: "warn" | "error"; children: React.ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.p
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={{ type: "spring", stiffness: 400, damping: 38 }}
          className={cn(
            "overflow-hidden text-center text-[12px]",
            tone === "warn" ? "text-amber-500" : "text-destructive",
          )}
        >
          <span className="block pt-3">{children}</span>
        </motion.p>
      )}
    </AnimatePresence>
  );
}

// The window has no frame of its own: a draggable strip with minimize and
// close, like the app's title bar on Windows. Close waits while files move.
function TitleBar({ busy }: { busy: boolean }) {
  const call = (fn: (w: ReturnType<typeof getCurrentWindow>) => Promise<void>) => () => {
    try {
      fn(getCurrentWindow()).catch(() => {});
    } catch {
      // Not running inside Tauri (browser preview).
    }
  };
  const base = cn(
    "flex h-full w-[46px] items-center justify-center text-foreground/80 transition-colors disabled:opacity-30 disabled:hover:bg-transparent",
    focusRing,
  );
  return (
    <header data-tauri-drag-region className="flex h-11 shrink-0 select-none items-stretch justify-end">
      <Hint label={t("minimizeWindow")}>
        <button
          type="button"
          onClick={call((w) => w.minimize())}
          aria-label={t("minimizeWindow")}
          className={cn(base, "hover:bg-secondary hover:text-foreground")}
        >
          <Minus className="size-4" aria-hidden="true" />
        </button>
      </Hint>
      <Hint label={t("closeWindow")}>
        <button
          type="button"
          disabled={busy}
          onClick={() => quit()}
          aria-label={t("closeWindow")}
          className={cn(base, "hover:bg-[#c42b1c] hover:text-white")}
        >
          <X className="size-4" aria-hidden="true" />
        </button>
      </Hint>
    </header>
  );
}
