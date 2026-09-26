import { MotionConfig } from "motion/react";
import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import { initLanguage } from "./lib/i18n";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Two entry points share this bundle. The floating presence HUD is opened as a
// separate Tauri window pointed at `index.html?overlay=1&port=<n>`; every other
// window is the main companion app. We branch with dynamic imports so the
// overlay window never pulls in App.tsx's module-level scanning side effects
// (and vice versa).
const params = new URLSearchParams(window.location.search);

// Hidden tabs may pause requestAnimationFrame and block Motion transitions.
// In development, ?raf=shim uses timers so automated previews can finish.
if (import.meta.env.DEV && params.get("raf") === "shim") {
  window.requestAnimationFrame = (cb) =>
    window.setTimeout(() => cb(performance.now()), 16) as unknown as number;
  window.cancelAnimationFrame = (id) => clearTimeout(id);
}

// Mock enough of Tauri to preview onboarding in a plain browser. The flag flips
// after installation so the complete flow can be tested without REAPER.
if (import.meta.env.DEV && params.get("mock") === "onboarding") {
  let installed = false;
  const status = () => ({
    reaperFound: params.get("step") !== "locate",
    isReaperDir: true,
    resourcePath: "/Users/you/Library/Application Support/REAPER",
    userPlugins: "/Users/you/Library/Application Support/REAPER/UserPlugins",
    installed,
    upToDate: installed,
  });
  let cbId = 0;
  // deno-lint-ignore no-explicit-any
  (window as unknown as { __TAURI_INTERNALS__: any }).__TAURI_INTERNALS__ = {
    transformCallback: () => ++cbId,
    unregisterCallback: () => {},
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string) => {
      switch (cmd) {
        case "reaper_status":
          return status();
        case "install_extension":
          installed = true;
          return status();
        case "get_reaper_path":
          return null;
        case "get_language":
          return "system";
        case "firewall_allowed":
          return params.get("fw") === "allowed";
        default:
          return undefined; // Event listeners and setters are no-ops in the mock.
      }
    },
  };
}
// Mock a fully installed setup for browser previews of the in-session screens
// (with a fake control server on a ctl port). `plugins` = comma-separated
// display names the fake `installed_plugins` scan reports.
if (import.meta.env.DEV && params.get("mock") === "tauri") {
  let cbId = 0;
  const plugins = (params.get("plugins") ?? "").split(",").filter(Boolean);
  // deno-lint-ignore no-explicit-any
  // Window metadata lets @tauri-apps/api/window resolve getCurrentWindow(), so
  // the overlay HUD (?overlay=1) previews too; its size/move calls are no-ops.
  const label = params.get("overlay") === "1" ? "overlay" : "main";
  (window as unknown as { __TAURI_INTERNALS__: any }).__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label },
      currentWebview: { label, windowLabel: label },
      windows: [{ label }],
      webviews: [{ label, windowLabel: label }],
    },
    transformCallback: () => ++cbId,
    unregisterCallback: () => {},
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string) => {
      switch (cmd) {
        case "reaper_status":
          return {
            reaperFound: true,
            isReaperDir: true,
            resourcePath: "/Users/you/Library/Application Support/REAPER",
            userPlugins: "/Users/you/Library/Application Support/REAPER/UserPlugins",
            installed: true,
            upToDate: true,
          };
        case "installed_plugins":
          return plugins;
        case "get_reaper_path":
          return null;
        case "get_language":
          return params.get("lang") ?? "system";
        case "get_overlay_settings":
          return 3; // roster + tint on, like a fresh install
        case "firewall_allowed":
          return params.get("fw") === "allowed"; // ?fw=allowed hides the firewall notice
        default:
          return undefined;
      }
    },
  };
}
const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

// Both windows render localized strings; sync the language preference with the
// backend before (and while) either tree is mounted.
initLanguage();

if (params.get("overlay") === "1") {
  document.documentElement.classList.add("overlay-window");
  const port = Number(params.get("port")) || null;
  import("./overlay/Overlay").then(({ Overlay }) => {
    root.render(
      <React.StrictMode>
        {/* Honor the OS "reduce motion" setting for every Motion animation. */}
        <MotionConfig reducedMotion="user">
          <ErrorBoundary where="overlay">
            <Overlay port={port} />
          </ErrorBoundary>
        </MotionConfig>
      </React.StrictMode>,
    );
  });
} else {
  import("./App").then(({ default: App }) => {
    root.render(
      <React.StrictMode>
        <MotionConfig reducedMotion="user">
          <ErrorBoundary>
            <App />
          </ErrorBoundary>
        </MotionConfig>
      </React.StrictMode>,
    );
  });
}
