import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
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
 *  everyone already there as "joined". */
function usePresenceToasts(state: ExtState | undefined) {
  const prev = useRef<
    { mode: Mode; ids: Map<string, { name: string; color: string }> } | undefined
  >(undefined);

  useEffect(() => {
    if (!state) {
      prev.current = undefined;
      return;
    }
    const inSession = state.mode === "hosting" || state.mode === "client";
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

/** Client side: a genuine simultaneous conflict was resolved in the host's
 *  favour, so one of our edits snapped back. Without a word that reads as a
 *  glitch, so tell the user. With active-editor-wins these are rare; still
 *  debounce, so a quick back-and-forth can never stack notices. */
function useOverrideToasts(state: ExtState | undefined) {
  const prev = useRef<{ mode?: Mode; count: number; lastAt: number }>({ count: 0, lastAt: 0 });
  useEffect(() => {
    const count = state?.overridden ?? 0;
    const sameSession = prev.current.mode === state?.mode;
    const now = Date.now();
    if (
      state?.mode === "client" &&
      sameSession &&
      count > prev.current.count &&
      now - prev.current.lastAt > 4000
    ) {
      // Name the track when the extension could resolve it (the most recent
      // override event carries it); otherwise fall back to the generic notice.
      const last = [...(state?.log ?? [])].reverse().find((e) => e.code === "edit_overridden");
      const track = last?.params?.track;
      toast(track ? t("editOverriddenTrack", { track }) : t("editOverridden"));
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
  usePresenceToasts(state);
  useErrorToasts(state);
  useOverrideToasts(state);

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
  } else if (reaper && !reaper.installed && !state) {
    screen = { key: "onboarding", node: <Onboarding status={reaper} /> };
  } else if (!state) {
    screen = { key: "disconnected", node: <Disconnected /> };
  } else if (mode === "downloading") {
    screen = { key: "downloading", node: <Downloading state={state} /> };
  } else if (mode === "hosting" || mode === "client") {
    screen = { key: "session", node: <Session state={state} /> };
  } else {
    screen = { key: "lobby", node: <Lobby state={state} /> };
  }

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
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
