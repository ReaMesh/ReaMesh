// Read state from the exact REAPER instance selected by the main window. The
// overlay only sends `reveal_peer`; session controls stay in the main window.

import { useCallback, useEffect, useRef, useState } from "react";
import { normalizeState, type ExtState } from "@/lib/types";

const RETRY_MS = 1500;

export function useOverlayState(port: number | null): {
  state: ExtState | null;
  connected: boolean;
  send: (cmd: Record<string, unknown>) => void;
} {
  const [state, setState] = useState<ExtState | null>(null);
  const [connected, setConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);

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
      wsRef.current = ws;
      ws.onopen = () => setConnected(true);
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(String(ev.data)) as Record<string, unknown>;
          // Same version-skew hardening the main window's socket applies: the
          // frame comes from a separately-versioned process, and a `peers` that
          // isn't an array would take the whole HUD down on render.
          if (msg.type === "state") setState(normalizeState(msg));
        } catch {
          /* ignore malformed frames */
        }
      };
      ws.onclose = () => {
        setConnected(false);
        if (wsRef.current === ws) wsRef.current = null;
        if (!closed) schedule();
      };
      ws.onerror = () => ws?.close();
    };

    open();
    return () => {
      closed = true;
      if (retry) clearTimeout(retry);
      wsRef.current = null;
      ws?.close();
    };
  }, [port]);

  const send = useCallback((cmd: Record<string, unknown>) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(cmd));
  }, []);

  return { state, connected, send };
}
