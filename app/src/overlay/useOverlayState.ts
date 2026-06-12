// The overlay is a second, read-only consumer of one REAPER instance's
// control server. Unlike the main app's connection manager (which scans every
// port), the overlay is told exactly which port to ride (the same instance
// the main window is in a session on), so it never drifts onto a different
// REAPER. It only listens; it never sends commands or a profile (the main
// window owns that).

import { useEffect, useState } from "react";
import type { ExtState } from "@/lib/types";

const RETRY_MS = 1500;

export function useOverlayState(port: number | null): {
  state: ExtState | null;
  connected: boolean;
} {
  const [state, setState] = useState<ExtState | null>(null);
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    if (!port) return;
    let ws: WebSocket | null = null;
    let retry: number | undefined;
    let closed = false;

    const schedule = () => {
      retry = window.setTimeout(open, RETRY_MS);
    };

    const open = () => {
      try {
        ws = new WebSocket(`ws://127.0.0.1:${port}`);
      } catch {
        schedule();
        return;
      }
      ws.onopen = () => setConnected(true);
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(String(ev.data)) as Record<string, unknown>;
          if (msg.type === "state") setState(msg as unknown as ExtState);
        } catch {
          /* ignore malformed frames */
        }
      };
      ws.onclose = () => {
        setConnected(false);
        if (!closed) schedule();
      };
      ws.onerror = () => ws?.close();
    };

    open();
    return () => {
      closed = true;
      if (retry) clearTimeout(retry);
      ws?.close();
    };
  }, [port]);

  return { state, connected };
}
