// The full-window, click-through overlay that paints, directly over REAPER's
// arrange view, where everyone else is: a thin colored playhead per peer and a
// tinted "ghost" around the items/tracks they have selected ("Alice is holding
// these three items"). It maps each peer's project-time / track GUID onto YOUR
// local view using geometry the extension measures every tick (see
// computeArrangeView in reamesh.cpp), so it stays correct whatever your layout
// or scroll position.
//
// Coordinate model: the extension reports geometry in REAPER SCREEN coordinates
// (logical points). The native layer sizes THIS window to cover REAPER's whole
// window, so a screen point becomes window-relative by subtracting the window's
// own screen origin, the single conversion below (`relX`/`relY`). If alignment
// is ever off on a given machine, that origin (or a constant) is the one knob.

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ArrangeView } from "@/lib/types";
import { displayName } from "@/lib/types";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { useOverlayState } from "./useOverlayState";

/** This window's top-left in logical screen points (it covers REAPER's window,
 *  so this is REAPER's window origin). Polled from Tauri as REAPER moves. */
function useWindowOrigin() {
  const [origin, setOrigin] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const poll = async () => {
      try {
        const [pos, scale] = await Promise.all([win.outerPosition(), win.scaleFactor()]);
        if (alive) setOrigin({ x: pos.x / scale, y: pos.y / scale });
      } catch {
        /* plain-browser dev: no Tauri window APIs */
      }
    };
    poll();
    const id = window.setInterval(poll, 90);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);
  return origin;
}

interface PlayheadDraw {
  id: string;
  x: number;
  top: number;
  bottom: number;
  color: string;
  name: string;
}
interface GhostDraw {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  color: string;
}

export function ArrangeOverlay({ port }: { port: number | null }) {
  const { state } = useOverlayState(port);
  const settings = useOverlaySettings();
  const origin = useWindowOrigin();

  const view = state?.view as ArrangeView | undefined;
  const peers = state?.peers ?? [];
  const tracks = view?.tracks;

  // Nothing measurable (not in a session, or geometry unresolved) -> paint air.
  if (!view || !tracks || view.spp <= 0) return null;

  const relX = (screenX: number) => screenX - origin.x;
  const relY = (screenY: number) => screenY - origin.y;
  const timeToX = (t: number) => relX(view.ax + (t - view.t0) / view.spp);
  const arrangeLeft = relX(view.ax);
  const winW = window.innerWidth;

  const playheads: PlayheadDraw[] = [];
  const ghosts: GhostDraw[] = [];

  for (const peer of peers) {
    if (peer.is_self) continue;
    const color = peer.color || "#888";

    if (settings.playheads && typeof peer.play === "number") {
      const x = timeToX(peer.play);
      if (x >= arrangeLeft - 1 && x <= winW + 1) {
        playheads.push({
          id: peer.id,
          x,
          top: relY(view.top),
          bottom: relY(view.bottom),
          color,
          name: displayName(peer),
        });
      }
    }

    if (settings.ghosts) {
      // Whole-track ghosts (the tracks they have selected).
      for (const guid of peer.seltr ?? []) {
        const tr = tracks[guid];
        if (!tr) continue;
        ghosts.push({
          id: `${peer.id}:tr:${guid}`,
          x: arrangeLeft,
          y: relY(tr.y),
          w: Math.max(0, winW - arrangeLeft),
          h: tr.h,
          color,
        });
      }
      // Item ghosts (the items they're holding).
      for (const guid of peer.seli ?? []) {
        const it = view.items?.[guid];
        const tr = it && tracks[it.tr];
        if (!it || !tr) continue;
        ghosts.push({
          id: `${peer.id}:it:${guid}`,
          x: timeToX(it.pos),
          y: relY(tr.y),
          w: Math.max(2, it.len / view.spp),
          h: tr.h,
          color,
        });
      }
    }
  }

  return (
    <svg
      style={{ position: "fixed", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}
    >
      {ghosts.map((g) => (
        <rect
          key={g.id}
          x={g.x}
          y={g.y}
          width={g.w}
          height={g.h}
          rx={3}
          fill={g.color}
          fillOpacity={0.1}
          stroke={g.color}
          strokeOpacity={0.7}
          strokeWidth={1.5}
        />
      ))}
      {playheads.map((p) => (
        <g key={p.id}>
          <line
            x1={p.x}
            y1={p.top}
            x2={p.x}
            y2={p.bottom}
            stroke={p.color}
            strokeWidth={2}
            strokeOpacity={0.9}
          />
          {/* Name flag at the top of the line. */}
          <rect x={p.x} y={p.top} width={Math.min(96, 8 + p.name.length * 6.2)} height={15} rx={3} fill={p.color} />
          <text x={p.x + 5} y={p.top + 11} fontSize={10} fontWeight={600} fill="#fff">
            {p.name}
          </text>
        </g>
      ))}
    </svg>
  );
}
