import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { TriangleAlert } from "lucide-react";
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
import { localizeLogEntry } from "@/lib/log";
import { displayName, type ExtState, type Mode } from "@/lib/types";

startScanning();
loadReaperPath().then(refreshStatus);

/** Toast people joining/leaving by diffing the peers list between states.
 *  Suppressed across mode changes so entering a session doesn't announce
 *  everyone already there as "joined".
 *
 *  Joins that happen while the user is *away* (this window unfocused and
 *  (checked backend-side) REAPER not frontmost either, so neither this list
 *  nor the overlay roster was visible) additionally badge the dock icon.
 *  Returns the ids of those peers once the user is back: the Session screen
 *  flashes their rows so the returning eye lands on who's new. */
function usePresenceToasts(state: ExtState | undefined): string[] {
  const prev = useRef<
    { mode: Mode; ids: Map<string, { name: string; color: string }> } | undefined
  >(undefined);
  const unseen = useRef<Set<string>>(new Set());
  const [flash, setFlash] = useState<string[]>([]);

  useEffect(() => {
    if (!state) {
      prev.current = undefined;
      return;
    }
    const inSession = state.mode === "hosting" || state.mode === "client";
    // Session over: whatever the badge was announcing no longer exists.
    if (!inSession && unseen.current.size) {
      unseen.current.clear();
      invoke("clear_join_badge").catch(() => {});
    }
    const ids = new Map(
      state.peers
        .filter((p) => !p.is_self)
        .map((p) => [p.id, { name: displayName(p), color: p.color }]),
    );
    const before = prev.current;
    prev.current = { mode: state.mode, ids };
    if (!before || before.mode !== state.mode || !inSession) return;

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
  }, [state]);

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

function useErrorToasts(state: ExtState | undefined) {
  const last = useRef("");
  useEffect(() => {
    const err = state?.error ?? null;
    const key = err ? JSON.stringify(err) : "";
    if (err && key !== last.current) toast.error(localizeLogEntry(err));
    last.current = key;
  }, [state?.error]);
}

/** One of our edits snapped back because someone else holds the soft editing
 *  lock on that field. It reads as a glitch otherwise, so tell the user calmly.
 *  Fires in either role, since the lock binds the host too. Debounced so a quick
 *  back-and-forth can't stack notices. */
function useOverrideToasts(state: ExtState | undefined) {
  const prev = useRef<{ mode?: Mode; count: number; lastAt: number; enteredAt: number }>({
    count: 0,
    lastAt: 0,
    enteredAt: 0,
  });
  useEffect(() => {
    const count = state?.overridden ?? 0;
    const sameSession = prev.current.mode === state?.mode;
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
    prev.current.count = count;
  }, [state?.overridden, state?.mode]);
}

export default function App() {
  const instance = useActiveInstance();
  const state = instance?.state;
  const reaper = useReaperStatus();
  const [settingsOpen, setSettingsOpen] = useState(false);
  useLang(); // re-render the whole tree when the language changes
  const flashPeers = usePresenceToasts(state);
  useErrorToasts(state);
  useOverrideToasts(state);

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

  // The lobby needs discovery running; the extension's browse is idempotent.
  const mode = state?.mode;
  const port = instance?.port;
  useEffect(() => {
    if (mode === "idle") send({ cmd: "browse" });
  }, [mode, port]);

  // The floating presence overlay rides along with the active instance for as
  // long as it's in a session, pinned to that instance's control port.
  const inSession = mode === "hosting" || mode === "client";
  useEffect(() => {
    if (inSession && port != null) openOverlay(port);
    else closeOverlay();
  }, [inSession, port]);

  // Keep the backend's quit handler informed of whether we're hosting and on
  // which control port, so it can stop the session before the app exits.
  useEffect(() => {
    setSessionState(mode === "hosting", port ?? null);
  }, [mode, port]);

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
    screen = { key: "disconnected", node: <Disconnected /> };
  } else if (mode === "downloading") {
    screen = { key: "downloading", node: <Downloading state={state} /> };
  } else if (beatActive && mode === "client") {
    // Keep the "downloading" key so the screen isn't torn down; it just
    // switches to its done state in place.
    screen = { key: "downloading", node: <Downloading state={state} done /> };
  } else if (mode === "hosting" || mode === "client") {
    screen = {
      key: "session",
      node: <Session state={state} port={port ?? null} flashPeers={flashPeers} />,
    };
  } else {
    screen = { key: "lobby", node: <Lobby state={state} /> };
  }

  return (
    // `relative` anchors the Windows title bar layer, which floats the
    // Settings gear over the top-right corner instead of taking a strip.
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
        {instance?.busy && !state?.opening && (
          <motion.div
            key="busy"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            className="overflow-hidden"
          >
            <div className="mx-4 mt-2 flex items-center gap-2.5 rounded-xl bg-amber-500/10 px-3 py-2.5 text-amber-500">
              <TriangleAlert className="size-4 shrink-0" />
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
