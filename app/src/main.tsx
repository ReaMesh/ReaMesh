import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import "slot-text/style.css";
import { initLanguage } from "./lib/i18n";

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
        <Overlay port={port} />
      </React.StrictMode>,
    );
  });
} else {
  import("./App").then(({ default: App }) => {
    root.render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  });
}
