import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { RefreshCw, TriangleAlert } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { TitleBar } from "@/components/TitleBar";
import { Disconnected } from "@/screens/Disconnected";
import { Lobby } from "@/screens/Lobby";
import { Downloading } from "@/screens/Downloading";
import { Onboarding } from "@/screens/Onboarding";
import { Session } from "@/screens/Session";
import { Settings } from "@/screens/Settings";
import { startScanning, send, useActiveInstance } from "@/lib/connection";
import { openOverlay, closeOverlay, setSessionState } from "@/lib/overlay";
import { loadReaperPath, refreshStatus, useReaperStatus } from "@/lib/install";
import { t, useLang } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { localizeLogEntry } from "@/lib/log";
import { pluginStatus, useInstalledPlugins } from "@/lib/plugins";
import { displayName, type ExtState, type LogEntry, type Mode } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

startScanning();
loadReaperPath().then(refreshStatus);

/** Show join and leave toasts during a session.
 *  Ignore mode changes so existing peers are not announced as new. If both
 *  this window and REAPER are out of view, badge the app and return unseen
 *  peer ids so Session can highlight them when the user returns. */
function usePresenceToasts(state: ExtState | undefined, port: number | undefined): string[] {
  const prev = useRef<
    { port: number | undefined; mode: Mode; ids: Map<string, { name: string; color: string }> } | undefined
  >(undefined);
  const unseen = useRef<Set<string>>(new Set());
  const [flash, setFlash] = useState<string[]>([]);

  useEffect(() => {
    // The badge outlives `state` going undefined (REAPER quitting closes the
    // socket), so clear it before that early return rather than after it.
    const inSession = state?.mode === "hosting" || state?.mode === "client";
    if (!inSession && unseen.current.size) {
      unseen.current.clear();
      invoke("clear_join_badge").catch(() => {});
    }
    if (!state) {
      prev.current = undefined;
      return;
    }
    const ids = new Map(
      state.peers
        .filter((p) => !p.is_self)
        .map((p) => [p.id, { name: displayName(p), color: p.color }]),
    );
    const before = prev.current;
    prev.current = { port, mode: state.mode, ids };
    // A mode or instance switch replaces the entire roster. Comparing the old
    // and new lists would report every peer as joining or leaving.
    if (!before || before.port !== port || before.mode !== state.mode || !inSession) return;

    const dot = (color: string) => (
      <span
        className="inline-block size-2 shrink-0 rounded-full"
        style={{ backgroundColor: color }}
      />
    );
    for (const [id, info] of ids) {
      if (!before.ids.has(id)) {
        toast(
          <span className="flex items-center gap-2">
            {dot(info.color)}
            <span>
              <b>{info.name}</b> {t("joinedSessionSuffix")}
            </span>
          </span>,
        );
        if (!document.hasFocus()) {
          // Highlight the peer only if the backend actually badged the app.
          invoke<boolean>("notify_join")
            .then((badged) => {
              if (badged) unseen.current.add(id);
            })
            .catch(() => {});
        }
      }
    }
    for (const [id, info] of before.ids) {
      if (!ids.has(id)) {
        toast(
          <span className="flex items-center gap-2">
            {dot(info.color)}
            <span>
              <b>{info.name}</b> {t("leftSessionSuffix")}
            </span>
          </span>,
        );
      }
    }
  }, [state, port]);

  // The user is back: drop the badge and hand the missed joins to the Session
  // screen for their one-shot row flash.
  useEffect(() => {
    const onFocus = () => {
      invoke("clear_join_badge").catch(() => {});
      if (unseen.current.size) {
        setFlash([...unseen.current]);
        unseen.current.clear();
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
  useEffect(() => {
    if (!flash.length) return;
    const id = setTimeout(() => setFlash([]), 2400);
    return () => clearTimeout(id);
  }, [flash]);

  return flash;
}

/** Deduplicate control-server errors per REAPER instance. The server replays
 *  its latest state after reconnects, so clearing the key while disconnected
 *  would show dismissed errors again. */
function useErrorToasts(state: ExtState | undefined, port: number | undefined) {
  const last = useRef<Map<number, string>>(new Map());
  useEffect(() => {
    if (port == null) return;
    const err = state?.error ?? null;
    // A missing state frame does not mean the error was cleared.
    if (!state) return;
    const key = err ? JSON.stringify(err) : "";
    if (err && key !== last.current.get(port)) toast.error(localizeLogEntry(err));
    last.current.set(port, key);
  }, [state, state?.error, port]);
}

/** Explain when a remote editing lock reverts a local change. Debounce the
 *  notice so quick back-and-forth edits do not stack toasts. */
function useOverrideToasts(state: ExtState | undefined, port: number | undefined) {
  const prev = useRef<{
    port?: number;
    mode?: Mode;
    count: number;
    lastAt: number;
    enteredAt: number;
  }>({
    count: 0,
    lastAt: 0,
    enteredAt: 0,
  });
  useEffect(() => {
    const count = state?.overridden ?? 0;
    // The counter belongs to one REAPER instance. Comparing across instances
    // could report an old conflict as a new one.
    const sameSession = prev.current.mode === state?.mode && prev.current.port === port;
    const inSession = state?.mode === "client" || state?.mode === "hosting";
    const now = Date.now();
    if (inSession && !sameSession) prev.current.enteredAt = now; // just joined/started
    if (
      inSession &&
      sameSession &&
      count > prev.current.count &&
      now - prev.current.lastAt > 4000 &&
      // Suppress conflict notices during the initial synchronization grace period.
      now - prev.current.enteredAt > 3000
    ) {
      // The most recent lock event carries the track and who's editing it; fall
      // back to the generic notice when the extension couldn't resolve them.
      const last = [...(state?.log ?? [])].reverse().find((e) => e.code === "edit_locked");
      toast(last ? localizeLogEntry(last) : t("editLocked"));
      prev.current.lastAt = now;
    }
    prev.current.mode = state?.mode;
    prev.current.port = port;
    prev.current.count = count;
  }, [state?.overridden, state?.mode, state?.log, port]);
}

/** Notify when a plugin is added during a session. Mention if it is missing
 *  locally. Find new entries from the previous final item because the log is a
 *  capped deque without ids. */
function usePluginToasts(state: ExtState | undefined, port: number | undefined) {
  const installed = useInstalledPlugins();
  const prev = useRef<{ port?: number; lastKey: string; len: number }>({
    lastKey: "",
    len: 0,
  });
  useEffect(() => {
    if (!state) return;
    const inSession = state.mode === "hosting" || state.mode === "client";
    const log = state.log ?? [];
    const samePort = prev.current.port === port;
    const newEntries: LogEntry[] = [];
    if (samePort && inSession && prev.current.lastKey) {
      let idx = -1;
      for (let i = log.length - 1; i >= 0; i--) {
        if (JSON.stringify(log[i]) === prev.current.lastKey) {
          idx = i;
          break;
        }
      }
      if (idx >= 0) newEntries.push(...log.slice(idx + 1));
      else if (log.length > prev.current.len) newEntries.push(...log.slice(prev.current.len));
    }
    prev.current = {
      port,
      lastKey: log.length ? JSON.stringify(log[log.length - 1]) : "",
      len: log.length,
    };
    for (const e of newEntries) {
      if (e.code !== "plugin_added" && e.code !== "plugins_added") continue;
      const plugin = e.code === "plugin_added" ? String(e.params?.plugin ?? "") : "";
      const missing = plugin ? pluginStatus(plugin, installed) === "missing" : false;
      const text = localizeLogEntry(e);
      toast(
        missing ? (
          <span>
            {text}
            <span className="block text-amber-500">{t("pluginNotInstalledHere")}</span>
          </span>
        ) : (
          text
        ),
      );
    }
  }, [state, port, installed]);
}

export default function App() {
  const instance = useActiveInstance();
  const state = instance?.state;
  const reaper = useReaperStatus();
  const [settingsOpen, setSettingsOpen] = useState(false);
  useLang(); // re-render the whole tree when the language changes
  // Every cross-render hook below is keyed to the instance as well as the mode:
  // the active instance swaps wholesale on a tab click or when a REAPER quits,
  // and none of this state carries across.
  const activePort = instance?.port;
  const flashPeers = usePresenceToasts(state, activePort);
  useErrorToasts(state, activePort);
  useOverrideToasts(state, activePort);
  usePluginToasts(state, activePort);

  // Remember the last connected mode so a closed REAPER gets a clear message
  // instead of looking like a first launch.
  const lastLive = useRef<{ mode: Mode } | null>(null);
  useEffect(() => {
    if (state) lastLive.current = { mode: state.mode };
  }, [state]);
  const reaperClosed = !state && lastLive.current != null;
  const closedWasSession =
    reaperClosed &&
    (lastLive.current?.mode === "hosting" || lastLive.current?.mode === "client");

  // Hold the Disconnected screen briefly after REAPER answers so its
  // success animation finishes before the lobby appears. `waiting` matches
  // the render condition below.
  const hasState = !!state;
  const waiting =
    !settingsOpen && reaper !== undefined && !state && !(reaper && !reaper.installed);
  const [foundBeat, setFoundBeat] = useState(false);
  // Detect the waiting-to-connected transition during render. This avoids a
  // one-frame trip through the lobby that would replay the entire entrance.
  const [prevWaiting, setPrevWaiting] = useState(waiting);
  if (prevWaiting !== waiting) {
    setPrevWaiting(waiting);
    if (prevWaiting && hasState) setFoundBeat(true);
  }
  useEffect(() => {
    // Clear the beat after the handoff, or immediately if the connection drops.
    if (!foundBeat) return;
    if (!hasState) {
      setFoundBeat(false);
      return;
    }
    const id = setTimeout(() => setFoundBeat(false), 850);
    return () => clearTimeout(id);
  }, [foundBeat, hasState]);

  // Keep the completed download screen visible briefly before showing Session.
  // Detect this during render to keep the same screen mounted, and key it by
  // port so switching to another client instance cannot trigger the beat.
  const [doneBeat, setDoneBeat] = useState<number | null>(null);
  const [prevInst, setPrevInst] = useState({ port: instance?.port, mode: state?.mode });
  if (prevInst.port !== instance?.port || prevInst.mode !== state?.mode) {
    if (
      prevInst.port === instance?.port &&
      prevInst.mode === "downloading" &&
      state?.mode === "client" &&
      instance?.port != null
    )
      setDoneBeat(instance.port);
    setPrevInst({ port: instance?.port, mode: state?.mode });
  }
  const beatActive = doneBeat != null && doneBeat === instance?.port;
  useEffect(() => {
    if (doneBeat == null) return;
    // Drop the beat if the session ends or the active instance changes.
    if (doneBeat !== instance?.port || state?.mode !== "client") {
      setDoneBeat(null);
      return;
    }
    const id = setTimeout(() => setDoneBeat(null), 1100);
    return () => clearTimeout(id);
  }, [doneBeat, instance?.port, state?.mode]);

  // Settings is opened from the native menu (ReaMesh → Settings…, Cmd+,). On
  // macOS the backend opens a native AppKit window instead and emits
  // `menu:settings-changed` after edits so we can re-read status; the
  // `menu:settings` event only fires on platforms using the in-webview screen.
  useEffect(() => {
    const subs = [
      listen("menu:settings", () => {
        refreshStatus();
        setSettingsOpen(true);
      }),
      listen("menu:settings-changed", () => {
        loadReaperPath().then(refreshStatus);
      }),
    ];
    return () => {
      subs.forEach((un) => un.then((f) => f()));
    };
  }, []);

  // Windows has no application menu, so Ctrl+, opens Settings from the keyboard
  // (the macOS menu item already carries Cmd+,).
  useEffect(() => {
    if (!isWindows) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key === ",") {
        e.preventDefault();
        setSettingsOpen(true);
        refreshStatus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // The lobby needs discovery running; the extension's browse is idempotent.
  // Addressed to the instance this render is for, never to whatever
  // `activePort` happens to be when the effect runs.
  const mode = state?.mode;
  const port = activePort;
  useEffect(() => {
    if (mode === "idle" && port != null) send({ cmd: "browse" }, port);
  }, [mode, port]);

  // Give the overlay the active instance's port and process. A port change must
  // rebuild the HUD even when both instances are already in sessions.
  const inSession = mode === "hosting" || mode === "client";
  const pid = instance?.pid;
  useEffect(() => {
    if (inSession && port != null) openOverlay(port, pid ?? null);
    else closeOverlay();
  }, [inSession, port, pid]);

  // Auto-install updates the file, but a running REAPER keeps using the copy it
  // loaded at startup. Prompt for a restart just like the manual install flow.
  const [installedThisRun, setInstalledThisRun] = useState(false);
  const [restartDismissed, setRestartDismissed] = useState(false);
  useEffect(() => {
    const un = listen("extension-installed", () => {
      setInstalledThisRun(true);
      setRestartDismissed(false);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  const needsRestart = installedThisRun && hasState && !restartDismissed;

  // A fresh state frame proves the main thread responded, even before the
  // watchdog clears `busy` on its next tick.
  const stateSeq = state ? JSON.stringify(state.progress) + state.mode + state.state : "";
  const [busySince, setBusySince] = useState("");
  useEffect(() => {
    if (instance?.busy) setBusySince(stateSeq);
  }, [instance?.busy]); // eslint-disable-line react-hooks/exhaustive-deps
  const busy = !!instance?.busy && !state?.opening && stateSeq === busySince;

  // Give the quit handler a port only during a session so it can leave cleanly.
  useEffect(() => {
    setSessionState(mode === "hosting", inSession ? (port ?? null) : null);
  }, [mode, inSession, port]);

  // Run onboarding until the bundled extension is installed. Skip the gate
  // while the check is pending and in plain-browser development.
  let screen: { key: string; node: React.ReactNode };
  if (settingsOpen) {
    screen = {
      key: "settings",
      node: <Settings onBack={() => setSettingsOpen(false)} />,
    };
  } else if (reaper === undefined) {
    screen = { key: "checking", node: null };
  } else if (foundBeat) {
    screen = { key: "disconnected", node: <Disconnected found /> };
  } else if (reaper && !reaper.installed && !state) {
    screen = { key: "onboarding", node: <Onboarding status={reaper} /> };
  } else if (!state) {
    screen = {
      key: "disconnected",
      node: <Disconnected closed={reaperClosed} closedWasSession={closedWasSession} />,
    };
  } else if (mode === "downloading") {
    screen = { key: "downloading", node: <Downloading state={state} port={port} /> };
  } else if (beatActive && mode === "client") {
    // Keep the screen mounted while it switches to the completed state.
    screen = { key: "downloading", node: <Downloading state={state} port={port} done /> };
  } else if (mode === "hosting" || mode === "client") {
    screen = {
      key: "session",
      node: <Session state={state} port={port ?? null} flashPeers={flashPeers} />,
    };
  } else {
    screen = { key: "lobby", node: <Lobby state={state} port={port ?? null} /> };
  }

  return (
    <div className="relative flex h-full flex-col">
      <TitleBar
        onOpenSettings={() => {
          // Open first, refresh after: the screen must appear even if the
          // status probe throws (no backend in the plain-browser dev harness).
          setSettingsOpen(true);
          refreshStatus();
        }}
      />
      {/* REAPER may be waiting on a modal dialog. Hide this during the expected
          pause while a completed download opens. */}
      <AnimatePresence>
        {needsRestart && (
          <motion.div
            key="restart"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            className="overflow-hidden"
          >
            <div
              role="status"
              className="mx-4 mt-2 flex items-center gap-2.5 rounded-xl bg-secondary px-3 py-2.5"
            >
              <RefreshCw className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              <span className="flex-1 text-[12px] font-medium">{t("extUpdatedRestart")}</span>
              <button
                type="button"
                onClick={() => setRestartDismissed(true)}
                className={cn(
                  "shrink-0 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground transition-colors hover:text-foreground",
                  focusRing,
                )}
              >
                {t("firewallGotIt")}
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <AnimatePresence>
        {busy && (
          <motion.div
            key="busy"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            className="overflow-hidden"
          >
            <div
              role="status"
              className="mx-4 mt-2 flex items-center gap-2.5 rounded-xl bg-amber-500/10 px-3 py-2.5 text-amber-500"
            >
              <TriangleAlert className="size-4 shrink-0" aria-hidden="true" />
              <span className="text-[12px] font-medium">{t("reaperBusyBanner")}</span>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <AnimatePresence mode="wait">
        <motion.main
          key={screen.key + (port ?? "")}
          className="flex min-h-0 flex-1 flex-col"
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          transition={{ duration: 0.18, ease: "easeOut" }}
        >
          {screen.node}
        </motion.main>
      </AnimatePresence>
      <Toaster />
    </div>
  );
}
