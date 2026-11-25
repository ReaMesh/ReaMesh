import { MotionConfig } from "motion/react";
import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import { initLanguage } from "@/lib/i18n";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { HintProvider } from "@/components/Hint";
import { Setup } from "./Setup";

// The Windows installer's window (setup.html, built into dist-setup/ and
// embedded into ../setup). Its backend is ReaMesh Setup's own, not the app's.

const params = new URLSearchParams(window.location.search);

// Browser preview without Tauri (dev only): ?mock=install|update|uninstall
// fakes the backend, including a few seconds of progress; &fail=1 makes the
// job fail at the end.
if (import.meta.env.DEV && params.get("mock")) {
  const kind = params.get("mock");
  const listeners = new Map<string, (payload: unknown) => void>();
  let cbId = 0;
  const callbacks = new Map<number, (e: unknown) => void>();
  const emit = (event: string, payload: unknown) => listeners.get(event)?.(payload);
  const info = {
    mode: kind === "uninstall" ? "uninstall" : "install",
    version: "1.3.1",
    installedVersion: kind === "install" ? null : "1.3.0",
    relation: kind === "install" ? "fresh" : kind === "update" ? "update" : "same",
    dir: "C:\\Users\\you\\AppData\\Local\\ReaMesh",
    defaultDir: "C:\\Users\\you\\AppData\\Local\\ReaMesh",
    appRunning: params.get("running") === "1",
    passive: params.get("passive") === "1",
    update: false,
    needBytes: 17_000_000,
    freeBytes: params.get("full") === "1" ? 10_000_000 : 80_000_000_000,
    extensionInstalled: true,
    dataExists: true,
  };
  const run = (phases: [string, number][]) => {
    let at = 0;
    for (const [phase, ms] of phases) {
      const steps = Math.max(1, Math.round(ms / 40));
      for (let i = 0; i <= steps; i++) {
        setTimeout(() => emit("setup:progress", { phase, value: i / steps }), at + (i * ms) / steps);
      }
      at += ms;
    }
    setTimeout(() => {
      const fail = params.get("fail") === "1";
      emit("setup:done", {
        ok: !fail,
        error: fail ? "C:\\Users\\you\\AppData\\Local\\ReaMesh\\app.exe: Access is denied. (os error 5)" : null,
        restartReaper: kind === "uninstall",
      });
    }, at + 120);
  };
  // deno-lint-ignore no-explicit-any
  (window as unknown as { __TAURI_INTERNALS__: any }).__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
      windows: [{ label: "main" }],
      webviews: [{ label: "main", windowLabel: "main" }],
    },
    transformCallback: (cb: (e: unknown) => void) => {
      callbacks.set(++cbId, cb);
      return cbId;
    },
    unregisterCallback: () => {},
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string, args: Record<string, unknown>) => {
      switch (cmd) {
        case "setup_info":
          return info;
        case "get_language":
          return params.get("lang") ?? "system";
        case "plugin:event|listen": {
          const cb = callbacks.get(args.handler as number);
          listeners.set(args.event as string, (payload) => cb?.({ event: args.event, payload }));
          return args.handler;
        }
        case "start_install":
          run([["verify", 300], ["close", 200], ["copy", 2600], ["register", 400]]);
          return null;
        case "start_uninstall":
          run([["close", 300], ["remove", 1800]]);
          return null;
        default:
          return undefined;
      }
    },
  };
}

window.addEventListener("contextmenu", (e) => e.preventDefault());

initLanguage();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <MotionConfig reducedMotion="user">
      <ErrorBoundary where="setup">
        <HintProvider>
          <Setup />
        </HintProvider>
      </ErrorBoundary>
    </MotionConfig>
  </React.StrictMode>,
);
