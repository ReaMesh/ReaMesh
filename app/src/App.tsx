import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";
import { PeerAvatar } from "@/components/PeerAvatar";
import { TitleBar } from "@/components/TitleBar";
import { Disconnected } from "@/screens/Disconnected";
import { Lobby } from "@/screens/Lobby";
import { Downloading } from "@/screens/Downloading";
import { Onboarding } from "@/screens/Onboarding";
import { Outdated } from "@/screens/Outdated";
import { Session } from "@/screens/Session";
import { Settings } from "@/screens/Settings";
import { Credits } from "@/screens/Credits";
import { ExtMismatch } from "@/screens/ExtMismatch";
import { startScanning, send, useActiveInstance } from "@/lib/connection";
import { offerInvite } from "@/lib/invite";
import { openOverlay, closeOverlay, setSessionState } from "@/lib/overlay";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { buildDiagnostics } from "@/lib/diagnostics";
import { loadReaperPath, refreshStatus, useExtensionStale, useReaperStatus } from "@/lib/install";
import { t, useLang } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { localizeLogEntry } from "@/lib/log";
import { pluginStatus, useInstalledPlugins } from "@/lib/plugins";
import { isInviteLink } from "@/lib/ticket";
import { displayName, type ExtState, type LogEntry, type Mode } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";
import { listenForRestart, revealWindow, takeResume, trackScreen } from "@/lib/resume";
import { useAppVersion } from "@/lib/updater";
import { protoSkew } from "@/lib/version";

// How long a fresh launch waits for REAPER to answer before showing the
// waiting screen (see `holdBlank`).
const LAUNCH_GRACE_MS = 1200;

startScanning();
loadReaperPath().then(refreshStatus);
listenForRestart();
/** Set when this launch is the restart after an update: put the screen back
 *  and show the window only once REAPER's state is in. */
const resumed = takeResume();

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

    // Their face, as in the roster and the lock notice.
    const face = (id: string, info: { name: string; color: string }) => (
      <PeerAvatar
        peer={{ id, name: info.name, color: info.color, is_self: false, is_owner: false }}
        size="xs"
      />
    );
    for (const [id, info] of ids) {
      if (!before.ids.has(id)) {
        toast(
          <span className="flex items-center gap-2.5">
            {face(id, info)}
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
          <span className="flex items-center gap-2.5">
            {face(id, info)}
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
      // Put the holder's face on it: "who" reads faster than the sentence.
      const by = last?.params?.by;
      const holder = by
        ? state?.peers.find((p) => !p.is_self && displayName(p) === by)
        : undefined;
      toast(
        <span className="flex items-center gap-2.5">
          {holder && <PeerAvatar peer={holder} size="xs" />}
          <span>{last ? localizeLogEntry(last) : t("editLocked")}</span>
        </span>,
      );
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
      // Progressive join: a snapped-back edit while loading, and the moment
      // editing unlocks. The extension rate-limits the former.
      if (e.code === "loading_readonly" || e.code === "media_ready") {
        toast(localizeLogEntry(e));
        continue;
      }
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
  // Settings is a webview screen on Windows only (macOS has the native
  // window), so the acknowledgements return to it only there.
  const [settingsOpen, setSettingsOpen] = useState(
    resumed?.screen === "settings" || (resumed?.screen === "credits" && isWindows),
  );
  const [creditsOpen, setCreditsOpen] = useState(resumed?.screen === "credits");
  useLang(); // re-render the whole tree when the language changes
  const appVersion = useAppVersion();

  // Remembered across the restart after an update.
  const screenRef = useRef({ settingsOpen, creditsOpen });
  screenRef.current = { settingsOpen, creditsOpen };
  useEffect(() => {
    trackScreen(() =>
      screenRef.current.creditsOpen
        ? "credits"
        : screenRef.current.settingsOpen
          ? "settings"
          : null,
    );
  }, []);

  // The window starts hidden (no blank flash). A normal launch shows it once
  // the first frame is up; the restart after an update waits for REAPER's
  // state too (briefly), so it comes back looking as it left.
  const hasInstanceState = !!instance?.state;
  const revealed = useRef(false);
  useEffect(() => {
    if (revealed.current) return;
    if (!resumed || hasInstanceState) {
      revealed.current = true;
      // After this commit has painted; a timer, since a hidden window may not
      // run animation frames.
      setTimeout(() => revealWindow(!!resumed), 0);
      return;
    }
    const id = setTimeout(() => {
      if (revealed.current) return;
      revealed.current = true;
      revealWindow(true);
    }, 1500);
    return () => clearTimeout(id);
  }, [hasInstanceState]);
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

  // `reamesh://` invite links the OS hands the app (clicked in a chat or a
  // browser), at launch or into the running instance. The Lobby's invite field
  // picks them up from lib/invite; while a session is up they wait there, and
  // the user is told so. Outside Tauri (plain-browser dev) the plugin call
  // rejects and nothing is wired.
  const modeRef = useRef<Mode | undefined>(undefined);
  modeRef.current = state?.mode;
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onOpenUrl((urls) => {
      const link = urls.find(isInviteLink);
      if (!link) return;
      offerInvite(link);
      const m = modeRef.current;
      if (m === "hosting" || m === "client" || m === "downloading") toast(t("inviteWaitsInLobby"));
    })
      .then((f) => (unlisten = f))
      .catch(() => {});
    return () => unlisten?.();
  }, []);
  const reaperClosed = !state && lastLive.current != null;
  const closedWasSession =
    reaperClosed &&
    (lastLive.current?.mode === "hosting" || lastLive.current?.mode === "client");

  // A running REAPER answers within a few hundred ms of launch. Until this
  // grace passes, a missing state shows the blank checking screen instead of
  // "Waiting for REAPER", so a normal start goes straight to the lobby without
  // flashing the waiting screen and its "REAPER found" handoff.
  const [launchGrace, setLaunchGrace] = useState(true);
  useEffect(() => {
    const id = setTimeout(() => setLaunchGrace(false), LAUNCH_GRACE_MS);
    return () => clearTimeout(id);
  }, []);
  const holdBlank = launchGrace && !state && lastLive.current == null;

  // Hold the Disconnected screen briefly after REAPER answers so its
  // success animation finishes before the lobby appears. `waiting` matches
  // the render condition below.
  const hasState = !!state;
  const waiting =
    !settingsOpen &&
    !creditsOpen &&
    reaper !== undefined &&
    !holdBlank &&
    !state &&
    !(reaper && !reaper.installed);
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
    const id = setTimeout(() => setFoundBeat(false), 1750);
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
  // The native window's "Copy diagnostics" asks us for the text (we hold the
  // connection state) and the backend puts it on the pasteboard.
  useEffect(() => {
    const subs = [
      listen("menu:settings", () => {
        refreshStatus();
        setSettingsOpen(true);
      }),
      listen("menu:credits", () => setCreditsOpen(true)),
      listen("menu:settings-changed", () => {
        loadReaperPath().then(refreshStatus);
      }),
      listen("settings:copy-diagnostics", async () => {
        await refreshStatus();
        invoke("copy_diagnostics_text", { text: await buildDiagnostics() }).catch(() => {});
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
  // An extension on a too-old REAPER runs as a stub that drops every command.
  const unsupported = state?.unsupported ?? null;
  useEffect(() => {
    if (mode === "idle" && port != null && !unsupported) send({ cmd: "browse" }, port);
  }, [mode, port, unsupported]);

  // Give the overlay the active instance's port and process. A port change must
  // rebuild the HUD even when both instances are already in sessions.
  const inSession = mode === "hosting" || mode === "client";
  // Switching the overlay off in Settings closes it on the spot.
  const pid = instance?.pid;
  const hud = useOverlaySettings().hud;
  useEffect(() => {
    if (hud && inSession && port != null) openOverlay(port, pid ?? null);
    else closeOverlay();
  }, [hud, inSession, port, pid]);

  // REAPER loads plugins only at startup: after an install it needs a restart
  // only if it was already running then (the app installs at launch, so a
  // REAPER started afterwards already has the new copy).
  const stale = useExtensionStale(instance?.pid);
  const [restartDismissed, setRestartDismissed] = useState(false);
  useEffect(() => {
    const un = listen("extension-installed", () => {
      setRestartDismissed(false);
      refreshStatus();
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  // Also when REAPER still runs an older build of the same protocol (e.g. this
  // is the restart after an app update, whose install happened at launch).
  // Not when it runs the newer copy pre-installed from a downloaded update.
  const loadedOld =
    !!instance?.version &&
    !!appVersion &&
    instance.version !== appVersion &&
    instance.version !== reaper?.preinstalled;
  const skew = instance?.state ? protoSkew(instance.proto) : "same";
  const needsRestart =
    (stale || loadedOld) && hasState && !restartDismissed && skew === "same";

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
  if (creditsOpen) {
    screen = { key: "credits", node: <Credits onBack={() => setCreditsOpen(false)} /> };
  } else if (settingsOpen) {
    screen = {
      key: "settings",
      node: (
        <Settings
          onBack={() => setSettingsOpen(false)}
          onOpenCredits={() => setCreditsOpen(true)}
        />
      ),
    };
  } else if (reaper === undefined || (holdBlank && !(reaper && !reaper.installed))) {
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
  } else if (unsupported) {
    screen = { key: "outdated", node: <Outdated info={unsupported} /> };
  } else if (skew !== "same") {
    screen = { key: "mismatch", node: <ExtMismatch skew={skew} /> };
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
              className="mx-4 mt-2 flex items-center gap-2.5 rounded-xl bg-card px-3 py-2.5"
            >
              <span className="flex-1 text-[12px] text-foreground/80">{t("extUpdatedRestart")}</span>
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
              className="mx-4 mt-2 flex items-center gap-3 rounded-xl bg-card px-3 py-2.5"
            >
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="text-[12.5px] font-medium leading-tight text-foreground">
                  {t("reaperBusyTitle")}
                </span>
                <span className="text-[11.5px] leading-snug text-muted-foreground">
                  {t("reaperBusyHint")}
                </span>
              </span>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-[12px]"
                onClick={() => invoke("focus_reaper", { pid: instance?.pid ?? 0 }).catch(() => {})}
              >
                {t("reaperBusyShow")}
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <AnimatePresence mode="wait">
        <motion.main
          // The waiting screen keeps one key while REAPER answers (the port
          // appears at that moment): the same spinner closes into a blue ring
          // instead of the screen remounting and fading a new one in.
          key={screen.key === "disconnected" ? screen.key : screen.key + (port ?? "")}
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
