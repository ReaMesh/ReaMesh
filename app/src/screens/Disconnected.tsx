import { motion } from "motion/react";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";

// Shown until a REAPER instance with the ReaMesh extension answers on
// localhost. A slim indeterminate sweep reads as "scanning" without the
// busy pulsing rings.
export function Disconnected() {
  const openReaper = async () => {
    try {
      await openPath("/Applications/REAPER.app");
    } catch {
      toast.error("Couldn't open REAPER. Launch it manually.");
    }
  };

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-7 px-10 text-center">
      <div className="h-px w-32 overflow-hidden rounded-full bg-border">
        <motion.div
          className="h-full w-1/3 rounded-full bg-foreground/50"
          animate={{ x: ["-120%", "320%"] }}
          transition={{ duration: 1.6, repeat: Infinity, ease: "easeInOut" }}
        />
      </div>
      <div className="space-y-1.5">
        <h2 className="text-[15px] font-semibold">Waiting for REAPER</h2>
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          Start REAPER to connect.
        </p>
      </div>
      <Button size="sm" onClick={openReaper}>
        Open REAPER
      </Button>
    </div>
  );
}
