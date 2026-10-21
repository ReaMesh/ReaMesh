import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Toaster } from "@/components/ui/sonner";
import { Settings } from "@/screens/Settings";
import { Credits } from "@/screens/Credits";
import { JoinedProjects } from "@/screens/JoinedProjects";
import { startScanning } from "@/lib/connection";
import { t, useLang } from "@/lib/i18n";
import { loadReaperPath, refreshStatus } from "@/lib/install";

// The macOS Settings window (ReaMesh → Settings…, Cmd+,): the same screen
// Windows shows inside the main window, in a window of its own. It keeps its
// own control connections (the extension version, the diagnostics and the
// profile need them) and follows the other windows' changes through the
// backend's events, like they follow its own.
startScanning();
loadReaperPath().then(refreshStatus);

type Sub = "credits" | "joined" | null;

export function SettingsWindow() {
  useLang(); // re-render the whole tree when the language changes
  const [sub, setSub] = useState<Sub>(null);

  // Hidden until the first frame is up (no blank flash), like the main window.
  useEffect(() => {
    const id = setTimeout(() => {
      try {
        const win = getCurrentWindow();
        win.show().catch(() => {});
        win.setFocus().catch(() => {});
      } catch {
        // Not running inside Tauri.
      }
    }, 0);
    return () => clearTimeout(id);
  }, []);

  let screen: { key: string; node: React.ReactNode };
  if (sub === "credits") {
    screen = { key: "credits", node: <Credits onBack={() => setSub(null)} /> };
  } else if (sub === "joined") {
    screen = { key: "joined", node: <JoinedProjects onBack={() => setSub(null)} /> };
  } else {
    screen = {
      key: "settings",
      node: (
        <Settings onOpenCredits={() => setSub("credits")} onOpenJoined={() => setSub("joined")} />
      ),
    };
  }

  return (
    <div className="relative flex h-full flex-col">
      {/* The traffic lights sit over the left end (titleBarStyle Overlay). */}
      <header data-tauri-drag-region className="relative flex h-11 shrink-0 items-center bg-background/80">
        <h1 className="pointer-events-none absolute inset-0 grid place-items-center text-[13px] font-semibold text-foreground/80 select-none">
          {t("settings")}
        </h1>
      </header>
      <AnimatePresence mode="wait">
        <motion.main
          key={screen.key}
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
