import { useEffect, useRef } from "react";
import { AnimatePresence, motion } from "motion/react";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { TitleBar } from "@/components/TitleBar";
import { SettingsDialog } from "@/components/SettingsDialog";
import { Disconnected } from "@/screens/Disconnected";
import { Lobby } from "@/screens/Lobby";
import { Downloading } from "@/screens/Downloading";
import { Onboarding } from "@/screens/Onboarding";
import { Session } from "@/screens/Session";
import { startScanning, send, useActiveInstance } from "@/lib/connection";
import { refreshStatus, useReaperStatus } from "@/lib/install";
import { displayName, type ExtState, type Mode } from "@/lib/types";

startScanning();
refreshStatus();

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
              <b>{info.name}</b> joined the session
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
              <b>{info.name}</b> left the session
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
    const err = state?.error ?? "";
    if (err && err !== last.current) toast.error(err);
    last.current = err;
  }, [state?.error]);
}

export default function App() {
  const instance = useActiveInstance();
  const state = instance?.state;
  const reaper = useReaperStatus();
  usePresenceToasts(state);
  useErrorToasts(state);

  // The lobby needs discovery running; the extension's browse is idempotent.
  const mode = state?.mode;
  const port = instance?.port;
  useEffect(() => {
    if (mode === "idle") send({ cmd: "browse" });
  }, [mode, port]);

  // Until the bundled extension is installed into REAPER's UserPlugins,
  // nothing can connect; run onboarding first. `undefined` means the
  // backend check hasn't answered yet (it's local fs, near-instant), and
  // `null` means no Tauri backend (plain-browser dev); both skip the gate.
  let screen: { key: string; node: React.ReactNode };
  if (reaper === undefined) {
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
      <SettingsDialog />
      <Toaster />
    </div>
  );
}
